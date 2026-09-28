//! What runs inside an entry point: arguments decoded from JSON, the
//! function called with panics caught, its result or error encoded back.

use std::fmt::Display;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value as Json, json};

use crate::abi::{Buffer, STATUS_ERROR, STATUS_OK, STATUS_PANIC};

/// A Grenat error to raise: its type and message.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub ty: String,
    pub message: String,
}

impl Failure {
    fn new(ty: &str, message: impl Into<String>) -> Failure {
        Failure { ty: ty.to_string(), message: message.into() }
    }
}

/// The arguments of one call, decoded one by one.
pub struct Args {
    values: std::vec::IntoIter<Json>,
    function: &'static str,
}

impl Args {
    /// `bytes`: a JSON array of exactly `count` values.
    pub fn parse(bytes: &[u8], count: usize, function: &'static str) -> Result<Args, Failure> {
        let values: Vec<Json> = serde_json::from_slice(bytes)
            .map_err(|e| Failure::new("ArgumentError", format!("`{function}`: arguments are not a JSON array: {e}")))?;
        if values.len() != count {
            return Err(Failure::new(
                "ArgumentError",
                format!("`{function}` takes {count} argument(s), got {}", values.len()),
            ));
        }
        Ok(Args { values: values.into_iter(), function })
    }

    /// The next argument, as the parameter `name` of type `T`.
    pub fn next<T: DeserializeOwned>(&mut self, name: &str) -> Result<T, Failure> {
        let value = self.values.next().unwrap_or(Json::Null);
        serde_json::from_value(value)
            .map_err(|e| Failure::new("TypeError", format!("`{}`: argument `{name}`: {e}", self.function)))
    }
}

/// A result, as JSON.
pub fn returned<T: Serialize>(value: T) -> Result<Json, Failure> {
    serde_json::to_value(value).map_err(|e| Failure::new("NativeError", format!("the result cannot be encoded: {e}")))
}

/// An `Err`, raised as the Grenat error `ty`.
pub fn raised<E: Display>(error: E, ty: &str) -> Failure {
    Failure::new(ty, error.to_string())
}

/// Runs `body` on the argument bytes and writes its outcome into `out`;
/// returns the status. A panic is caught here: it never unwinds into the
/// caller, which gets its message (status [`STATUS_PANIC`]) instead.
///
/// # Safety
/// `args` points to `len` readable bytes (or `len` is 0), `out` is writable.
pub unsafe fn entry(
    args: *const u8,
    len: usize,
    out: *mut Buffer,
    body: impl FnOnce(&[u8]) -> Result<Json, Failure>,
) -> i32 {
    let bytes: &[u8] = if len == 0 || args.is_null() {
        &[]
    } else {
        // SAFETY: `len` readable bytes, as the caller guarantees
        unsafe { std::slice::from_raw_parts(args, len) }
    };
    let (status, json) = match crate::panics::catch(|| body(bytes)) {
        Ok(Ok(value)) => (STATUS_OK, value),
        Ok(Err(failure)) => (STATUS_ERROR, json!({"type": failure.ty, "message": failure.message})),
        Err(message) => (STATUS_PANIC, json!({"type": "NativePanic", "message": message})),
    };
    let bytes = serde_json::to_vec(&json).unwrap_or_else(|_| b"null".to_vec());
    // SAFETY: `out` is writable, as the caller guarantees
    unsafe { out.write(Buffer::from_vec(bytes)) };
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `body` as an entry point would, on `args`; the status and JSON written.
    fn run(args: &str, body: impl FnOnce(&[u8]) -> Result<Json, Failure>) -> (i32, Json) {
        let mut out = Buffer::empty();
        // SAFETY: a live string and a local buffer
        let status = unsafe { entry(args.as_ptr(), args.len(), &mut out, body) };
        // SAFETY: filled by `entry`, in this library
        let bytes = unsafe { out.into_vec() };
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn arguments_are_decoded_in_order() {
        let (status, json) = run("[2, \"x\"]", |bytes| {
            let mut args = Args::parse(bytes, 2, "f")?;
            let n: i64 = args.next("n")?;
            let s: String = args.next("s")?;
            returned(format!("{s}{n}"))
        });
        assert_eq!((status, json), (STATUS_OK, json!("x2")));
    }

    #[test]
    fn bad_arguments_are_errors() {
        let (status, json) = run("[1]", |bytes| returned(Args::parse(bytes, 2, "f").map(|_| ())?));
        assert_eq!(status, STATUS_ERROR);
        assert_eq!(json, json!({"type": "ArgumentError", "message": "`f` takes 2 argument(s), got 1"}));
        let (status, json) = run("[\"one\"]", |bytes| {
            let n: i64 = Args::parse(bytes, 1, "f")?.next("n")?;
            returned(n)
        });
        assert_eq!(status, STATUS_ERROR);
        assert_eq!(json["type"], "TypeError");
        assert!(json["message"].as_str().unwrap().starts_with("`f`: argument `n`: invalid type"), "{json}");
        let (_, json) = run("{", |bytes| returned(Args::parse(bytes, 0, "f").map(|_| ())?));
        assert!(json["message"].as_str().unwrap().contains("not a JSON array"), "{json}");
    }

    #[test]
    fn errors_and_panics_are_caught() {
        let (status, json) = run("[]", |_| Err(raised("no such sheet", "SheetError")));
        assert_eq!((status, json), (STATUS_ERROR, json!({"type": "SheetError", "message": "no such sheet"})));
        let (status, json) = run("[]", |_| panic!("boom {}", 42));
        assert_eq!((status, &json["type"]), (STATUS_PANIC, &json!("NativePanic")));
        assert!(json["message"].as_str().unwrap().starts_with("boom 42 (at "), "{json}");
    }
}
