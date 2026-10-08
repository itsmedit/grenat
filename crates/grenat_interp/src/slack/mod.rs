//! Slack as the runtime speaks it: its Web API ([`api`]: `chat.postMessage`,
//! `chat.update`, `chat.postEphemeral`), the Block Kit messages of an
//! approval ([`blocks`]), and the clicks on their buttons ([`click`]: the
//! interactivity payload). Nothing here knows the language's values; the
//! interpreter's side is `eval::slack` (`Slack.post`) and
//! `eval::slack_approvals` (`approvals_in_slack`).

pub(crate) mod api;
pub(crate) mod blocks;
pub(crate) mod click;

/// The host every Web API method is on: `net("slack.com")`.
pub(crate) const HOST: &str = "slack.com";

/// Where the Web API is, unless `Options::slack_api` says otherwise (tests).
pub(crate) const API: &str = "https://slack.com/api";

/// The interactivity route of `approvals_in_slack`, unless `path:` says otherwise.
pub(crate) const CLICKS_PATH: &str = "/slack/interactivity";

/// What `approvals_in_slack` declared: where questions go, how to post
/// them, how to know a click comes from Slack, and who may decide. The
/// token and the signing secret are revealed here, and only ever reach a
/// request's header or a signature check.
#[derive(Clone)]
pub(crate) struct SlackApprovals {
    /// A channel's name (`#approvals`) or ID (`C0123`).
    pub channel: String,
    pub token: String,
    pub signing_secret: String,
    /// The Slack user IDs who may decide; empty: anyone in the channel.
    pub allowed: Vec<String>,
    /// The route Slack sends clicks to (the app's Interactivity Request URL).
    pub path: String,
}

impl SlackApprovals {
    /// Whether Slack user `user` may decide.
    pub(crate) fn allows(&self, user: &str) -> bool {
        self.allowed.is_empty() || self.allowed.iter().any(|a| a == user)
    }
}

/// Whether `id` looks like a Slack ID (`U0123ABC`, `C0123`, `W012`): what
/// is put between `<@` and `>` must be nothing else.
pub(crate) fn is_id(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_who_may_decide() {
        assert!(is_id("U0123ABC") && is_id("C1"));
        assert!(!is_id("") && !is_id("u123") && !is_id("U1>") && !is_id("!channel") && !is_id(&"U".repeat(33)));
        let mut approvals = SlackApprovals {
            channel: "#approvals".into(),
            token: "xoxb".into(),
            signing_secret: "s".into(),
            allowed: Vec::new(),
            path: CLICKS_PATH.into(),
        };
        assert!(approvals.allows("U999"));
        approvals.allowed = vec!["U123".into()];
        assert!(approvals.allows("U123") && !approvals.allows("U999"));
    }
}
