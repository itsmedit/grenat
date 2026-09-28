//! Calls of `native def` functions: Rust code a native facet ships, run
//! through its library (see `grenat_native`).
//!
//! The arguments go as JSON, the result comes back decoded into the
//! declared type. A native function's effects are its callers' — checked
//! against their capabilities before it runs, since Grenat cannot watch
//! what Rust code does — its result is untrusted unless it is `pure`, and
//! no secret is ever handed to it. An `Err` raises the facet's error type
//! (`NativeError` by default); a panic, caught in the library, a `NativeError`.

use serde_json::Value as Json;

use crate::llm::{Ty, value_to_json};
use crate::prelude::*;

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

    /// Runs the native function `def`, its parameters bound in the current frame.
    pub(crate) fn call_foreign(&mut self, def: &'p FnDef) -> R<'p> {
        let name = def.name.name.as_str();
        let values: Vec<Value<'p>> =
            def.params.iter().map(|p| scope_get(self.scope(), &p.name.name).unwrap_or(Value::Nil)).collect();
        if values.iter().any(Value::contains_secret) {
            return raise(
                "SecretError",
                format!("a secret is never handed to native code: `{name}` runs outside Grenat's sandbox"),
            );
        }
        let tainted = values.iter().any(Value::contains_taint);
        let args =
            serde_json::to_vec(&Json::Array(values.iter().map(value_to_json).collect())).expect("JSON values encode");
        let outcome = match self.natives.call(name, &args) {
            Ok(outcome) => outcome,
            Err(e) => return raise("NativeError", e),
        };
        if self.log {
            let facet = self.natives.function(name).map_or("?", |(facet, _)| facet);
            self.write_err(&format!("[native] {facet}: {name}\n"));
        }
        match outcome {
            grenat_native::Outcome::Returned(json) => {
                let ty = match &def.ret {
                    Some(t) => self.ty(t).or_else(|e| raise("TypeError", e))?,
                    None => Ty::Nil,
                };
                let value = match self.json_to_value(&json, &ty) {
                    Ok(value) => value,
                    Err(e) => return raise("NativeError", format!("`{name}` returned a value of another type: {e}")),
                };
                let pure = def.pure || self.natives.function(name).is_some_and(|(_, f)| f.pure);
                Ok(if pure && !tainted { value } else { value.taint() })
            }
            grenat_native::Outcome::Raised { ty, message } => raise(&ty, message),
            grenat_native::Outcome::Panicked(message) => raise("NativeError", format!("`{name}` panicked: {message}")),
        }
    }
}
