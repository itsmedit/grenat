//! Slack and Teams, checked: webhooks they sign (`signature: :slack`,
//! `:teams`), `approvals_in_slack` and `Slack.post` held to
//! `net("slack.com")` with nothing untrusted, what Slack answers untrusted,
//! and `deliver_slack_click`, a test double (E0500 elsewhere).

mod common;

use common::*;

/// Declared with literal secrets: no `env` effect beside `net`.
const LITERAL: &str = "approvals_in_slack channel: \"#approvals\", token: \"xoxb\", signing_secret: \"s\"\n";

const DECLARED: &str = "approvals_in_slack channel: \"#approvals\", token: Credentials.fetch(:slack, :bot_token), \
signing_secret: Credentials.fetch(:slack, :signing_secret), allowed: [\"U123\"]\n";

#[test]
fn slack_and_teams_webhooks_and_approvals() {
    clean(&format!(
        "{DECLARED}on_webhook \"/slack/events\", signature: :slack, secret: Credentials.fetch(:slack, :signing_secret) do |req|
  nil
end
on_webhook \"/teams\", signature: :teams, secret: Credentials.fetch(:teams, :security_token) do |req|
  {{type: \"message\", text: \"On it.\"}}
end
def done(version: String) uses net(\"slack.com\")
  Slack.post(\"#ops\", \"Deploy of #{{version}} done\")
end
workflow deploy(version: String) uses human, net(\"slack.com\")
  step(:ask) {{ approve! \"Deploy #{{version}}?\" }}
  step(:done) {{ done(version) }}
end
test \"a click\" do
  r = deliver_slack_click(1, action: :approve, user: \"U123\")
  assert_equal \"approved\", r[\"outcome\"]
end
"
    ));
    clean("approvals_in_slack channel: \"C0123\", token: \"xoxb\", signing_secret: \"s\", path: \"/slack/clicks\"\n");
}

#[test]
fn a_webhooks_proof_is_checked() {
    let d = single("on_webhook \"/x\", signature: :gitlab, secret: \"s\" do |r|\nend\n", "E0200", ":gitlab");
    assert!(d.message.contains("`:github`, `:slack`, `:teams`"), "{}", d.message);
    single("on_webhook \"/x\", secret: \"s\" do |r|\nend\n", "E0200", "on_webhook \"/x\", secret: \"s\" do |r|\nend");
    single("on_webhook \"/x\", signature: :slack, secret: 42 do |r|\nend\n", "E0200", "42");
    single("on_webhook \"/x\", token: \"t\", every: 1 do |r|\nend\n", "E0200", "1");
    single("on_webhook \"x\" do |r|\nend\n", "E0200", "\"x\"");
    single(
        &format!("{PRELUDE}on_webhook \"/x\", signature: :slack, secret: summarize(\"x\").title do |r|\nend\n"),
        "E0412",
        "summarize(\"x\").title",
    );
}

#[test]
fn approvals_in_slack_takes_its_options() {
    let d = single(
        "approvals_in_slack channel: \"#a\", token: \"x\"\n",
        "E0200",
        "approvals_in_slack channel: \"#a\", token: \"x\"",
    );
    assert!(d.message.contains("`signing_secret:`"), "{}", d.message);
    let all = "channel: \"#a\", token: \"x\", signing_secret: \"s\"";
    single(&format!("approvals_in_slack {all}, room: \"r\"\n"), "E0200", "\"r\"");
    single(&format!("approvals_in_slack {all}, allowed: \"U1\"\n"), "E0200", "\"U1\"");
    single(&format!("approvals_in_slack {all}, path: \"clicks\"\n"), "E0200", "\"clicks\"");
    single("approvals_in_slack channel: \" \", token: \"x\", signing_secret: \"s\"\n", "E0200", "\" \"");
    single(
        "approvals_in_slack channel: Credentials.fetch(:a, :b), token: \"x\", signing_secret: \"s\"\n",
        "E0414",
        "Credentials.fetch(:a, :b)",
    );
    single(
        &format!("{PRELUDE}approvals_in_slack channel: summarize(\"x\").title, token: \"x\", signing_secret: \"s\"\n"),
        "E0412",
        "summarize(\"x\").title",
    );
}

#[test]
fn slack_is_net_on_slack_com() {
    let approvals = format!("def setup uses net(\"api.github.com\")\n  {LITERAL}end\n");
    let d = single(&approvals, "E0300", LITERAL.trim_end());
    assert_eq!(d.help.as_deref(), Some("add `net(\"slack.com\")` to `uses`"));
    clean(&format!("def setup uses net(\"slack.com\"), env(\"slack\")\n  {DECLARED}end\n"));
    single(
        "def done uses net(\"api.github.com\")\n  Slack.post(\"#ops\", \"done\")\nend\n",
        "E0300",
        "Slack.post(\"#ops\", \"done\")",
    );
    clean("def done uses net\n  Slack.post(\"#ops\", \"done\")\nend\n");
}

#[test]
fn slack_post_posts_nothing_untrusted_and_its_answer_is_untrusted() {
    single(&format!("{PRELUDE}Slack.post(\"#ops\", summarize(\"x\").title)\n"), "E0412", "summarize(\"x\").title");
    clean(&format!(
        "{PRELUDE}def announce(s: Summary) uses net(\"slack.com\")\n  title = s.title.check {{ |t| t.size < 200 }}?\n  Slack.post(\"#ops\", title)\nend\n"
    ));
    single("Slack.post(\"#ops\", Credentials.fetch(:a, :b))\n", "E0414", "Credentials.fetch(:a, :b)");
    single("Slack.post(\"#ops\")\n", "E0200", "Slack.post(\"#ops\")");
    single("Slack.post(\"#ops\", 42)\n", "E0200", "42");
    single("Slack.post(\"#ops\", \"x\", thread: \"1.2\")\n", "E0200", "\"1.2\"");
    clean("Slack.post(\"#ops\", \"x\", token: Credentials.fetch(:slack, :ops_token))\n");
    let src = "ts = Slack.post(\"#ops\", \"x\")\nShell.run([\"echo\", ts])\n";
    single(src, "E0412", "[\"echo\", ts]");
}

#[test]
fn deliver_slack_click_exists_in_tests_only() {
    let d = single(
        &format!("{DECLARED}deliver_slack_click(1, action: :approve, user: \"U1\")\n"),
        "E0500",
        "deliver_slack_click(1, action: :approve, user: \"U1\")",
    );
    assert!(d.message.contains("inside a test"), "{}", d.message);
    let in_test = |call: &str| format!("{DECLARED}test \"t\" do\n  {call}\nend\n");
    single(&in_test("deliver_slack_click(\"1\", action: :approve, user: \"U1\")"), "E0200", "\"1\"");
    single(&in_test("deliver_slack_click(1, action: :maybe, user: \"U1\")"), "E0200", ":maybe");
    single(&in_test("deliver_slack_click(1, action: :deny, who: \"U1\")"), "E0200", "\"U1\"");
    single(
        &in_test("deliver_slack_click(1, action: :deny, user: Credentials.fetch(:a, :b))"),
        "E0414",
        "Credentials.fetch(:a, :b)",
    );
}
