//! Time, checked: `Time.parse`, `Time.iso`, `Time.date`, `Time.weekday`,
//! `Time.at` (pure, typed), durations added to instants, and `freeze_time`
//! in tests only.

mod common;

use common::*;

#[test]
fn the_time_functions_are_typed_and_pure() {
    clean(
        "\
def parsed(text: String) -> Float = Time.parse(text)
def stamp(t: Float) -> String = Time.iso(t)
def day(t: Float) -> String = Time.date(t)
def weekday(t: Float) -> Int = Time.weekday(t)
def start -> Float = Time.at(2026, 9, 28)
def precise -> Float = Time.at(2026, 9, 28, 8, 30, 15.5)
def from_epoch -> String = Time.iso(0)
def now -> Float uses time = Time.now
",
    );
}

#[test]
fn their_results_have_their_types() {
    let d = single("def f(t: Float) -> Int = Time.iso(t)\n", "E0200", "Time.iso(t)");
    assert!(d.message.contains("returns `String`"), "{}", d.message);
    let d = single("def f -> String = Time.parse(\"2026-09-28\")\n", "E0200", "Time.parse(\"2026-09-28\")");
    assert!(d.message.contains("returns `Float`"), "{}", d.message);
}

#[test]
fn their_arguments_are_checked() {
    for (call, at, says) in [
        ("Time.parse(12)", "12", "`Time.parse` expects `String`, got `Int`"),
        ("Time.parse", "Time.parse", "`Time.parse` expects a text"),
        ("Time.iso(\"2026\")", "\"2026\"", "`Time.iso` expects `Float`, got `String`"),
        ("Time.weekday(1.day)", "1.day", "`Time.weekday` expects `Float`, got `Duration`"),
        ("Time.date(1, 2)", "Time.date(1, 2)", "an instant"),
        ("Time.at(2026, 9)", "Time.at(2026, 9)", "`Time.at(year, month, day"),
        ("Time.at(2026, 9, \"28\")", "\"28\"", "expects `Int`, got `String`"),
        ("Time.at(2026, 9, 28, 0, 0, 0, 0)", "Time.at(2026, 9, 28, 0, 0, 0, 0)", "`Time.at(year"),
        ("Time.at(year: 2026, month: 9, day: 28)", "2026", "takes no named argument"),
        ("Time.now(1)", "Time.now(1)", "no argument"),
    ] {
        let src = format!("def f uses time\n  {call}\nend\n");
        let d = single(&src, "E0200", at);
        assert!(d.message.contains(says), "{call}: {}", d.message);
    }
}

#[test]
fn a_secret_is_no_time_text() {
    let d = single(
        "def f uses env\n  Time.parse(Credentials.fetch(:api, :since))\nend\n",
        "E0200",
        "Credentials.fetch(:api, :since)",
    );
    assert!(d.message.contains("got `Secret`"), "{}", d.message);
}

#[test]
fn what_comes_from_untrusted_text_is_untrusted() {
    let src = format!(
        "{PRELUDE}s = summarize(\"x\")\nt = Time.parse(s.title)\nsend(\"a@b.c\", Time.iso(t + 1.day))\nsend(\"a@b.c\", Time.date(t.trust!))\n"
    );
    let d = single(&src, "E0412", "Time.iso(t + 1.day)");
    assert_eq!(&src[d.notes[0].0.range()], "summarize(\"x\")");
}

#[test]
fn durations_move_instants() {
    clean(
        "\
def later(t: Float) -> Float = t + 7.days
def earlier(t: Int) -> Float = t - 90.min
def after(t: Float) -> Float = 1.h + t
def span -> Duration = 2.h - 30.min
def twice -> Duration = 2 * 1.day
def thrice -> Duration = 1.day * 3
def seconds -> Int = 7.days.to_i
def longer -> Bool = 3.days > 2.days
",
    );
    for (body, op) in [("1.day * 1.5", "*"), ("1.day - 5", "-"), ("1.day * 1.day", "*"), ("1.day / 2", "/")] {
        let src = format!("def f = {body}\n");
        let d = single(&src, "E0200", body);
        assert!(d.message.contains(&format!("operator `{op}` is not defined")), "{body}: {}", d.message);
    }
    let d = single("def f(t: Float) -> Duration = t + 1.day\n", "E0200", "t + 1.day");
    assert!(d.message.contains("Float"), "{}", d.message);
}

#[test]
fn freeze_time_is_for_tests() {
    clean(
        "\
def deadline -> String uses time = Time.date(Time.now + 7.days)
test \"a week later\" do
  freeze_time(\"2026-09-28T08:00:00Z\") do
    assert_equal \"2026-10-05\", deadline
    freeze_time(0) do
      assert_equal \"1970-01-01\", Time.today
    end
  end
  day = freeze_time(1790582400.0) { Time.today }
  assert_equal \"2026-09-28\", day.upcase
end
",
    );
    for src in [
        "freeze_time(\"2026-09-28\") do\nend\n",
        "def helper uses time\n  freeze_time(\"2026-09-28\") do\n    Time.today\n  end\nend\n",
    ] {
        let d = diags(src);
        assert_eq!(d.len(), 1, "{}", render(src, &d));
        assert_eq!(d[0].code, Some("E0500"));
        assert!(src[d[0].span.range()].starts_with("freeze_time(\"2026-09-28\") do"), "{}", render(src, &d));
        assert!(d[0].message.contains("only be used inside a test"), "{}", d[0].message);
    }
}

#[test]
fn freeze_time_takes_an_instant_and_a_block() {
    let wrapped = |body: &str| format!("test \"t\" do\n  {body}\nend\n");
    let d = single(&wrapped("freeze_time([1]) do\n  end"), "E0200", "[1]");
    assert!(d.message.contains("expects an instant"), "{}", d.message);
    for body in ["freeze_time(\"2026-09-28\")", "freeze_time do\n  end"] {
        let src = wrapped(body);
        let d = diags(&src);
        assert!(
            d.iter().any(|d| d.code == Some("E0200") && d.message.contains("an instant and a block")),
            "{body}:\n{}",
            render(&src, &d)
        );
    }
}
