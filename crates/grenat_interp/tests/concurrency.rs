//! Structured concurrency: `parallel_map`, `race`, cancellation, shared budgets.

mod common;

use std::time::Duration;

use common::*;
use grenat_interp::{Response, Scripted};
use serde_json::json;

const SLOW: Duration = Duration::from_millis(300);

#[test]
fn parallel_map_runs_items_concurrently_and_keeps_order() {
    let r = run_with("p [1, 2, 3, 4].parallel_map(limit: 4) { |x| sleep 0.3; x * 10 }\n", vec![], &[]);
    assert_eq!(r.output, "[10, 20, 30, 40]\n");
    // sequential: 1.2 s; concurrent: ~0.3 s
    assert!(r.elapsed < SLOW * 3, "too slow: {:?}", r.elapsed);
}

#[test]
fn parallel_map_respects_the_limit() {
    let r = run_with("p [1, 2, 3, 4].parallel_map(limit: 2) { |x| sleep 0.3; x }\n", vec![], &[]);
    assert_eq!(r.output, "[1, 2, 3, 4]\n");
    // two waves of two
    assert!(r.elapsed >= SLOW * 2, "limit ignored: {:?}", r.elapsed);
}

#[test]
fn parallel_map_with_limit_one_is_sequential() {
    assert_eq!(run("p [3, 1, 2].parallel_map(limit: 1) { |x| x + 1 }\n"), "[4, 2, 3]\n");
}

#[test]
fn first_error_wins_and_cancels_the_other_items() {
    let src = "\
seen = []
begin
  [0, 1, 2, 3].parallel_map(limit: 4) { |x|
    raise ArgumentError, \"boom #{x}\" if x == 0
    sleep 0.3
    seen << x
    x
  }
rescue ArgumentError => e
  puts e.message
end
sleep 0.4
p seen
";
    assert_eq!(run(src), "boom 0\n[]\n");
}

#[test]
fn parallel_prompts_get_their_own_answers() {
    let src = "\
model :fast, name: \"claude-haiku-4-5\"
prompt double(n: Int) -> ~Int using :fast
  user \"n=#{n}\"
end
p (1..6).to_a.parallel_map(limit: 6) { |n| double(n).trust! }
";
    // reply computed from the request: independent of arrival order
    let provider = Scripted::responder(|body| {
        let n: i64 = body["messages"][0]["content"].as_str().unwrap().trim_start_matches("n=").parse().unwrap();
        Response::json_reply(json!({"value": n * 2}))
    });
    let r = run_provider(src, provider, &[], &[]);
    assert_eq!(r.output.clone(), "[2, 4, 6, 8, 10, 12]\n", "{:?}", r.result);
    assert_eq!(r.requests.len(), 6);
}

#[test]
fn budgets_are_shared_by_parallel_tasks() {
    let src = "\
model :fast, name: \"claude-haiku-4-5\"
prompt double(n: Int) -> ~Int using :fast
  user \"n=#{n}\"
end
within budget(usd: 0.001) do
  [1, 2, 3].parallel_map(limit: 3) { |n| double(n) }
  puts \"not reached\"
rescue BudgetExceeded => e
  puts \"budget exceeded\"
end
";
    let provider = Scripted::responder(|_| Response::json_reply(json!({"value": 0})).with_usage(1000, 1000));
    assert_eq!(run_provider(src, provider, &[], &[]).ok(), "budget exceeded\n");
}

#[test]
fn race_returns_the_fastest_branch() {
    let src = "\
t = Time.now
winner = race do
  begin
    sleep 0.5
    \"slow\"
  end
  \"fast\"
end
puts winner
puts Time.now - t < 0.3
";
    assert_eq!(run(src), "fast\ntrue\n");
}

#[test]
fn race_ignores_failed_branches_while_one_succeeds() {
    let src = "p(race do\n  raise ArgumentError, \"a\"\n  begin\n    sleep 0.1\n    \"b\"\n  end\nend)\n";
    assert_eq!(run(src), "\"b\"\n");
}

#[test]
fn race_raises_the_first_error_when_all_branches_fail() {
    let src = "race do\n  raise ArgumentError, \"first\"\n  begin\n    sleep 0.1\n    raise ArgumentError, \"second\"\n  end\nend\n";
    let e = run_err(src, vec![]);
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("ArgumentError", "first"));
}

#[test]
fn race_cancels_the_losers() {
    let src = "\
log = []
race do
  begin
    sleep 0.3
    log << \"loser\"
  end
  \"winner\"
end
sleep 0.5
p log
";
    let r = run_with(src, vec![], &[]);
    assert_eq!(r.output, "[]\n");
    // the loser stops at its next checkpoint: the program does not wait for it
    assert!(r.elapsed < Duration::from_millis(900), "{:?}", r.elapsed);
}

#[test]
fn race_with_a_single_branch_runs_inline() {
    assert_eq!(run("p(race do\n  1 + 1\nend)\n"), "2\n");
}

#[test]
fn deep_recursion_inside_a_task_is_an_error_not_a_crash() {
    // tasks have a smaller stack than the main thread: the guard must still catch it
    let src = "\
def down(n) = if n == 0 then 0 else 1 + down(n - 1) end
begin
  [1, 2].parallel_map(limit: 2) { |_| down(1_000_000) }
rescue StackOverflow => e
  puts e.message
end
";
    assert_eq!(run(src), "recursion too deep\n");
}

#[test]
fn ten_thousand_tasks_wait_together_on_a_few_threads() {
    // green threads: a sleeping task parks, it does not hold an OS thread
    let src = "\
xs = (1..10000).to_a.parallel_map(limit: 10000) do |x|
  sleep 0.2
  x * 2
end
p [xs.length, xs.sum]
";
    let started = std::time::Instant::now();
    assert_eq!(run(src), "[10000, 100010000]\n");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
}
