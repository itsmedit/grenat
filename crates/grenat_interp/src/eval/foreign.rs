//! Calls of `native def` functions: Rust code a native facet ships, run
//! through its library (see `grenat_native`), or Ruby and Python code a
//! bridge facet ships, run by its server process (see `grenat_bridge`).
//!
//! The arguments go as JSON, the result comes back decoded into the
//! declared type. A native function's effects are its callers' — checked
//! against their capabilities before it runs, since Grenat cannot watch
//! what foreign code does — its result is untrusted unless it is `pure`,
//! and no secret is ever handed to it. An `Err` or an exception raises the
//! error type the facet names (`NativeError`, `BridgeError` by default); a
//! panic, caught in the library, a `NativeError`; a bridge's server that
//! dies or runs too long, a `BridgeError`.

use serde_json::Value as Json;

use crate::llm::{Ty, value_to_json};
use crate::prelude::*;
use crate::{Options, Output};

/// Where a `native def` is implemented.
#[derive(Clone, Copy)]
enum Origin {
    Library,
    Bridge,
}

impl Origin {
    /// The error raised when the call itself fails.
    fn error(self) -> &'static str {
        match self {
            Origin::Library => "NativeError",
            Origin::Bridge => grenat_bridge::protocol::BRIDGE_ERROR,
        }
    }

    /// As `--log` shows it: `[native]`, `[bridge]`.
    fn tag(self) -> &'static str {
        match self {
            Origin::Library => "native",
            Origin::Bridge => "bridge",
        }
    }
}

/// The log of bridge servers' standard error (`--log`), written where the
/// program writes its errors.
pub(crate) fn bridge_log(options: &Options) -> Option<grenat_bridge::Log> {
    if !options.log {
        return None;
    }
    Some(match &options.output {
        Output::Stdout => std::sync::Arc::new(|line: &str| {
            use std::io::Write as _;
            let _ = std::io::stderr().lock().write_all(line.as_bytes());
        }),
        Output::Capture(buffer) => {
            let buffer = buffer.clone();
            std::sync::Arc::new(move |line: &str| buffer.lock().unwrap_or_else(|e| e.into_inner()).push_str(line))
        }
    })
}

impl<'p> Interp<'p> {
    /// The effects of the native function `def` must be allowed by every
    /// function on the stack that declares its own.
    pub(crate) fn check_native_effects(&self, def: &FnDef) -> Result<(), Ctrl<'p>> {
        for effect in &def.effects {
            let path: Vec<&str> = effect.path.iter().map(|i| i.name.as_str()).collect();
            self.check_effect(&path.join("."))?;
        }
        Ok(())
    }

    /// Where `name` is implemented: a library or a bridge, the facet that
    /// ships it (`?` if none does), and whether its manifest says it is pure.
    fn foreign(&self, name: &str) -> (Origin, String, bool) {
        let (origin, found) = match self.bridges.function(name) {
            Some(found) if self.natives.function(name).is_none() => (Origin::Bridge, Some(found)),
            _ => (Origin::Library, self.natives.function(name)),
        };
        (origin, found.map_or("?", |(facet, _)| facet).to_string(), found.is_some_and(|(_, f)| f.pure))
    }

    /// Runs the native function `def`, its parameters bound in the current frame.
    pub(crate) fn call_foreign(&mut self, def: &'p FnDef) -> R<'p> {
        let name = def.name.name.as_str();
        let (origin, facet, pure) = self.foreign(name);
        let values: Vec<Value<'p>> =
            def.params.iter().map(|p| scope_get(self.scope(), &p.name.name).unwrap_or(Value::Nil)).collect();
        if values.iter().any(Value::contains_secret) {
            return raise(
                "SecretError",
                format!("a secret is never handed to {} code: `{name}` runs outside Grenat's sandbox", origin.tag()),
            );
        }
        let tainted = values.iter().any(Value::contains_taint);
        let args =
            serde_json::to_vec(&Json::Array(values.iter().map(value_to_json).collect())).expect("JSON values encode");
        let called = match origin {
            Origin::Library => self.natives.call(name, &args),
            Origin::Bridge => {
                // the server may take long to answer: off the scheduler's workers
                let bridges = &self.bridges;
                grenat_green::blocking(|| bridges.call(name, &args))
            }
        };
        let outcome = match called {
            Ok(outcome) => outcome,
            Err(e) => return raise(origin.error(), e),
        };
        if self.log {
            self.write_err(&format!("[{}] {facet}: {name}\n", origin.tag()));
        }
        match outcome {
            grenat_native::Outcome::Returned(json) => {
                let ty = match &def.ret {
                    Some(t) => self.ty(t).or_else(|e| raise("TypeError", e))?,
                    None => Ty::Nil,
                };
                let value = match self.json_to_value(&json, &ty) {
                    Ok(value) => value,
                    Err(e) => return raise(origin.error(), format!("`{name}` returned a value of another type: {e}")),
                };
                Ok(if (def.pure || pure) && !tainted { value } else { value.taint() })
            }
            grenat_native::Outcome::Raised { ty, message } => raise(&ty, message),
            grenat_native::Outcome::Panicked(message) => raise("NativeError", format!("`{name}` panicked: {message}")),
        }
    }
}
