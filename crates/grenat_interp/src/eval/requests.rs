//! The HTTP requests a test's program sent, answered by `mock_http` or not:
//! `Http.requests`, in order, so that a test asserts what a service received.
//!
//! ```ruby
//! req = Http.requests.last
//! assert_equal "POST", req["method"]
//! assert_equal({"title" => "Bug"}, req["json"])
//! assert_equal "Bearer t", req["headers"]["Authorization"]
//! ```
//!
//! Each one is a hash with string keys: `method`, `url` (its query
//! included), `headers` (as given, with the `Content-Type` a `json:` body
//! adds), `body` (the text sent, `nil` without one) and `json` (the body
//! parsed, `nil` when it is not JSON). Secrets stay secrets: a header, a URL
//! or a body that holds one is a `Secret`, equal to its text, never printed.
//! Only a test records them, and each test starts with none.

use crate::builtins::json_to_untyped;
use crate::llm::{revealed_json, value_to_json};
use crate::prelude::*;

impl<'p> Interp<'p> {
    /// Records a request of `Http.<method>(url, …)` (or `Audio.url`), in a test.
    pub(crate) fn record_request(&self, method: &str, url: &str, args: &Args<'p>) {
        if !self.in_test() {
            return;
        }
        let request = sent(method, url, args);
        self.sent_requests.borrow_mut().push(request);
    }

    /// `Http.requests`: what the running test sent.
    pub(crate) fn sent_requests(&self) -> R<'p> {
        self.only_in_tests("Http.requests", "Http.requests.last")?;
        Ok(Value::array(self.sent_requests.borrow().clone()))
    }
}

/// A request as the service received it.
fn sent<'p>(method: &str, url: &str, args: &Args<'p>) -> Value<'p> {
    let url_secret = args.pos.first().is_some_and(Value::contains_secret)
        || args.named.iter().any(|(n, v)| n == "query" && v.contains_secret());
    let mut headers: Vec<(Value<'p>, Value<'p>)> = Vec::new();
    let (mut body, mut json) = (Value::Nil, Value::Nil);
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("headers", Value::Hash(entries)) => {
                headers.extend(entries.borrow().iter().map(|(k, v)| (Value::str(k.to_display()), text(v))));
            }
            ("json", value) => {
                body = secret_if(value.contains_secret(), revealed_json(value).to_string());
                json = received(value);
                headers.push((Value::str("Content-Type"), Value::str("application/json")));
            }
            ("body", given @ (Value::Str(_) | Value::Secret(_))) => {
                body = given.clone();
                json = parsed(given);
            }
            _ => {}
        }
    }
    let pairs = vec![
        (Value::str("method"), Value::str(method)),
        (Value::str("url"), secret_if(url_secret, url.to_string())),
        (Value::str("headers"), Value::Hash(Arc::new(Mutex::new(headers)))),
        (Value::str("body"), body),
        (Value::str("json"), json),
    ];
    Value::Hash(Arc::new(Mutex::new(pairs)))
}

/// `text`, as a secret when it holds one.
fn secret_if<'p>(secret: bool, text: String) -> Value<'p> {
    if secret { Value::Secret(text.into()) } else { Value::str(text) }
}

/// A header's value: its text, or the secret it is.
fn text<'p>(value: &Value<'p>) -> Value<'p> {
    match value.untainted() {
        Value::Secret(s) => Value::Secret(s.clone()),
        other => Value::str(other.to_display()),
    }
}

/// A body sent as `json:`, as the service parses it: string keys, records
/// as objects, and each secret still a secret.
fn received<'p>(value: &Value<'p>) -> Value<'p> {
    if !value.contains_secret() {
        return json_to_untyped(&value_to_json(value));
    }
    let object = |pairs: Vec<(Value<'p>, Value<'p>)>| Value::Hash(Arc::new(Mutex::new(pairs)));
    match value.untainted() {
        Value::Secret(s) => Value::Secret(s.clone()),
        Value::Array(items) => Value::array(items.borrow().iter().map(received).collect()),
        Value::Hash(entries) => {
            object(entries.borrow().iter().map(|(k, v)| (Value::str(k.to_display()), received(v))).collect())
        }
        Value::Record(r) => object(r.fields.iter().map(|(k, v)| (Value::str(&**k), received(v))).collect()),
        Value::Variant(v) => {
            let mut pairs: Vec<_> = v.fields.iter().map(|(k, v)| (Value::str(&**k), received(v))).collect();
            pairs.push((Value::str("kind"), Value::str(&*v.name)));
            object(pairs)
        }
        other => json_to_untyped(&value_to_json(other)),
    }
}

/// A `body:` parsed as JSON (`nil` when it is not); from a secret body,
/// every text is a secret.
fn parsed<'p>(body: &Value<'p>) -> Value<'p> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body.reveal()) else {
        return Value::Nil;
    };
    let value = json_to_untyped(&json);
    if matches!(body, Value::Secret(_)) { secret_texts(&value) } else { value }
}

/// `value` with each of its texts made a secret.
fn secret_texts<'p>(value: &Value<'p>) -> Value<'p> {
    match value {
        Value::Str(s) => Value::Secret(s.clone()),
        Value::Array(items) => Value::array(items.borrow().iter().map(secret_texts).collect()),
        Value::Hash(entries) => Value::Hash(Arc::new(Mutex::new(
            entries.borrow().iter().map(|(k, v)| (k.clone(), secret_texts(v))).collect(),
        ))),
        other => other.clone(),
    }
}
