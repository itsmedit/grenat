//! Triggers: what wakes a served program (`grenat serve`).
//!
//! ```ruby
//! every 1.week do … end                        # or: every cron: "0 8 * * MON" (UTC)
//! on_webhook "/github", secret: Env.fetch("GITHUB_WEBHOOK_SECRET"), signature: :github do |req|
//!   req.json["action"]                          # untrusted
//!   "ok"                                        # the response
//! end
//! ```
//!
//! A request whose signature (or token) does not match is refused (401)
//! before the handler runs. What a request carries is untrusted. In tests,
//! `deliver_webhook "/github", json: {…}` sends a request, signed as it
//! must be, through the same path.

use grenat_serve::Cron;

use crate::prelude::*;

use super::*;

/// When a schedule runs.
pub(crate) enum Every {
    Seconds(f64),
    Cron(Cron),
}

pub(crate) struct Schedule<'p> {
    pub every: Every,
    pub block: Value<'p>,
    pub label: String,
}

/// How a webhook proves where it comes from.
#[derive(Clone)]
pub(crate) enum Proof {
    None,
    /// `X-Hub-Signature-256`, as GitHub signs.
    Github(String),
    /// `Authorization: Bearer <token>`.
    Token(String),
}

pub(crate) struct Webhook<'p> {
    pub path: String,
    pub proof: Proof,
    pub block: Value<'p>,
}

pub(crate) fn every<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let block = block(args, "every")?;
    let (every, label) = match (args.pos.first().map(Value::untainted), args.named.first()) {
        (Some(Value::Duration(s) | Value::Float(s)), None) if *s > 0.0 => (Every::Seconds(*s), format!("every {s}s")),
        (Some(Value::Int(n)), None) if *n > 0 => (Every::Seconds(*n as f64), format!("every {n}s")),
        (None, Some((name, Value::Str(text)))) if name == "cron" => {
            let cron = Cron::parse(text).or_else(|e| raise("ArgumentError", e))?;
            (Every::Cron(cron), format!("every cron: {text}"))
        }
        _ => return raise("ArgumentError", "`every` expects a duration (`every 1.hour`) or `cron: \"0 8 * * MON\"`"),
    };
    interp.schedules.borrow_mut().push(Schedule { every, block, label });
    Ok(Value::Nil)
}

pub(crate) fn on_webhook<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let path = str_arg(args, 0, "on_webhook")?.to_string();
    if !path.starts_with('/') {
        return raise("ArgumentError", format!("a webhook path starts with `/`, got {path:?}"));
    }
    let named = |n: &str| args.named.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let proof = match (named("signature"), named("secret"), named("token")) {
        (Some(Value::Symbol(kind)), Some(secret), None) if &*kind == "github" => Proof::Github(secret.reveal()),
        (None, None, Some(token)) => Proof::Token(token.reveal()),
        (None, None, None) => Proof::None,
        _ => return raise("ArgumentError", "`on_webhook` takes `secret:, signature: :github`, or `token:`"),
    };
    let block = block(args, "on_webhook")?;
    interp.webhooks.borrow_mut().push(Webhook { path, proof, block });
    Ok(Value::Nil)
}

/// `deliver_webhook "/path", json: {…}` (or `body:`, `headers:`): the
/// response, as a hash (`status`, `body`).
pub(crate) fn deliver_webhook<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    let path = str_arg(args, 0, "deliver_webhook")?.to_string();
    let mut raw = RawRequest {
        method: "POST".into(),
        path: path.clone(),
        query: String::new(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("json", v) => {
                raw.body = crate::llm::value_to_json(v).to_string().into_bytes();
                raw.headers.push(("Content-Type".into(), "application/json".into()));
            }
            ("body", v) => raw.body = v.to_display().into_bytes(),
            ("headers", Value::Hash(h)) => {
                raw.headers.extend(h.borrow().iter().map(|(k, v)| (k.to_display(), v.to_display())))
            }
            (option, v) => {
                return raise("ArgumentError", format!("invalid `deliver_webhook` option `{option}: {}`", v.inspect()));
            }
        }
    }
    // signed as the sender would sign it, unless the test gives its own
    let proof = interp.webhooks.borrow().iter().find(|w| w.path == path).map(|w| w.proof.clone());
    let given = |name: &str| raw.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name));
    match proof {
        Some(Proof::Github(secret)) if !given("X-Hub-Signature-256") => {
            let signature = grenat_serve::signature::github(&secret, &raw.body);
            raw.headers.push(("X-Hub-Signature-256".into(), signature));
        }
        Some(Proof::Token(token)) if !given("Authorization") => {
            raw.headers.push(("Authorization".into(), format!("Bearer {token}")))
        }
        _ => {}
    }
    let answer = interp.handle_webhook(raw)?;
    Ok(answer_value(answer))
}

impl<'p> Interp<'p> {
    /// Answers a webhook request. A request that does not prove where it
    /// comes from never reaches its handler.
    pub(crate) fn handle_webhook(&mut self, raw: RawRequest) -> Result<HttpAnswer, Ctrl<'p>> {
        let found =
            self.webhooks.borrow().iter().find(|w| w.path == raw.path).map(|w| (w.proof.clone(), w.block.clone()));
        let Some((proof, handler)) = found else { return Ok(HttpAnswer::text(404, "no such webhook")) };
        if raw.method != "POST" {
            return Ok(HttpAnswer::text(405, "webhooks are POST requests"));
        }
        let header =
            |name: &str| raw.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
        let proven = match &proof {
            Proof::None => true,
            Proof::Github(secret) => header("X-Hub-Signature-256")
                .is_some_and(|given| grenat_serve::signature::github_valid(secret, &raw.body, &given)),
            Proof::Token(token) => header("Authorization").is_some_and(|given| given == format!("Bearer {token}")),
        };
        if !proven {
            if self.log {
                self.write_err(&format!("[webhook] {}: refused (bad signature or token)\n", raw.path));
            }
            return Ok(HttpAnswer::text(401, "unauthorized"));
        }
        let request = request_value(&raw, Vec::new());
        let answer = self.call_block(&handler, vec![request])?;
        Ok(answer_of(&answer))
    }
}
