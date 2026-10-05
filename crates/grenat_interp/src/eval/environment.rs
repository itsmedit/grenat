//! The environment a program reads: `Env.get(name)` (`nil` when unset),
//! `Env.fetch(name)` (`KeyError` when unset), `Env.fetch(name, default)`,
//! `Env.key?(name)`. Plain strings: secrets come from `Credentials`.
//!
//! Tests never see the process's environment, so that they pass the same
//! everywhere: under `grenat test`, `Env` reads only what the running test
//! gave with `mock_env({"GITHUB_TOKEN" => "t"})` (merged when called again,
//! forgotten after the test), and the file's top-level code, which loads
//! before any test, sees an empty environment (`Env.fetch(name, default)`
//! gives the default).

use crate::builtins::{arg, str_arg};
use crate::prelude::*;

impl<'p> Interp<'p> {
    /// `Env.get`, `Env.fetch`, `Env.key?`.
    pub(crate) fn call_env(&self, name: &str, args: &Args<'p>) -> R<'p> {
        let key = str_arg(args, 0, name)?;
        let value = self.env_var(&key);
        match (name, value) {
            ("get", value) => Ok(value.map_or(Value::Nil, Value::str)),
            ("key?", value) => Ok(Value::Bool(value.is_some())),
            ("fetch", Some(value)) => Ok(Value::str(value)),
            ("fetch", None) => match args.pos.get(1) {
                Some(default) => Ok(default.clone()),
                None if self.offline => raise(
                    "KeyError",
                    format!("missing environment variable `{key}` (tests see only what `mock_env` gives)"),
                ),
                None => raise("KeyError", format!("missing environment variable `{key}`")),
            },
            _ => raise("NoMethodError", format!("unknown method `Env.{name}`")),
        }
    }

    /// The variable `name`: the test's double under `grenat test`, else the process's.
    fn env_var(&self, name: &str) -> Option<String> {
        if self.offline {
            return self.env_double.borrow().get(name).cloned();
        }
        std::env::var(name).ok()
    }

    /// `mock_env({"GITHUB_TOKEN" => "t"})`: what `Env` reads until the end of the test.
    pub(crate) fn mock_env(&mut self, args: &Args<'p>) -> R<'p> {
        const USAGE: &str = "mock_env({\"GITHUB_TOKEN\" => \"t\"})";
        self.only_in_tests("mock_env", USAGE)?;
        let Value::Hash(entries) = arg(args, 0, "mock_env")?.untainted().clone() else {
            return raise("TypeError", format!("`mock_env` takes a hash of strings: `{USAGE}`"));
        };
        let mut variables = Vec::new();
        for (key, value) in entries.borrow().iter() {
            match (key.untainted(), value.untainted()) {
                (Value::Str(k) | Value::Symbol(k), Value::Str(v)) => variables.push((k.to_string(), v.to_string())),
                (_, Value::Secret(_)) => {
                    return raise(
                        "SecretError",
                        "`Env` gives plain strings: a secret is given with `mock_credentials`",
                    );
                }
                _ => {
                    return raise(
                        "TypeError",
                        format!(
                            "`mock_env` takes a hash of strings: `{USAGE}`, got {} => {}",
                            key.inspect(),
                            value.inspect()
                        ),
                    );
                }
            }
        }
        self.env_double.borrow_mut().extend(variables);
        Ok(Value::Nil)
    }
}
