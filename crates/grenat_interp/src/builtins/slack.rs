//! Slack: `Slack.post` (see `eval::slack`), and approvals decided with one
//! click (see `eval::slack_approvals`), declared once:
//!
//! ```ruby
//! approvals_in_slack channel: "#approvals",
//!   token: Credentials.fetch(:slack, :bot_token),
//!   signing_secret: Credentials.fetch(:slack, :signing_secret),
//!   allowed: ["U0123ABC"]                    # optional: who may decide
//! ```
//!
//! `path:` is the route Slack sends clicks to (the app's Interactivity
//! Request URL; `/slack/interactivity` by default). The declaration posts
//! nothing: it is held to `net("slack.com")`, and its token and secret
//! must be trusted (a `String` or a `Secret`). In tests,
//! `deliver_slack_click` clicks a button (see `eval::slack_double`).

use crate::prelude::*;
use crate::slack::{CLICKS_PATH, HOST, SlackApprovals, is_id};

const USAGE: &str = "`approvals_in_slack channel: \"#approvals\", token: Credentials.fetch(:slack, :bot_token), signing_secret: Credentials.fetch(:slack, :signing_secret)`";

/// `Slack.<name>(…)`.
pub(crate) fn call_slack<'p>(interp: &mut Interp<'p>, name: &str, args: Args<'p>) -> R<'p> {
    match name {
        "post" => interp.slack_post(&args),
        _ => raise("NoMethodError", format!("unknown method `Slack.{name}`")),
    }
}

pub(crate) fn approvals_in_slack<'p>(interp: &mut Interp<'p>, args: &Args<'p>) -> R<'p> {
    if args.named.iter().any(|(_, v)| v.contains_taint()) {
        return raise(
            "TaintError",
            "an untrusted value reaches `approvals_in_slack` (effect `net`) without validation",
        );
    }
    if !args.pos.is_empty() || args.block.is_some() {
        return raise("ArgumentError", format!("`approvals_in_slack` takes options: {USAGE}"));
    }
    let mut slack = SlackApprovals {
        channel: String::new(),
        token: String::new(),
        signing_secret: String::new(),
        allowed: Vec::new(),
        path: CLICKS_PATH.into(),
    };
    for (option, value) in &args.named {
        match (option.as_str(), value.untainted()) {
            ("channel", Value::Str(c)) if !c.trim().is_empty() => slack.channel = c.trim().to_string(),
            ("token", Value::Str(t) | Value::Secret(t)) if !t.trim().is_empty() => slack.token = t.trim().to_string(),
            ("signing_secret", Value::Str(s) | Value::Secret(s)) if !s.trim().is_empty() => {
                slack.signing_secret = s.trim().to_string();
            }
            ("allowed", Value::Array(users)) => {
                for user in users.borrow().iter() {
                    match user.untainted() {
                        Value::Str(id) if is_id(id) => slack.allowed.push(id.to_string()),
                        other => {
                            let message = format!(
                                "`approvals_in_slack` takes `allowed:` Slack user IDs (\"U0123ABC\"), got {}",
                                other.inspect()
                            );
                            return raise("ArgumentError", message);
                        }
                    }
                }
            }
            ("path", Value::Str(p)) if p.starts_with('/') => slack.path = p.to_string(),
            (option, v) => {
                // a secret is never shown, even refused
                let shown = if v.contains_secret() { "[secret]".to_string() } else { v.inspect() };
                let message = format!(
                    "invalid `approvals_in_slack` option `{option}: {shown}` (it takes `channel:`, `token:`, `signing_secret:`, `allowed:`, `path:`)"
                );
                return raise("ArgumentError", message);
            }
        }
    }
    for (missing, what) in [
        (slack.channel.is_empty(), "channel:"),
        (slack.token.is_empty(), "token:"),
        (slack.signing_secret.is_empty(), "signing_secret:"),
    ] {
        if missing {
            return raise("ArgumentError", format!("`approvals_in_slack` expects `{what}`: {USAGE}"));
        }
    }
    interp.check_net(HOST, &format!("https://{HOST}/api/chat.postMessage"))?;
    if interp.slack_approvals.borrow().is_some() {
        return raise("ArgumentError", "`approvals_in_slack` is declared once");
    }
    if interp.webhooks.borrow().iter().any(|w| w.path == slack.path) {
        let message = format!("`{}` is already a webhook: give `approvals_in_slack` another `path:`", slack.path);
        return raise("ArgumentError", message);
    }
    if let Some((method, route)) = super::route_matching(interp, &slack.path) {
        return raise("ArgumentError", super::never_reached(&method, &route, &slack.path));
    }
    *interp.slack_approvals.borrow_mut() = Some(slack);
    Ok(Value::Nil)
}
