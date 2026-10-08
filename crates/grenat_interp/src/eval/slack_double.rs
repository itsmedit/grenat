//! A click in Slack, in tests: `deliver_slack_click(id, action: :approve,
//! user: "U0123ABC")` sends the interactivity request Slack would send for
//! a click on approval `id`'s button — a `block_actions` payload, signed
//! with the declared signing secret — through the same route as served,
//! and says what it did: `{"status" => 200, "outcome" => "approved"}`
//! (`denied`, `already_decided`, `not_allowed`, `unknown`). The Web API
//! calls that follow (`chat.update`, `chat.postEphemeral`) are requests as
//! any other: `mock_http` answers them, `Http.requests` lists them.

use serde_json::json;

use crate::builtins::RawRequest;
use crate::prelude::*;
use crate::slack::{blocks, is_id};

const USAGE: &str = "deliver_slack_click(approval_id, action: :approve, user: \"U0123ABC\")";

/// Distinguishes the clicks of a test, as Slack's own trigger IDs do.
static CLICKS: AtomicUsize = AtomicUsize::new(0);

impl<'p> Interp<'p> {
    /// `deliver_slack_click(id, action:, user:)`.
    pub(crate) fn deliver_slack_click(&mut self, args: &Args<'p>) -> R<'p> {
        self.only_in_tests("deliver_slack_click", USAGE)?;
        let Some(slack) = self.slack_approvals.borrow().clone() else {
            return raise("ArgumentError", "no approvals in Slack: declare `approvals_in_slack channel: …` first");
        };
        let approval = match args.pos.as_slice() {
            [id] => match id.untainted() {
                Value::Int(id) => *id,
                other => {
                    let message = format!("`deliver_slack_click` expects an approval's id, got {}", other.type_name());
                    return raise("TypeError", message);
                }
            },
            _ => return raise("ArgumentError", format!("`deliver_slack_click` expects an approval's id: `{USAGE}`")),
        };
        let (mut action, mut user) = (None, None);
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("action", Value::Symbol(a)) if &**a == "approve" => action = Some(blocks::APPROVE),
                ("action", Value::Symbol(a)) if &**a == "deny" => action = Some(blocks::DENY),
                ("user", Value::Str(u)) if is_id(u) => user = Some(u.to_string()),
                (option, v) => {
                    let message = format!(
                        "invalid `deliver_slack_click` option `{option}: {}` (it takes `action: :approve` or `:deny`, `user:` a Slack user ID)",
                        v.inspect()
                    );
                    return raise("ArgumentError", message);
                }
            }
        }
        let (Some(action), Some(user)) = (action, user) else {
            return raise("ArgumentError", format!("`deliver_slack_click` expects `action:` and `user:`: `{USAGE}`"));
        };
        let n = CLICKS.fetch_add(1, AtomicOrdering::Relaxed);
        let payload = json!({
            "type": "block_actions",
            "user": {"id": user, "username": user.to_lowercase(), "team_id": "T0TEST"},
            "api_app_id": "A0TEST",
            "container": {"type": "message", "message_ts": "1700000000.000100", "channel_id": "C0TEST"},
            "trigger_id": format!("{n}.0.test"),
            "channel": {"id": "C0TEST", "name": slack.channel.trim_start_matches('#')},
            "message": {"ts": "1700000000.000100"},
            "actions": [{
                "action_id": action,
                "block_id": "grenat_approval",
                "value": approval.to_string(),
                "type": "button",
                "action_ts": format!("1700000000.{n:06}"),
            }],
        });
        let body = crate::http::form(&[("payload".into(), payload.to_string())]).into_bytes();
        let mut headers = self.slack_headers(&slack.signing_secret, &body);
        headers.push(("Content-Type".into(), "application/x-www-form-urlencoded".into()));
        let raw = RawRequest { method: "POST".into(), path: slack.path, query: String::new(), headers, body };
        let (answer, outcome) = self.handle_slack_click(&raw)?;
        let pairs = vec![
            (Value::str("status"), Value::Int(i64::from(answer.status))),
            (Value::str("outcome"), Value::str(outcome.as_str())),
        ];
        Ok(Value::Hash(Arc::new(Mutex::new(pairs))))
    }
}
