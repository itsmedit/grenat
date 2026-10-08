//! Rate limits, checked: `limit net("host"), rate, concurrent: n` with a
//! host and nothing more, a `Rate` (`10.per_second`, `100.per(15.minutes)`)
//! allowing one call at least, a positive `concurrent:`, nothing untrusted
//! or secret, and one declaration per host — as the interpreter refuses
//! them when the program runs.

mod common;

use common::*;

#[test]
fn declarations_and_rates() {
    clean(
        "limit net(\"api.hubspot.com\"), 10.per_second
limit net(\"api.github.com\"), 5000.per_hour, concurrent: 4
limit net(\"api.anthropic.com\"), 50.per_minute
limit net(\"smtp.example.com\"), 1000.per_day
limit net(\"crm.example.com\"), 100.per(15.minutes)
limit net(\"db.example.com\"), concurrent: 2
r = 10.per_second
n = r.count + 1
d = r.period
limit net(\"api.x.io\"), r
host = \"api.y.io\"
limit net(host), 3.per_second
test \"a test's own limit\" do
  limit net(\"api.hubspot.com\"), 1.per_second
end
",
    );
}

#[test]
fn a_host_and_nothing_more() {
    let d = single("limit net(\"https://api.x.io/v1\"), 10.per_second\n", "E0200", "\"https://api.x.io/v1\"");
    assert!(d.message.contains("expects a host"), "{}", d.message);
    single("limit net(\"api.x.io:443\"), 10.per_second\n", "E0200", "\"api.x.io:443\"");
    single("limit net(\"user@api.x.io\"), 10.per_second\n", "E0200", "\"user@api.x.io\"");
    single("limit net(42), 10.per_second\n", "E0200", "42");
    let d = single("limit \"api.x.io\", 10.per_second\n", "E0200", "limit \"api.x.io\", 10.per_second");
    assert!(d.message.contains("expects a host first"), "{}", d.message);
}

#[test]
fn a_rate_or_a_concurrency_limit() {
    let d = single("limit net(\"a.io\"), 0.per_second\n", "E0200", "0.per_second");
    assert!(d.message.contains("at least one call"), "{}", d.message);
    let d = single("limit net(\"a.io\"), 10\n", "E0200", "10");
    assert!(d.message.contains("expects a rate"), "{}", d.message);
    let d = single("limit net(\"a.io\")\n", "E0200", "limit net(\"a.io\")");
    assert!(d.message.contains("a rate, `concurrent:`, or both"), "{}", d.message);
    single(
        "limit net(\"a.io\"), 10.per_second, 3.per_minute\n",
        "E0200",
        "limit net(\"a.io\"), 10.per_second, 3.per_minute",
    );
    single("limit net(\"a.io\"), 10.per_second, concurrent: 0\n", "E0200", "0");
    single("limit net(\"a.io\"), concurrent: 2.5\n", "E0200", "2.5");
    single("limit net(\"a.io\"), concurrent: \"4\"\n", "E0200", "\"4\"");
    let d = single("limit net(\"a.io\"), 10.per_second, burst: 3\n", "E0200", "3");
    assert!(d.message.contains("no option `burst:`"), "{}", d.message);
    // `per` takes a duration
    single("r = 10.per(3)\n", "E0200", "per");
}

#[test]
fn a_host_is_limited_once() {
    let d = single("limit net(\"a.io\"), 1.per_second\nlimit net(\"A.IO\"), concurrent: 2\n", "E0200", "\"A.IO\"");
    assert!(d.message.contains("already limited"), "{}", d.message);
    clean("limit net(\"a.io\"), 1.per_second\nlimit net(\"b.io\"), 1.per_second\n");
}

#[test]
fn nothing_untrusted_or_secret() {
    single(&format!("{PRELUDE}limit net(summarize(\"x\").title), 10.per_second\n"), "E0412", "summarize(\"x\").title");
    single("limit net(Credentials.fetch(:crm, :host)), 10.per_second\n", "E0414", "Credentials.fetch(:crm, :host)");
}
