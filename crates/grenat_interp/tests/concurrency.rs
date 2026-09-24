//! Concurrence structurée : `parallel_map`, `race`, annulation, budgets partagés.

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
    // séquentiel : 1,2 s ; concurrent : ~0,3 s
    assert!(r.elapsed < SLOW * 3, "trop lent : {:?}", r.elapsed);
}

#[test]
fn parallel_map_respects_the_limit() {
    let r = run_with("p [1, 2, 3, 4].parallel_map(limit: 2) { |x| sleep 0.3; x }\n", vec![], &[]);
    assert_eq!(r.output, "[1, 2, 3, 4]\n");
    // deux vagues de deux
    assert!(r.elapsed >= SLOW * 2, "limite ignorée : {:?}", r.elapsed);
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
    raise ArgumentError, \"boum #{x}\" if x == 0
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
    assert_eq!(run(src), "boum 0\n[]\n");
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
    // réponse calculée depuis la requête : indépendante de l'ordre d'arrivée
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
  puts \"pas atteint\"
rescue BudgetExceeded => e
  puts \"budget dépassé\"
end
";
    let provider = Scripted::responder(|_| Response::json_reply(json!({"value": 0})).with_usage(1000, 1000));
    assert_eq!(run_provider(src, provider, &[], &[]).ok(), "budget dépassé\n");
}

#[test]
fn race_returns_the_fastest_branch() {
    let src = "\
t = Time.now
winner = race do
  begin
    sleep 0.5
    \"lent\"
  end
  \"rapide\"
end
puts winner
puts Time.now - t < 0.3
";
    assert_eq!(run(src), "rapide\ntrue\n");
}

#[test]
fn race_ignores_failed_branches_while_one_succeeds() {
    let src = "p(race do\n  raise ArgumentError, \"a\"\n  begin\n    sleep 0.1\n    \"b\"\n  end\nend)\n";
    assert_eq!(run(src), "\"b\"\n");
}

#[test]
fn race_raises_the_first_error_when_all_branches_fail() {
    let src = "race do\n  raise ArgumentError, \"premier\"\n  begin\n    sleep 0.1\n    raise ArgumentError, \"second\"\n  end\nend\n";
    let e = run_err(src, vec![]);
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("ArgumentError", "premier"));
}

#[test]
fn race_cancels_the_losers() {
    let src = "\
log = []
race do
  begin
    sleep 0.3
    log << \"perdant\"
  end
  \"gagnant\"
end
sleep 0.5
p log
";
    let r = run_with(src, vec![], &[]);
    assert_eq!(r.output, "[]\n");
    // le perdant s'arrête à son prochain point de contrôle : le programme ne l'attend pas
    assert!(r.elapsed < Duration::from_millis(900), "{:?}", r.elapsed);
}

#[test]
fn race_with_a_single_branch_runs_inline() {
    assert_eq!(run("p(race do\n  1 + 1\nend)\n"), "2\n");
}
