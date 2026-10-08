//! Error messages, made fit to leave the process: every credential the run
//! has read (or a test gave) blanked out as `[secret]`, wherever it shows.
//! A secret is shown as `[secret]` already; this is for one a transport
//! named once revealed (a URL in a connection error…).

use serde_json::Value as Json;

use crate::prelude::*;

/// Shorter values are not blanked out: they would blank out common words.
const SHORTEST: usize = 6;

impl<'p> Interp<'p> {
    pub(crate) fn redact(&self, message: &str) -> String {
        let mut secrets = Vec::new();
        if let Some(Ok(tree)) = &*self.credentials_cache.borrow() {
            leaves(tree, &mut secrets);
        }
        if let Some(tree) = &*self.credentials_double.borrow() {
            leaves(tree, &mut secrets);
        }
        // the longest first: a secret that contains another is blanked out whole
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let mut message = message.to_string();
        for secret in secrets.iter().filter(|s| s.len() >= SHORTEST) {
            message = message.replace(secret.as_str(), "[secret]");
        }
        message
    }
}

/// Every text and number of a credentials tree.
fn leaves(tree: &Json, out: &mut Vec<String>) {
    match tree {
        Json::String(text) => out.push(text.clone()),
        Json::Number(n) => out.push(n.to_string()),
        Json::Array(items) => items.iter().for_each(|item| leaves(item, out)),
        Json::Object(map) => map.values().for_each(|value| leaves(value, out)),
        Json::Bool(_) | Json::Null => {}
    }
}
