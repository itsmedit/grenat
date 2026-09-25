//! Web applications: routes, requests, responses (served by `grenat serve`).
//!
//! ```ruby
//! get "/tickets/:id" do |req|
//!   json(find_ticket(req.params["id"].to_i))     # params: the path's and the query's, untrusted
//! end
//! post "/tickets" do |req|
//!   status 201, json(create(req.json))
//! end
//! get "/" do |req|
//!   html "<h1>#{Html.escape(req.params["name"])}</h1>"
//! end
//! ```
//!
//! What a request carries is untrusted. A page is a sink: `html` refuses an
//! untrusted value that was not escaped (`Html.escape`), so a page cannot
//! carry a script someone slipped in; `redirect` refuses an untrusted URL.
//! Tests send requests without a server: `request :get, "/tickets/1"`.

use crate::prelude::*;

use super::*;

/// The record a handler receives (routes and webhooks).
pub(crate) const REQUEST: &str = "Request";
/// The record `html`, `json`, `status` and `redirect` build.
pub(crate) const RESPONSE_RECORD: &str = "Response";

/// A request received, before a handler sees it.
pub(crate) struct RawRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What goes back to the client.
pub(crate) struct HttpAnswer {
    pub status: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpAnswer {
    pub(crate) fn text(status: u16, body: &str) -> HttpAnswer {
        HttpAnswer { status, content_type: "text/plain; charset=utf-8".into(), headers: Vec::new(), body: body.into() }
    }
}

pub(crate) struct Route<'p> {
    pub method: String,
    /// Segments: a literal, or `:name`.
    pub pattern: Vec<String>,
    pub block: Value<'p>,
}

/// `get "/path" do |req| … end` (and `post`, `put`, `patch`, `delete`).
pub(crate) fn declare_route<'p>(interp: &mut Interp<'p>, method: &str, args: &Args<'p>) -> R<'p> {
    let path = str_arg(args, 0, method)?.to_string();
    if !path.starts_with('/') {
        return raise("ArgumentError", format!("a route starts with `/`, got {path:?}"));
    }
    let block = block(args, method)?;
    let pattern = segments(&path).into_iter().map(str::to_string).collect();
    interp.routes.borrow_mut().push(Route { method: method.to_uppercase(), pattern, block });
    Ok(Value::Nil)
}

fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// The path parameters of `path` if `pattern` matches it.
fn matches(pattern: &[String], path: &str) -> Option<Vec<(String, String)>> {
    let parts = segments(path);
    if parts.len() != pattern.len() {
        return None;
    }
    let mut params = Vec::new();
    for (p, part) in pattern.iter().zip(parts) {
        match p.strip_prefix(':') {
            Some(name) => params.push((name.to_string(), decode(part))),
            None if p == part => {}
            None => return None,
        }
    }
    Some(params)
}

/// `a%20b+c` → `a b c`.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `a=1&b=x%20y` → [(a, 1), (b, x y)].
fn query_params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(p), String::new()),
        })
        .collect()
}

/// The `Request` record: its headers, body and parameters untrusted.
pub(crate) fn request_value<'p>(raw: &RawRequest, path_params: Vec<(String, String)>) -> Value<'p> {
    let untrusted = |pairs: Vec<(String, String)>| {
        Value::Hash(Arc::new(Mutex::new(pairs.into_iter().map(|(k, v)| (Value::str(k), Value::str(v).taint())).collect())))
    };
    let mut params = query_params(&raw.query);
    params.extend(path_params);
    let headers = raw.headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
    Value::record(
        REQUEST,
        vec![
            ("method".into(), Value::str(&raw.method)),
            ("path".into(), Value::str(&raw.path)),
            ("query".into(), Value::str(&raw.query).taint()),
            ("params".into(), untrusted(params)),
            ("headers".into(), untrusted(headers)),
            ("body".into(), Value::str(String::from_utf8_lossy(&raw.body)).taint()),
        ],
    )
}

/// What a handler's value means as an answer.
pub(crate) fn answer_of<'p>(value: &Value<'p>) -> HttpAnswer {
    match value.untainted() {
        Value::Nil => HttpAnswer::text(204, ""),
        Value::Int(status) => HttpAnswer::text(u16::try_from(*status).unwrap_or(500), ""),
        Value::Str(text) => HttpAnswer::text(200, text),
        Value::Record(r) if &*r.ty == RESPONSE_RECORD => {
            let field = |n: &str| r.fields.iter().find(|(k, _)| &**k == n).map(|(_, v)| v.clone()).unwrap_or(Value::Nil);
            let headers = match field("headers") {
                Value::Hash(h) => h.borrow().iter().map(|(k, v)| (k.to_display(), v.to_display())).collect(),
                _ => Vec::new(),
            };
            HttpAnswer {
                status: match field("status") {
                    Value::Int(n) => u16::try_from(n).unwrap_or(500),
                    _ => 200,
                },
                content_type: field("content_type").to_display(),
                headers,
                body: field("body").to_display(),
            }
        }
        other => HttpAnswer {
            status: 200,
            content_type: "application/json".into(),
            headers: Vec::new(),
            body: crate::llm::value_to_json(other).to_string(),
        },
    }
}

fn response<'p>(status: i64, content_type: &str, headers: Vec<(Value<'p>, Value<'p>)>, body: String) -> Value<'p> {
    Value::record(
        RESPONSE_RECORD,
        vec![
            ("status".into(), Value::Int(status)),
            ("content_type".into(), Value::str(content_type)),
            ("headers".into(), Value::Hash(Arc::new(Mutex::new(headers)))),
            ("body".into(), Value::str(body)),
        ],
    )
}

/// `html`, `json`, `status`, `redirect`.
pub(crate) fn response_helper<'p>(name: &str, args: &Args<'p>) -> R<'p> {
    match name {
        "html" => {
            let page = arg(args, 0, name)?;
            if page.contains_taint() {
                return raise(
                    "TaintError",
                    "an untrusted value reaches `html` (a page) without escaping: `Html.escape(…)` it first",
                );
            }
            Ok(response(200, "text/html; charset=utf-8", Vec::new(), page.to_display()))
        }
        "json" => {
            let value = arg(args, 0, name)?;
            Ok(response(200, "application/json", Vec::new(), crate::llm::value_to_json(&value).to_string()))
        }
        "redirect" => {
            let to = arg(args, 0, name)?;
            if to.contains_taint() {
                return raise("TaintError", "an untrusted value is the URL of `redirect`: check it first");
            }
            Ok(response(302, "text/plain; charset=utf-8", vec![(Value::str("Location"), Value::str(to.to_display()))], String::new()))
        }
        "status" => {
            let code = int_arg(args, 0, name)?;
            match args.pos.get(1).map(Value::untainted) {
                Some(Value::Record(r)) if &*r.ty == RESPONSE_RECORD => {
                    let fields = r.fields.iter().map(|(k, v)| (k.clone(), if &**k == "status" { Value::Int(code) } else { v.clone() })).collect();
                    Ok(Value::record(RESPONSE_RECORD, fields))
                }
                Some(other) => {
                    let answer = answer_of(other);
                    Ok(response(code, &answer.content_type, Vec::new(), answer.body))
                }
                None => Ok(response(code, "text/plain; charset=utf-8", Vec::new(), String::new())),
            }
        }
        _ => raise("NoMethodError", format!("unknown function `{name}`")),
    }
}

/// `request :get, "/path", json: {…}` (or `body:`, `headers:`): the
/// answer, as a hash (`status`, `body`, `content_type`, `headers`).
pub(crate) fn test_request<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let method = match args.pos.first().map(Value::untainted) {
        Some(Value::Symbol(m)) => m.to_uppercase(),
        _ => return raise("ArgumentError", "`request` expects a method and a path: `request :get, \"/tickets\"`"),
    };
    let target = str_arg(args, 1, "request")?.to_string();
    let (path, query) = target.split_once('?').map_or((target.clone(), String::new()), |(p, q)| (p.into(), q.into()));
    let mut raw = RawRequest { method, path, query, headers: Vec::new(), body: Vec::new() };
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("json", v) => {
                raw.body = crate::llm::value_to_json(v).to_string().into_bytes();
                raw.headers.push(("Content-Type".into(), "application/json".into()));
            }
            ("body", v) => raw.body = v.to_display().into_bytes(),
            ("headers", Value::Hash(h)) => raw.headers.extend(h.borrow().iter().map(|(k, v)| (k.to_display(), v.to_display()))),
            (option, v) => return raise("ArgumentError", format!("invalid `request` option `{option}: {}`", v.inspect())),
        }
    }
    let answer = interp.handle_request(raw)?;
    Ok(answer_value(answer))
}

/// An answer as a hash, for tests.
pub(crate) fn answer_value<'p>(answer: HttpAnswer) -> Value<'p> {
    let headers = answer.headers.into_iter().map(|(k, v)| (Value::str(k), Value::str(v))).collect();
    let pairs = vec![
        (Value::str("status"), Value::Int(i64::from(answer.status))),
        (Value::str("body"), Value::str(answer.body)),
        (Value::str("content_type"), Value::str(answer.content_type)),
        (Value::str("headers"), Value::Hash(Arc::new(Mutex::new(headers)))),
    ];
    Value::Hash(Arc::new(Mutex::new(pairs)))
}

impl<'p> Interp<'p> {
    /// Answers a request: exposed tools, else a webhook, else a route, else
    /// 404 (405 when the path exists for another method).
    pub(crate) fn handle_request(&mut self, raw: RawRequest) -> Result<HttpAnswer, Ctrl<'p>> {
        if let Some(answer) = self.handle_exposed(&raw) {
            return answer;
        }
        if self.webhooks.borrow().iter().any(|w| w.path == raw.path) {
            return self.handle_webhook(raw);
        }
        let mut other_method = false;
        let found = self.routes.borrow().iter().find_map(|route| {
            let params = matches(&route.pattern, &raw.path)?;
            if route.method != raw.method {
                other_method = true;
                return None;
            }
            Some((route.block.clone(), params))
        });
        match found {
            Some((block, params)) => {
                let request = request_value(&raw, params);
                let value = self.call_block(&block, vec![request])?;
                Ok(answer_of(&value))
            }
            None if other_method => Ok(HttpAnswer::text(405, "method not allowed")),
            None => Ok(HttpAnswer::text(404, "not found")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_decoding() {
        let pattern: Vec<String> = ["tickets", ":id"].map(String::from).to_vec();
        assert_eq!(matches(&pattern, "/tickets/42"), Some(vec![("id".into(), "42".into())]));
        assert_eq!(matches(&pattern, "/tickets/42/x"), None);
        assert_eq!(matches(&pattern, "/users/42"), None);
        assert_eq!(decode("a%20b+c%2F"), "a b c/");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(query_params("q=rust+lang&n=5&flag"), [("q".into(), "rust lang".into()), ("n".into(), "5".into()), ("flag".into(), String::new())]);
    }
}
