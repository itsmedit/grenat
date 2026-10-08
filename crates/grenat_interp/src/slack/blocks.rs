//! The messages of an approval, in Block Kit: the question with Approve and
//! Deny buttons (each carrying the approval's id), then, once decided, the
//! question with the decision and who took it, without buttons.
//!
//! The question is shown as text: `&`, `<` and `>` are escaped, so nothing
//! in it — a model's draft quoted in a question — can mention `<!channel>`,
//! a user, or hide a link. A section's text is at most 3000 characters:
//! a longer question is cut.

use serde_json::{Value as Json, json};

use grenat_ops::approvals::Decision;

/// The `action_id`s of the two buttons.
pub(crate) const APPROVE: &str = "grenat_approve";
pub(crate) const DENY: &str = "grenat_deny";

/// The block holding the buttons.
const ACTIONS_BLOCK: &str = "grenat_approval";

/// What of a question a message shows.
const MAX_QUESTION: usize = 2800;

/// `text` as Slack shows it, never as markup.
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The question, escaped and cut to fit a section.
fn question(message: &str) -> String {
    let mut shown: String = message.chars().take(MAX_QUESTION).collect();
    if shown.len() < message.len() {
        shown.push('…');
    }
    escape(&shown)
}

fn section(text: String) -> Json {
    json!({"type": "section", "text": {"type": "mrkdwn", "text": text}})
}

fn context(text: String) -> Json {
    json!({"type": "context", "elements": [{"type": "mrkdwn", "text": text}]})
}

fn button(action: &str, label: &str, style: &str, approval: i64) -> Json {
    json!({
        "type": "button",
        "action_id": action,
        "text": {"type": "plain_text", "text": label},
        "style": style,
        "value": approval.to_string(),
    })
}

/// The question of approval `id`, asked by job `job`: the fallback text
/// (notifications) and the blocks.
pub(crate) fn request(id: i64, job: i64, message: &str) -> (String, Json) {
    let question = question(message);
    let blocks = json!([
        section(format!("*Approval needed*\n{question}")),
        context(format!("Job {job} · approval {id}")),
        {
            "type": "actions",
            "block_id": ACTIONS_BLOCK,
            "elements": [button(APPROVE, "Approve", "primary", id), button(DENY, "Deny", "danger", id)],
        },
    ]);
    (format!("Approval needed: {question}"), blocks)
}

/// Who decided, as the message says it: a Slack user mentioned, the
/// console, or the program itself.
fn by_whom(decided_by: Option<&str>) -> String {
    match decided_by {
        Some(by) => match by.strip_prefix("slack:").filter(|id| super::is_id(id)) {
            Some(user) => format!("by <@{user}>"),
            None if by == "console" => "in the console".into(),
            None => format!("by {}", escape(by)),
        },
        None => "by the program".into(),
    }
}

/// Approval `id`'s message once decided: the fallback text and the blocks.
pub(crate) fn decided(
    id: i64,
    job: i64,
    message: &str,
    decision: Decision,
    decided_by: Option<&str>,
) -> (String, Json) {
    let question = question(message);
    let outcome = match decision {
        Decision::Approved => format!("Approved {}", by_whom(decided_by)),
        Decision::Denied => format!("Denied {}", by_whom(decided_by)),
        Decision::Pending => "Waiting for a decision".into(),
    };
    let blocks = json!([
        section(format!("*Approval*\n{question}")),
        context(format!("*{outcome}* · job {job} · approval {id}")),
    ]);
    (format!("{outcome}: {question}"), blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_has_two_buttons_carrying_its_id() {
        let (text, blocks) = request(5, 12, "Deploy <b>v2</b> & migrate?");
        assert_eq!(text, "Approval needed: Deploy &lt;b&gt;v2&lt;/b&gt; &amp; migrate?");
        assert_eq!(blocks[0]["text"]["text"], "*Approval needed*\nDeploy &lt;b&gt;v2&lt;/b&gt; &amp; migrate?");
        assert_eq!(blocks[1]["elements"][0]["text"], "Job 12 · approval 5");
        let buttons = blocks[2]["elements"].as_array().unwrap();
        assert_eq!(blocks[2]["type"], "actions");
        assert_eq!((buttons[0]["action_id"].as_str(), buttons[0]["value"].as_str()), (Some(APPROVE), Some("5")));
        assert_eq!((buttons[1]["action_id"].as_str(), buttons[1]["style"].as_str()), (Some(DENY), Some("danger")));
        assert_eq!(buttons[0]["text"], json!({"type": "plain_text", "text": "Approve"}));
    }

    #[test]
    fn nothing_in_a_question_mentions_or_links() {
        let (text, _) = request(1, 1, "<!channel> see <https://evil.example|docs> <@U1>");
        assert!(!text.contains('<') && !text.contains('>'), "{text}");
    }

    #[test]
    fn a_long_question_is_cut() {
        let (_, blocks) = request(1, 1, &"é".repeat(5000));
        let shown = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(shown.chars().count() < 3000 && shown.ends_with('…'));
    }

    #[test]
    fn a_decided_message_says_who_decided_and_has_no_buttons() {
        let (text, blocks) = decided(5, 12, "Deploy?", Decision::Approved, Some("slack:U123"));
        assert_eq!(text, "Approved by <@U123>: Deploy?");
        assert_eq!(blocks.as_array().unwrap().len(), 2);
        assert_eq!(blocks[1]["elements"][0]["text"], "*Approved by <@U123>* · job 12 · approval 5");
        assert_eq!(decided(5, 12, "x", Decision::Denied, Some("console")).0, "Denied in the console: x");
        assert_eq!(decided(5, 12, "x", Decision::Approved, None).0, "Approved by the program: x");
        // a user ID is checked before it is mentioned
        assert_eq!(decided(5, 12, "x", Decision::Denied, Some("slack:<!here>")).0, "Denied by slack:&lt;!here&gt;: x");
    }
}
