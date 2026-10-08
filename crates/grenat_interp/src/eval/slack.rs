//! Slack from a program: `Slack.post(channel, text)` posts a message
//! (`chat.postMessage`), and the runtime calls the Web API the same way for
//! the approvals it posts (see `eval::slack_approvals`).
//!
//! ```ruby
//! Slack.post("#ops", "Deploy of #{version} done")    # the message's `ts`
//! Slack.post("C0123", text, token: Credentials.fetch(:slack, :ops_token))
//! ```
//!
//! Posting is `net("slack.com")`. Nothing untrusted is posted (the channel,
//! the text or the token: a `TaintError`, E0412 statically): a model's
//! answer or a webhook's payload reaches a channel once checked. The token
//! is `token:`, or else the credential `slack.bot_token`; it reaches the
//! request's header only. The `ts` Slack answers is untrusted, as anything
//! from outside. In tests the call is a request as any other: `mock_http
//! "POST https://slack.com/api/chat.postMessage", json: {ok: true, ts:
//! "1.2"}` answers it, and `Http.requests` lists it (the token a `Secret`).

use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::builtins::json_to_untyped;
use crate::prelude::*;
use crate::slack::{HOST, api};

/// How long a call of `Slack.post`, or the post of an approval, may last.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

impl<'p> Interp<'p> {
    /// Calls Web API `method` with `body`: Slack's answer, or a `SlackError`
    /// that names the method and Slack's error code (never the token).
    pub(crate) fn slack_call(&mut self, method: &str, token: &str, body: Json, timeout: Duration) -> R<'p> {
        let request = api::request(&self.slack_api, method, token, &body, timeout);
        self.record_slack_request(&request.url, token, &body);
        let reply = match self.http(&request) {
            Ok(reply) => reply,
            // the transport's error names the URL, never the token
            Err(Ctrl::Raise(e)) => return raise("SlackError", format!("{method}: {}", e.message)),
            Err(other) => return Err(other),
        };
        let answer = api::reply(method, &reply).or_else(|e| raise("SlackError", e))?;
        if self.log {
            self.write_err(&format!("[slack] {method} → ok\n"));
        }
        Ok(json_to_untyped(&answer))
    }

    /// In a test, the request as `Http.requests` lists it: its token a secret.
    fn record_slack_request(&self, url: &str, token: &str, body: &Json) {
        let headers = vec![(Value::str("Authorization"), Value::Secret(format!("Bearer {token}").into()))];
        let args = Args {
            named: vec![
                ("headers".into(), Value::Hash(Arc::new(Mutex::new(headers)))),
                ("json".into(), json_to_untyped(body)),
            ],
            ..Args::default()
        };
        self.record_request("POST", url, &args);
    }

    /// `Slack.post(channel, text, token:)`: the posted message's `ts`.
    pub(crate) fn slack_post(&mut self, args: &Args<'p>) -> R<'p> {
        const USAGE: &str = "`Slack.post(\"#ops\", \"Deploy done\")`";
        if args.pos.iter().chain(args.named.iter().map(|(_, v)| v)).any(Value::contains_taint) {
            return raise("TaintError", "an untrusted value reaches `Slack.post` (effect `net`) without validation");
        }
        let (channel, text) = match args.pos.as_slice() {
            [channel, text] => match (channel.untainted(), text.untainted()) {
                (Value::Str(channel), Value::Str(text)) if !channel.trim().is_empty() => {
                    (channel.to_string(), text.to_string())
                }
                (Value::Secret(_), _) | (_, Value::Secret(_)) => {
                    return raise("SecretError", "a secret is never posted: `Slack.post` takes a channel and a text");
                }
                _ => return raise("TypeError", format!("`Slack.post` expects a channel and a text: {USAGE}")),
            },
            _ => return raise("ArgumentError", format!("`Slack.post` expects a channel and a text: {USAGE}")),
        };
        let mut token = None;
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("token", Value::Str(t) | Value::Secret(t)) => token = Some(t.to_string()),
                (option, v) => {
                    let message =
                        format!("invalid `Slack.post` option `{option}: {}` (it takes `token:`)", v.inspect());
                    return raise("ArgumentError", message);
                }
            }
        }
        let url = format!("https://{HOST}/api/chat.postMessage");
        self.check_net(HOST, &url)?;
        let token = match token {
            Some(token) => token,
            None => self.bot_token()?,
        };
        let body = json!({"channel": channel, "text": text, "unfurl_links": false, "unfurl_media": false});
        let answer = self.slack_call("chat.postMessage", &token, body, TIMEOUT)?;
        let ts = match &answer {
            Value::Hash(h) => h.borrow().iter().find(|(k, _)| k.to_display() == "ts").map(|(_, v)| v.to_display()),
            _ => None,
        };
        Ok(Value::str(ts.unwrap_or_default()).taint())
    }

    /// The credential `slack.bot_token`, revealed for a request's header.
    fn bot_token(&mut self) -> Result<String, Ctrl<'p>> {
        let path = Args { pos: vec![Value::str("slack"), Value::str("bot_token")], ..Args::default() };
        match self.credential(&path, true) {
            Ok(token) => Ok(token.reveal()),
            Err(Ctrl::Raise(e)) => {
                raise(&e.ty, format!("`Slack.post` takes `token:`, or the credential `slack.bot_token`: {}", e.message))
            }
            Err(other) => Err(other),
        }
    }
}

/// Seconds since the epoch, as Slack's timestamps count them.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Interp<'_> {
    /// Whether `raw` comes from Slack: signed with `secret` within five
    /// minutes of now, and never seen before (a replay is refused). Why
    /// not, otherwise.
    pub(crate) fn slack_signed(&self, secret: &str, raw: &crate::builtins::RawRequest) -> Result<(), String> {
        let header =
            |name: &str| raw.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        let (timestamp, signature) = (header("X-Slack-Request-Timestamp"), header("X-Slack-Signature"));
        let now = unix_now();
        grenat_serve::signature::slack_check(secret, timestamp, signature, &raw.body, now)
            .map_err(|e| e.to_string())?;
        let sent = timestamp.and_then(|t| t.trim().parse().ok()).unwrap_or(now);
        if !self.slack_replays.borrow_mut().first(signature.unwrap_or_default().trim(), sent, now) {
            return Err("replayed".into());
        }
        Ok(())
    }

    /// The headers Slack would send with `body` now, signed with `secret` —
    /// at an earlier second if this one's signature was already accepted, as
    /// a request sent again by Slack is signed anew (test doubles).
    pub(crate) fn slack_headers(&self, secret: &str, body: &[u8]) -> Vec<(String, String)> {
        let mut timestamp = unix_now();
        let mut signature = grenat_serve::signature::slack(secret, &timestamp.to_string(), body);
        while self.slack_replays.borrow().seen(&signature) {
            timestamp -= 1;
            signature = grenat_serve::signature::slack(secret, &timestamp.to_string(), body);
        }
        vec![("X-Slack-Request-Timestamp".into(), timestamp.to_string()), ("X-Slack-Signature".into(), signature)]
    }
}
