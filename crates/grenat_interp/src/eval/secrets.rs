//! Secrets: the application's credentials (`Credentials.fetch(:github,
//! :token)`), as `Secret` values — shown as `[secret]`, never sent to a
//! model, never journaled, queued or stored; revealed only where they serve
//! (HTTP URLs and headers, connections, webhook signatures).
//!
//! Credentials are read once, when first fetched: the encrypted file of the
//! environment (`grenat_config`), or in tests those `mock_credentials` gives
//! — and when a test has none, `Credentials.fetch(:github, :token)` is the
//! stand-in secret `test-github-token`.

use serde_json::Value as Json;

use crate::builtins::arg;
use crate::prelude::*;

/// What a secret refused says.
pub(crate) const TO_A_MODEL: &str = "a secret never reaches a model: keep it for headers, URLs and connections";

/// Refuses `values` if a secret is among them: they go to a model.
pub(crate) fn not_for_models<'p>(values: &[Value<'p>]) -> Result<(), Ctrl<'p>> {
    if values.iter().any(Value::contains_secret) {
        return raise("SecretError", TO_A_MODEL);
    }
    Ok(())
}

impl<'p> Interp<'p> {
    /// `Credentials.fetch(:github, :token)` (`dig`: `nil` when missing).
    pub(crate) fn credential(&mut self, args: &Args<'p>, required: bool) -> R<'p> {
        let mut path = Vec::new();
        for key in &args.pos {
            match key.untainted() {
                Value::Symbol(s) | Value::Str(s) => path.push(s.to_string()),
                other => return raise("TypeError", format!("`Credentials.fetch` takes keys (`:github, :token`), got {}", other.inspect())),
            }
        }
        if path.is_empty() {
            return raise("ArgumentError", "`Credentials.fetch` takes keys: `Credentials.fetch(:github, :token)`");
        }
        let credentials = match self.credentials_tree() {
            Ok(credentials) => credentials,
            // tests need no secrets: without credentials, each one stands for itself
            Err(_) if self.offline => return Ok(Value::Secret(format!("test-{}", path.join("-")).into())),
            Err(e) => return Err(e),
        };
        let mut node = &credentials;
        for key in &path {
            node = match node.get(key) {
                Some(next) => next,
                None if required => {
                    return raise("KeyError", format!("no credential `{}` (grenat credentials edit)", path.join(".")));
                }
                None => return Ok(Value::Nil),
            };
        }
        match node {
            Json::String(text) => Ok(Value::Secret(text.as_str().into())),
            Json::Number(n) => Ok(Value::Secret(n.to_string().into())),
            Json::Bool(b) => Ok(Value::Secret(b.to_string().into())),
            Json::Null if !required => Ok(Value::Nil),
            Json::Null => raise("KeyError", format!("the credential `{}` is empty", path.join("."))),
            _ => raise("TypeError", format!("`{}` holds several credentials: fetch one of its keys", path.join("."))),
        }
    }

    /// The credentials, read once: those of the tests' double, else the
    /// environment's file.
    pub(crate) fn credentials_tree(&mut self) -> Result<Json, Ctrl<'p>> {
        if let Some(double) = self.credentials_double.borrow().clone() {
            return Ok(double);
        }
        if let Some(loaded) = self.credentials_cache.borrow().clone() {
            return loaded.or_else(|e| raise("CredentialsError", e));
        }
        let loaded = match &self.credentials_root {
            Some(root) => {
                let location = grenat_config::credentials::Location::for_env(root, &grenat_config::environment());
                if location.exists() {
                    grenat_green::blocking(|| location.load())
                } else {
                    Err(format!("no credentials: {} (grenat credentials edit)", location.file.display()))
                }
            }
            None => Err("no credentials: this program is not in an application (grenat new --app)".into()),
        };
        *self.credentials_cache.borrow_mut() = Some(loaded.clone());
        loaded.or_else(|e| raise("CredentialsError", e))
    }

    /// `mock_credentials({"github" => {"token" => "t"}})`, in tests.
    pub(crate) fn mock_credentials(&mut self, args: &Args<'p>) -> R<'p> {
        let tree = crate::llm::value_to_json(&arg(args, 0, "mock_credentials")?);
        if !tree.is_object() {
            return raise("TypeError", "`mock_credentials` takes a hash: `mock_credentials({\"github\" => {\"token\" => \"t\"}})`");
        }
        *self.credentials_double.borrow_mut() = Some(tree);
        Ok(Value::Nil)
    }
}
