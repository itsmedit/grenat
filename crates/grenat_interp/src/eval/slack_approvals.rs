//! Approvals decided in Slack (`approvals_in_slack`): a question a job
//! stores is posted to the channel with Approve and Deny buttons; a click
//! comes back on the interactivity route, signed by Slack, and decides it
//! as `Approvals.approve` and `deny` do — recorded as decided by
//! `slack:<user ID>` — then the message is updated: the decision, who took
//! it, no more buttons.
//!
//! - Only a request Slack signed with the signing secret, within five
//!   minutes and never seen before, is read (401 otherwise).
//! - Only the users of `allowed:`, when given, decide: anyone else is told
//!   so, privately (`chat.postEphemeral`), and the refusal is an event.
//! - A click on an approval already decided (in the console, by another
//!   click) changes nothing: the message is updated to the decision taken.
//! - Slack waits three seconds for an answer: the decision is taken, then
//!   the message updated (within two seconds), then the click answered.
//!
//! Posting never fails the job that asks, nor a click once decided: a
//! failed call is a line on standard error and a `slack` event, and the
//! approval can still be decided in the console.

use std::time::Duration;

use serde_json::json;

use grenat_ops::approvals::{self, Approval};

use crate::RuntimeError;
use crate::builtins::{HttpAnswer, RawRequest, query_params};
use crate::eval::slack::TIMEOUT;
use crate::prelude::*;
use crate::slack::click::{self, Click};
use crate::slack::{SlackApprovals, blocks};

/// How long updating a message may take while Slack waits for the answer.
const WHILE_SLACK_WAITS: Duration = Duration::from_secs(2);

/// What a click did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClickOutcome {
    Approved,
    Denied,
    /// Decided before: nothing changed.
    AlreadyDecided,
    /// The user is not in `allowed:`.
    NotAllowed,
    /// No such approval.
    Unknown,
    /// Not signed by Slack, stale or replayed (401), or malformed (400).
    Refused,
    /// Another interaction than a click on an approval's button.
    Ignored,
}

impl ClickOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ClickOutcome::Approved => "approved",
            ClickOutcome::Denied => "denied",
            ClickOutcome::AlreadyDecided => "already_decided",
            ClickOutcome::NotAllowed => "not_allowed",
            ClickOutcome::Unknown => "unknown",
            ClickOutcome::Refused => "refused",
            ClickOutcome::Ignored => "ignored",
        }
    }
}

fn slack_error(message: String) -> RuntimeError {
    RuntimeError { ty: "SlackError".into(), message, span: None, trace: Vec::new() }
}

impl<'p> Interp<'p> {
    /// The declaration of `approvals_in_slack`, if any.
    fn slack_approvals(&self) -> Option<SlackApprovals> {
        self.slack_approvals.borrow().clone()
    }

    /// Posts approval `id`'s question to the channel, if approvals are
    /// decided in Slack. A failure is reported, never raised.
    pub(crate) fn post_approval(&mut self, id: i64, job: i64, message: &str) {
        let Some(slack) = self.slack_approvals() else { return };
        let (text, blocks) = blocks::request(id, job, message);
        let body = json!({"channel": slack.channel, "text": text, "blocks": blocks, "unfurl_links": false});
        if let Err(ctrl) = self.slack_call("chat.postMessage", &slack.token, body, TIMEOUT) {
            let error = self.runtime_error(ctrl);
            self.slack_failed(&format!("approval {id}"), slack_error(error.message));
        }
    }

    /// A failed call, or a refusal, for whoever runs the server.
    fn slack_failed(&mut self, subject: &str, error: RuntimeError) {
        self.write_err(&format!("[slack] {subject}: {}: {}\n", error.ty, error.message));
        self.record_event("slack", subject, &error);
    }

    /// Whether `raw` is for the interactivity route of `approvals_in_slack`.
    pub(crate) fn is_slack_click(&self, raw: &RawRequest) -> bool {
        self.slack_approvals.borrow().as_ref().is_some_and(|s| s.path == raw.path)
    }

    /// Answers a request on the interactivity route, and says what it did.
    pub(crate) fn handle_slack_click(&mut self, raw: &RawRequest) -> Result<(HttpAnswer<'p>, ClickOutcome), Ctrl<'p>> {
        let Some(slack) = self.slack_approvals() else {
            return Ok((HttpAnswer::text(404, "not found"), ClickOutcome::Ignored));
        };
        if raw.method != "POST" {
            return Ok((HttpAnswer::text(405, "Slack sends clicks as POST requests"), ClickOutcome::Refused));
        }
        if let Err(why) = self.slack_signed(&slack.signing_secret, raw) {
            if self.log {
                self.write_err(&format!("[slack] {}: refused ({why})\n", raw.path));
            }
            return Ok((HttpAnswer::text(401, "unauthorized"), ClickOutcome::Refused));
        }
        let form = query_params(&String::from_utf8_lossy(&raw.body));
        let click = match click::parse(&form) {
            Ok(Some(click)) => click,
            Ok(None) => return Ok((HttpAnswer::text(200, ""), ClickOutcome::Ignored)),
            Err(why) => {
                self.slack_failed(&raw.path, slack_error(format!("a malformed click: {why}")));
                return Ok((HttpAnswer::text(400, "malformed payload"), ClickOutcome::Refused));
            }
        };
        let outcome = self.decide_click(&slack, &click)?;
        Ok((HttpAnswer::text(200, ""), outcome))
    }

    /// Decides the approval clicked, if the user may, then shows the
    /// decision in the message.
    fn decide_click(&mut self, slack: &SlackApprovals, click: &Click) -> Result<ClickOutcome, Ctrl<'p>> {
        let subject = format!("approval {}", click.approval);
        if !slack.allows(&click.user) {
            let why = format!("{} may not decide (not in `allowed:`)", click.user);
            self.slack_failed(&subject, slack_error(why));
            self.tell(slack, click, "You may not decide this approval.");
            return Ok(ClickOutcome::NotAllowed);
        }
        let by = format!("slack:{}", click.user);
        let decided = self.decide_approval(click.approval, click.approve, Some(&by))?;
        let id = click.approval;
        let Some(approval) = self.store("approvals", |db| approvals::find(db, id))? else {
            self.tell(slack, click, "This approval no longer exists.");
            return Ok(ClickOutcome::Unknown);
        };
        self.show_decision(slack, click, &approval);
        Ok(match (decided, click.approve) {
            (true, true) => ClickOutcome::Approved,
            (true, false) => ClickOutcome::Denied,
            (false, _) => ClickOutcome::AlreadyDecided,
        })
    }

    /// Updates the message clicked to say the approval's decision.
    fn show_decision(&mut self, slack: &SlackApprovals, click: &Click, approval: &Approval) {
        let (text, blocks) = blocks::decided(
            approval.id,
            approval.job_id,
            &approval.message,
            approval.decision,
            approval.decided_by.as_deref(),
        );
        let body = json!({"channel": click.channel, "ts": click.message_ts, "text": text, "blocks": blocks});
        if let Err(ctrl) = self.slack_call("chat.update", &slack.token, body, WHILE_SLACK_WAITS) {
            let error = self.runtime_error(ctrl);
            self.slack_failed(&format!("approval {}", approval.id), slack_error(error.message));
        }
    }

    /// Tells the user who clicked, and only them, `text`.
    fn tell(&mut self, slack: &SlackApprovals, click: &Click, text: &str) {
        let body = json!({"channel": click.channel, "user": click.user, "text": text});
        if let Err(ctrl) = self.slack_call("chat.postEphemeral", &slack.token, body, WHILE_SLACK_WAITS) {
            let error = self.runtime_error(ctrl);
            self.slack_failed(&format!("approval {}", click.approval), slack_error(error.message));
        }
    }
}
