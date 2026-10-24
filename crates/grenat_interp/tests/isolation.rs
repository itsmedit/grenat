//! `grenat test` keeps tests apart: each one starts from the state the
//! script left — its own database (and operations store), journals,
//! doubles, deliveries, requests, rate-limit buckets — and what a test
//! declares or starts goes with it. Each case runs a test that leaves
//! state behind between two that must see none: the second starts clean,
//! and the dirty one, run after a clean one, sees only its own.

mod common;

use common::*;

/// `program`, then a clean test, the dirty one, and the clean one again.
fn apart(program: &str, dirty: &str, clean: &str) {
    let src = format!(
        "{program}\ntest \"clean, before\" do\n{clean}\nend\ntest \"dirty\" do\n{dirty}\nend\ntest \"clean, after\" do\n{clean}\nend\n"
    );
    tests_pass(&src);
}

const REFUND: &str = "\
workflow refund(order: Int) uses human
  step(:ask) { approve! \"Refund #{order}?\" }
end
";

const NOTES: &str = "\
struct Note
  table :notes
  id: Int?
  text: String
end
migration \"001_notes\" do |db|
  db.migrate(\"CREATE TABLE notes (id #{db.primary_key}, text TEXT NOT NULL)\")
end
";

// ── The database and the operations store ─────────────────────────────

#[test]
fn the_same_workflow_enqueued_by_two_tests_waits_once_in_each() {
    let same = "  enqueue(:refund, 42)\n  Jobs.perform\n  assert_equal 1, Approvals.pending.size";
    apart(REFUND, same, same);
}

#[test]
fn approvals_start_clean_without_migrations() {
    let dirty = "  enqueue(:refund, 42)\n  Jobs.perform\n  assert_equal 1, Approvals.pending.size";
    apart(REFUND, dirty, "  assert_equal [], Approvals.pending");
}

#[test]
fn approvals_start_clean_with_a_database_declared() {
    let program = format!("database \"sqlite://app.db\"\n{REFUND}");
    let dirty = "  enqueue(:refund, 42)\n  Jobs.perform\n  assert_equal 1, Approvals.pending.size";
    apart(&program, dirty, "  assert_equal [], Approvals.pending");
}

#[test]
fn approvals_start_clean_with_migrations() {
    let program = format!("{NOTES}{REFUND}");
    let dirty = "  enqueue(:refund, 42)\n  Jobs.perform\n  assert_equal 1, Approvals.pending.size";
    apart(&program, dirty, "  assert_equal [], Approvals.pending");
}

#[test]
fn jobs_start_clean() {
    let program = "def boom(n: Int)\n  raise ArgumentError, \"boom #{n}\"\nend\n";
    let dirty = "  enqueue(:boom, 1)\n  enqueue(:boom, 2)\n  assert_equal 2, Jobs.enqueued.size\n  Jobs.perform\n  assert_equal 2, Jobs.failed.size";
    apart(program, dirty, "  assert_equal [], Jobs.enqueued\n  assert_equal [], Jobs.failed");
}

#[test]
fn records_start_clean() {
    let dirty = "  Note.create(text: \"a\")\n  assert_equal 1, Note.count";
    apart(NOTES, dirty, "  assert_equal 0, Note.count");
}

// ── Journals ───────────────────────────────────────────────────────────

#[test]
fn no_test_resumes_a_workflow_another_completed() {
    let approved = "  with_human(approve_all) { refund(42) }";
    let denied = "  with_human(deny_all) { assert_raises(ApprovalDenied) { refund(42) } }";
    apart(REFUND, approved, denied);
    apart(REFUND, denied, approved);
    let program = format!("database \"sqlite://app.db\"\n{NOTES}{REFUND}");
    apart(&program, approved, denied);
    apart(&program, denied, approved);
}

// ── Doubles ────────────────────────────────────────────────────────────

#[test]
fn a_model_mock_answers_only_its_test() {
    let dirty = "  mock :fast, replies: [{title: \"Cat\", bullets: [], sentiment: \"Positive\"}]\n  assert_equal \"Cat\", summarize(\"x\").title.trust!";
    let clean = "  e = assert_raises(LlmError) { summarize(\"x\") }\n  assert e.message.include?(\"mock\")";
    apart(SUMMARY, dirty, clean);
}

#[test]
fn http_and_shell_stubs_answer_only_their_test() {
    let program = "def ping -> Int uses net(\"api.acme.io\") = Http.get(\"https://api.acme.io/ping\").status\n";
    let dirty = "  mock_http \"GET https://api.acme.io/*\", status: 204\n  assert_equal 204, ping\n  mock_shell \"date\", stdout: \"today\"\n  assert_equal \"today\", Shell.run([\"date\"]).stdout.trust!";
    let clean = "  assert_raises(HttpError) { ping }\n  assert_raises(ShellError) { Shell.run([\"date\"]) }";
    apart(program, dirty, clean);
}

#[test]
fn env_and_credentials_doubles_answer_only_their_test() {
    let program = "def region -> String uses env = Env.fetch(\"REGION\", \"none\")\n";
    let dirty = "  mock_env({\"REGION\" => \"eu\"})\n  assert_equal \"eu\", region\n  mock_credentials({\"crm\" => {}})\n  assert_equal nil, Credentials.dig(:crm, :token)";
    let clean = "  assert_equal \"none\", region\n  assert Credentials.dig(:crm, :token) != nil";
    apart(program, dirty, clean);
}

#[test]
fn a_failing_mailer_and_deliveries_stay_in_their_test() {
    let program = "def notify uses net(\"smtp.acme.com\")\n  Mail.connect(\"smtps://bot:pw@smtp.acme.com:465\").send(from: \"bot@acme.com\", to: \"ada@acme.com\", subject: \"Hi\", body: \"Hello\")\nend\n";
    let dirty = "  notify\n  assert_equal 1, Mail.deliveries.size\n  mock_mail(raise: \"SMTP down\")\n  assert_raises(MailError) { notify }";
    let clean = "  assert_equal 0, Mail.deliveries.size\n  notify\n  assert_equal 1, Mail.deliveries.size";
    apart(program, dirty, clean);
}

#[test]
fn requests_sent_stay_in_their_test() {
    let program = "def ping -> Int uses net(\"api.acme.io\") = Http.get(\"https://api.acme.io/ping\").status\n";
    let dirty =
        "  mock_http \"GET https://api.acme.io/*\", status: 200\n  ping\n  ping\n  assert_equal 2, Http.requests.size";
    apart(program, dirty, "  assert_equal [], Http.requests");
}

#[test]
fn a_human_double_stays_in_its_test_even_when_it_raises() {
    let program = "def deploy -> String uses human\n  approve! \"deploy?\"\n  \"deployed\"\nend\n";
    let dirty = "  assert_raises(ArgumentError) { with_human(approve_all) { raise ArgumentError, \"out\" } }\n  with_human(approve_all) { assert_equal \"deployed\", deploy }";
    let clean = "  assert_raises(HumanError) { deploy }";
    apart(program, dirty, clean);
}

#[test]
fn a_frozen_clock_stays_in_its_test() {
    let dirty = "  assert_raises(ArgumentError) { freeze_time(0) { raise ArgumentError, \"out\" } }\n  freeze_time(0) { assert_equal \"1970-01-01\", Time.today }";
    apart("", dirty, "  assert Time.now > Time.parse(\"2026-01-01\")");
}

// ── Rate limits ────────────────────────────────────────────────────────

#[test]
fn every_test_starts_with_full_buckets_and_without_another_s_limit() {
    // the script's bucket holds two calls a minute: a test that empties it
    // leaves it full for the next; a limit a test declares goes with it
    let program = "limit net(\"api.acme.io\"), 2.per_minute\n";
    let dirty = "  mock_http \"GET https://*\", status: 200\n  limit net(\"api.other.io\"), 1.per_minute\n  t = Time.now\n  2.times { Http.get(\"https://api.acme.io/a\") }\n  Http.get(\"https://api.other.io/a\")\n  assert Time.now - t < 5";
    let clean = "  mock_http \"GET https://*\", status: 200\n  t = Time.now\n  2.times { Http.get(\"https://api.acme.io/a\") }\n  3.times { Http.get(\"https://api.other.io/a\") }\n  assert Time.now - t < 5";
    apart(program, dirty, clean);
}

// ── What a test declares or starts ─────────────────────────────────────

#[test]
fn an_approval_handler_a_test_declares_goes_with_it() {
    let program = "def deploy -> String uses human\n  approve! \"deploy?\"\n  \"deployed\"\nend\n";
    let dirty = "  Runtime.on_approval { |req| true }\n  assert_equal \"deployed\", deploy";
    apart(program, dirty, "  assert_raises(HumanError) { deploy }");
}

#[test]
fn approvals_in_slack_a_test_declares_go_with_it() {
    let dirty = "  approvals_in_slack channel: \"#approvals\", token: \"xoxb-test\", signing_secret: \"shh\"\n  mock_http \"POST https://slack.com/api/*\", json: {ok: true, ts: \"1700000000.000100\"}\n  enqueue(:refund, 42)\n  Jobs.perform\n  assert_equal 1, Http.requests.size";
    let clean = "  enqueue(:refund, 42)\n  Jobs.perform\n  assert_equal [], Http.requests\n  e = assert_raises(ArgumentError) { deliver_slack_click(Approvals.pending.first[\"id\"], action: :approve, user: \"U123\") }\n  assert e.message.include?(\"no approvals in Slack\")";
    apart(REFUND, dirty, clean);
}

#[test]
fn a_trigger_a_test_declares_goes_with_it() {
    let dirty = "  on_file \"inbox/*.pdf\" do |path|\n    nil\n  end\n  assert_equal \"handled\", deliver_file(\"inbox/a.pdf\")[\"status\"]";
    let clean = "  e = assert_raises(ArgumentError) { deliver_file(\"inbox/a.pdf\") }\n  assert e.message.include?(\"no folder is watched\")";
    apart("", dirty, clean);
}

#[test]
fn the_children_a_supervisor_starts_in_a_test_go_with_it() {
    let program = "\
agent Counter
  @n: Int = 0
  on Incr -> Int
    @n += 1
  end
end
supervisor Desk, strategy: :one_for_one
  child Counter
end
";
    let dirty = "  assert_equal 1, Desk[Counter].ask(Incr())\n  assert_equal 2, Desk[Counter].ask(Incr())";
    apart(program, dirty, "  assert_equal 1, Desk[Counter].ask(Incr())");
}
