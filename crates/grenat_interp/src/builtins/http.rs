//! `Http`: the HTTP client of the standard library.
//!
//! ```ruby
//! res = Http.get("https://api.github.com/repos/acme/app", headers: {"Accept" => "application/json"})
//! res.status   # 200
//! res.ok?      # status in 200..299
//! res.json     # the body, parsed
//! Http.post(url, json: {title: "Bug"}, timeout: 5)
//! Http.get("https://api.x.io/search", query: {q: "rust & grenat"})  # encoded
//! ```
//!
//! A request is a `net` effect: its host must be allowed by every function
//! on the stack that declares its effects (`uses net("api.github.com")`).
//! Nothing untrusted may go out (a tainted URL, header or body is a
//! `TaintError`), and what comes back is untrusted: the body and headers of
//! a response are tainted, as a model's answer is.

use std::time::Duration;

use crate::http::{HttpReply, HttpRequest, host};
use crate::prelude::*;

use super::*;

/// The record a request returns.
pub(crate) const RESPONSE: &str = "HttpResponse";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn call_http<'p>(interp: &mut Interp<'p>, name: &str, args: Args<'p>) -> R<'p> {
    let method = match name {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "patch" => "PATCH",
        "delete" => "DELETE",
        "head" => "HEAD",
        _ => return raise("NoMethodError", format!("unknown method `Http.{name}`")),
    };
    let target = format!("Http.{name}");
    if args.pos.iter().chain(args.named.iter().map(|(_, v)| v)).any(Value::contains_taint) {
        return raise("TaintError", format!("an untrusted value reaches `{target}` (effect `net`) without validation"));
    }
    let url = text_arg(&args, 0, name)?;
    let Some(host) = host(&url) else {
        return raise("ArgumentError", format!("`{target}` expects an http(s) URL, got {url:?}"));
    };
    interp.check_net(host, &url)?;
    let request = request(method, url.clone(), &args)?;
    // an error names the URL: one holding a secret is not named
    let secret_url = args.pos[0].contains_secret()
        || args.named.iter().any(|(n, v)| n == "query" && v.contains_secret());
    let reply = match interp.http(&request) {
        Err(Ctrl::Raise(e)) if secret_url => {
            return raise(&e.ty, e.message.replace(&request.url, &format!("{} [secret]", host)));
        }
        other => other?,
    };
    if interp.log {
        // a URL holding a secret is logged as `[secret]`
        let shown = if args.pos[0].contains_secret() { args.pos[0].to_display() } else { url };
        interp.write_err(&format!("[http] {method} {shown} → {}\n", reply.status));
    }
    Ok(response(reply))
}

fn request<'p>(method: &'static str, url: String, args: &Args<'p>) -> Result<HttpRequest, Ctrl<'p>> {
    let mut request = HttpRequest { method, url, headers: Vec::new(), body: None, timeout: DEFAULT_TIMEOUT };
    for (option, value) in &args.named {
        match (option.as_str(), value) {
            ("headers", Value::Hash(entries)) => {
                request.headers.extend(entries.borrow().iter().map(|(k, v)| (k.to_display(), v.reveal())));
            }
            ("json", value) => {
                request.body = Some(crate::llm::revealed_json(value).to_string());
                request.headers.push(("Content-Type".into(), "application/json".into()));
            }
            ("body", Value::Str(text) | Value::Secret(text)) => request.body = Some(text.to_string()),
            ("query", Value::Hash(entries)) => {
                let params: Vec<(String, String)> =
                    entries.borrow().iter().map(|(k, v)| (k.to_display(), v.reveal())).collect();
                request.url = crate::http::with_query(&request.url, &params);
            }
            ("timeout", Value::Int(n)) if *n > 0 => request.timeout = Duration::from_secs(*n as u64),
            ("timeout", Value::Float(s) | Value::Duration(s)) if *s > 0.0 => {
                request.timeout = Duration::from_secs_f64(*s);
            }
            (option, value) => {
                return raise("ArgumentError", format!("invalid `Http` option `{option}: {}`", value.inspect()));
            }
        }
    }
    Ok(request)
}

/// `HttpResponse(status:, headers:, body:)`, headers and body tainted.
fn response<'p>(reply: HttpReply) -> Value<'p> {
    let headers = reply.headers.into_iter().map(|(k, v)| (Value::str(k), Value::str(v).taint())).collect();
    Value::record(
        RESPONSE,
        vec![
            ("status".into(), Value::Int(i64::from(reply.status))),
            ("headers".into(), Value::Hash(Arc::new(Mutex::new(headers)))),
            ("body".into(), Value::str(reply.body).taint()),
        ],
    )
}

/// `res.ok?` and `res.json`.
pub(crate) fn response_method<'p>(fields: &Fields<'p>, name: &str) -> Option<R<'p>> {
    let field = |n: &str| fields.iter().find(|(k, _)| &**k == n).map(|(_, v)| v.clone()).unwrap_or(Value::Nil);
    match name {
        "ok?" => Some(Ok(Value::Bool(matches!(field("status"), Value::Int(200..=299))))),
        "json" => Some(match serde_json::from_str::<serde_json::Value>(&field("body").to_display()) {
            Ok(json) => Ok(json_to_untyped(&json).taint()),
            Err(e) => raise("ParseError", format!("the response is not JSON: {e}")),
        }),
        _ => None,
    }
}
