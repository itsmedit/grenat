//! Time: instants read and written in ISO 8601 (`Time.parse`, `Time.iso`,
//! `Time.date`, `Time.weekday`, `Time.at`), durations added to instants,
//! and `freeze_time` stopping the clock in a test.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, TestOutcome, run_tests};

fn lines(src: &str) -> Vec<String> {
    run(src).lines().map(str::to_string).collect()
}

/// Each test's name and error (`None` when it passed), and what was printed.
fn tests_of(src: &str) -> (Vec<(String, Option<String>)>, String) {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let buffer = Arc::new(Mutex::new(String::new()));
    let options =
        Options { output: Output::Capture(buffer.clone()), journal: Some(temp_dir("journal")), ..Options::default() };
    let outcomes: Vec<TestOutcome> = run_tests(&parsed.program, options).unwrap();
    let results = outcomes
        .iter()
        .map(|o| (o.name.clone(), o.error.as_ref().map(|e| format!("{}: {}", e.ty, e.message))))
        .collect();
    let output = buffer.lock().unwrap().clone();
    (results, output)
}

fn all_pass(src: &str) -> String {
    let (results, output) = tests_of(src);
    for (name, error) in results {
        assert!(error.is_none(), "`{name}` failed: {}\n{output}", error.unwrap());
    }
    output
}

#[test]
fn iso_8601_is_read_with_offsets_and_fractions() {
    let out = lines(
        "\
p Time.parse(\"2026-09-28T08:00:00Z\")
p Time.parse(\"2026-09-28T10:00:00+02:00\")
p Time.parse(\"2026-09-28T03:00:00-05:00\")
p Time.parse(\"2026-09-28\")
p Time.parse(\"2026-09-28T08:00:00.250Z\")
p Time.parse(\"1970-01-01T00:00:00Z\")
p Time.parse(\"1969-12-31T23:59:59Z\")
",
    );
    assert_eq!(out, ["1790582400.0", "1790582400.0", "1790582400.0", "1790553600.0", "1790582400.25", "0.0", "-1.0"]);
}

#[test]
fn instants_are_written_in_utc() {
    let out = lines(
        "\
t = Time.parse(\"2026-12-31T22:30:00-05:00\")
p Time.iso(t)
p Time.date(t)
p Time.iso(0)
p Time.iso(-0.5)
p Time.iso(Time.parse(\"2026-09-28T08:00:00.250Z\"))
p Time.date(-1)
",
    );
    // a negative offset crosses midnight, and the year
    assert_eq!(
        out,
        [
            "\"2027-01-01T03:30:00Z\"",
            "\"2027-01-01\"",
            "\"1970-01-01T00:00:00Z\"",
            "\"1969-12-31T23:59:59.500Z\"",
            "\"2026-09-28T08:00:00.250Z\"",
            "\"1969-12-31\"",
        ]
    );
}

#[test]
fn parse_and_iso_round_trip() {
    let out = run(
        "\
[\"2026-09-28T08:00:00Z\", \"2024-02-29T23:59:59.999Z\", \"1968-05-01T00:00:00.250Z\", \"0000-01-01T00:00:00Z\", \"9999-12-31T23:59:59Z\"].each do |s|
  p Time.iso(Time.parse(s)) == s
end
",
    );
    assert_eq!(out, "true\n".repeat(5));
}

#[test]
fn leap_years_and_weekdays() {
    let out = lines(
        "\
p Time.date(Time.parse(\"2024-02-28\") + 1.day)
p Time.date(Time.parse(\"2026-02-28\") + 1.day)
p Time.date(Time.parse(\"2000-02-28\") + 1.day)
p Time.date(Time.parse(\"1900-02-28\") + 1.day)
p [\"1970-01-01\", \"2026-09-28\", \"2026-10-04\", \"2000-02-29\", \"1969-12-31\"].map { |d| Time.weekday(Time.parse(d)) }
",
    );
    assert_eq!(out, ["\"2024-02-29\"", "\"2026-03-01\"", "\"2000-02-29\"", "\"1900-03-01\"", "[4, 1, 7, 2, 3]"]);
}

#[test]
fn an_instant_is_cut_to_the_millisecond_never_rounded_into_the_next_day() {
    let out = lines(
        "\
t = Time.parse(\"2026-12-31T23:59:59.9996Z\")
p [Time.date(t), Time.iso(t), Time.weekday(t)]
p Time.date(Time.parse(\"2026-12-31\") + 86399.9999)
p Time.iso(Time.parse(\"2026-03-29T00:59:59.9996Z\"))
p Time.iso(-0.0004)
",
    );
    assert_eq!(
        out,
        [
            "[\"2026-12-31\", \"2026-12-31T23:59:59.999Z\", 4]",
            "\"2026-12-31\"",
            "\"2026-03-29T00:59:59.999Z\"",
            "\"1969-12-31T23:59:59.999Z\"",
        ]
    );
}

#[test]
fn time_at_builds_an_instant_in_utc() {
    let out = lines(
        "\
p Time.at(2026, 9, 28) == Time.parse(\"2026-09-28\")
p Time.at(2026, 9, 28, 8) == Time.parse(\"2026-09-28T08:00:00Z\")
p Time.iso(Time.at(2024, 2, 29, 23, 59, 59.5))
p Time.at(1970, 1, 1)
",
    );
    assert_eq!(out, ["true", "true", "\"2024-02-29T23:59:59.500Z\"", "0.0"]);
}

#[test]
fn what_is_not_an_instant_is_an_error() {
    for (src, ty, says) in [
        ("Time.parse(\"2026-02-29\")", "ArgumentError", "day 29 is not in 2026-02"),
        ("Time.parse(\"28/09/2026\")", "ArgumentError", "invalid time `28/09/2026`"),
        ("Time.parse(\"2026-09-28T25:00:00Z\")", "ArgumentError", "not a time of day"),
        ("Time.parse(\"2026-09-28T08:00:00+2\")", "ArgumentError", "an offset"),
        ("Time.parse(12)", "TypeError", "expects a string"),
        ("Time.at(2026, 13, 1)", "ArgumentError", "month 13"),
        ("Time.at(2026, 2, 29)", "ArgumentError", "day 29"),
        ("Time.at(2026, 9)", "ArgumentError", "Time.at(year, month, day"),
        ("Time.at(2026, 9, 28, \"8\")", "TypeError", "integers"),
        ("Time.iso(\"now\")", "TypeError", "expects an instant"),
        ("Time.iso(1.0e15)", "ArgumentError", "out of range"),
        ("Time.date(0.0 / 0.0)", "ArgumentError", "not an instant"),
        ("Time.yesterday", "NoMethodError", "`Time.yesterday`"),
    ] {
        let e = run_err(src, Vec::new());
        assert_eq!(e.ty, ty, "{src}: {}", e.message);
        assert!(e.message.contains(says), "{src}: {}", e.message);
    }
}

#[test]
fn durations_move_instants() {
    let out = lines(
        "\
t = Time.parse(\"2026-09-28T08:00:00Z\")
p Time.iso(t + 7.days)
p Time.iso(t - 90.min)
p Time.iso(1.h + t)
p Time.iso(0 + 2.days)
p [7.days.to_i, 90.s.to_f, 1.5.round]
p [2.h - 30.min, 1.day * 2, 3 * 1.h, 1.h + 1.h]
p [1.h < 2.h, 3.days > 2.days, 2.min == 120.s, 60.0 < 2.min, 2.min <= 120.0]
p [2.days, 1.h].sort
p [1.h < 3601, 3600 < 1.h, 1.h <= 3600, 7200 >= 2.h, 1.h <=> 3600, 3600 <=> 1.h, 1.h <=> 3600.0]
p [1.h == 3600, 3600 == 1.h, 1.h == 3600.0, 3600.0 == 1.h, 1.h != 3601, 1.h == 60.min, [1.h].include?(3600)]
",
    );
    assert_eq!(
        out,
        [
            "\"2026-10-05T08:00:00Z\"",
            "\"2026-09-28T06:30:00Z\"",
            "\"2026-09-28T09:00:00Z\"",
            "\"1970-01-03T00:00:00Z\"",
            "[604800, 90.0, 2]",
            "[90min, 2d, 3h, 2h]",
            "[true, true, true, true, true]",
            "[1h, 2d]",
            "[true, false, true, true, 0, 0, 0]",
            "[true, true, true, true, true, true, true]",
        ]
    );
}

#[test]
fn a_duration_is_no_number_where_it_makes_no_sense() {
    for src in ["p 1.day - 5", "p 1.day * 1.5", "p \"a\" + 1.day", "p 1.day * 1.day"] {
        assert_eq!(run_err(src, Vec::new()).ty, "TypeError", "{src}");
    }
}

#[test]
fn what_comes_from_untrusted_text_is_untrusted() {
    let src = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
prompt stamp(x: String) -> ~String using :fast
  user x
end
s = stamp(\"when?\")
t = Time.parse(s)
p [t.tainted?, Time.iso(t).tainted?, Time.date(t).tainted?, Time.weekday(t).tainted?, (t + 1.day).tainted?]
p Time.iso(t.trust!)
p Time.parse(\"2026-09-28\").tainted?
";
    let out = run_with(src, vec![Response::text_reply("2026-09-28T08:00:00Z")], &[]).ok();
    assert_eq!(out, "[true, true, true, true, true]\n\"2026-09-28T08:00:00Z\"\nfalse\n");
}

#[test]
fn a_secret_is_not_read_as_a_time() {
    // in a test, `Credentials.fetch(:api, :since)` is the stand-in secret `"test-api-since"`
    let (results, _) = tests_of("test \"secret\" do\n  Time.parse(Credentials.fetch(:api, :since))\nend\n");
    let error = results[0].1.clone().unwrap();
    assert!(error.starts_with("TypeError: `Time.parse` expects a string, got Secret"), "{error}");
    assert!(!error.contains("test-api-since"), "{error}");
}

#[test]
fn time_now_is_still_the_clock() {
    let out = lines(
        "t = Time.now\np t.is_a?(Float)\np t > Time.parse(\"2026-01-01\")\np Time.today == Time.date(Time.now)\n",
    );
    assert_eq!(out, ["true", "true", "true"]);
}

#[test]
fn freeze_time_stops_the_clock_in_a_test() {
    all_pass(
        "\
test \"frozen\" do
  freeze_time(\"2026-09-28T08:00:00Z\") do
    assert_equal Time.parse(\"2026-09-28T08:00:00Z\"), Time.now
    assert_equal \"2026-09-28\", Time.today
    assert_equal \"2026-10-05\", Time.date(Time.now + 7.days)
  end
end

test \"epoch seconds, and a negative offset crossing midnight\" do
  freeze_time(0) do
    assert_equal \"1970-01-01\", Time.today
  end
  freeze_time(\"2026-09-28T23:30:00-05:00\") do
    assert_equal \"2026-09-29\", Time.today
  end
end

test \"nested, then restored\" do
  freeze_time(\"2026-09-28\") do
    freeze_time(\"2000-02-29T12:00:00Z\") do
      assert_equal \"2000-02-29\", Time.today
    end
    assert_equal \"2026-09-28\", Time.today
  end
  assert Time.now > Time.parse(\"2026-10-01\")
end

test \"restored after a raise\" do
  assert_raises RuntimeError do
    freeze_time(\"2000-01-01\") do
      raise \"boom\"
    end
  end
  assert Time.now > Time.parse(\"2026-10-01\")
end

test \"the block's value\" do
  assert_equal \"2026-09-28\", freeze_time(\"2026-09-28\") { Time.today }
end
",
    );
}

#[test]
fn a_frozen_clock_ends_with_its_test() {
    let (results, output) = tests_of(
        "\
test \"stops the clock and fails inside\" do
  freeze_time(\"2000-01-01\") do
    assert false, \"failed while frozen\"
  end
end

test \"sees the real clock\" do
  puts Time.today == \"2000-01-01\"
  assert Time.now > Time.parse(\"2026-10-01\")
end
",
    );
    assert_eq!(results[0].1.as_deref(), Some("AssertionError: failed while frozen"));
    assert_eq!(results[1].1, None);
    assert_eq!(output, "false\n");
}

#[test]
fn freeze_time_needs_a_test_an_instant_and_a_block() {
    let e = run_err("freeze_time(\"2026-09-28\") do\n  puts Time.today\nend\n", Vec::new());
    assert_eq!(e.ty, "RuntimeError");
    assert!(e.message.contains("only works in a test"), "{}", e.message);
    // nor while a test file loads, outside its tests
    let (src, out) = ("freeze_time(\"2026-09-28\") do\nend\ntest \"t\" do\nend\n", grenat_parser::parse);
    let options = Options { journal: Some(temp_dir("journal")), ..Options::default() };
    let loaded = run_tests(&out(src).program, options).unwrap_err();
    assert_eq!(loaded.ty, "RuntimeError");
    let (results, _) = tests_of(
        "\
test \"bad text\" do
  freeze_time(\"yesterday\") do
  end
end
test \"no instant\" do
  freeze_time(nil) do
  end
end
test \"no block\" do
  freeze_time(\"2026-09-28\")
end
",
    );
    let errors: Vec<String> = results.into_iter().map(|(_, e)| e.unwrap()).collect();
    assert!(errors[0].starts_with("ArgumentError: invalid time `yesterday`"), "{}", errors[0]);
    assert!(errors[1].starts_with("ArgumentError: `freeze_time` expects an instant"), "{}", errors[1]);
    assert!(errors[2].starts_with("ArgumentError: `freeze_time` expects a block"), "{}", errors[2]);
}
