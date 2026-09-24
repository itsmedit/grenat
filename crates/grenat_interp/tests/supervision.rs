//! Supervision : redémarrage des agents qui plantent, stratégies, plafond de redémarrages.

mod common;

use common::*;

/// Deux enfants, `Worker` qui peut planter et `Other` ; `strategy` et `options` sont insérés.
fn desk(strategy: &str, options: &str, children: &str) -> String {
    format!(
        "\
agent Worker
  @n: Int = 0
  on Incr -> Int
    @n += 1
  end
  on Crash
    raise ArgumentError, \"boum\"
  end
end
agent Other
  @n: Int = 0
  on Incr -> Int
    @n += 1
  end
end
supervisor Desk, strategy: :{strategy}{options}
{children}
end
def crash
  Desk[Worker].ask(Crash())
rescue ArgumentError
  puts \"planté\"
end
"
    )
}

const BOTH: &str = "  child Worker\n  child Other";

#[test]
fn one_for_one_restarts_only_the_crashed_agent() {
    let src = desk("one_for_one", "", BOTH)
        + "Desk[Worker].ask(Incr())\nDesk[Worker].ask(Incr())\nDesk[Other].ask(Incr())\ncrash\np Desk[Worker].ask(Incr()), Desk[Other].ask(Incr())\n";
    let out = run(&src);
    assert!(out.contains("[superviseur Desk] `Worker` redémarré après ArgumentError : boum\n"), "{out}");
    // Worker repart d'un état neuf, Other garde le sien
    assert!(out.ends_with("planté\n1\n2\n"), "{out}");
}

#[test]
fn one_for_all_restarts_every_child() {
    let src = desk("one_for_all", "", BOTH)
        + "Desk[Other].ask(Incr())\nDesk[Other].ask(Incr())\ncrash\np Desk[Other].ask(Incr())\n";
    let out = run(&src);
    assert!(out.contains("`Other` redémarré"), "{out}");
    assert!(out.ends_with("planté\n1\n"), "{out}");
}

#[test]
fn rest_for_one_restarts_the_crashed_agent_and_the_later_ones() {
    // Other est déclaré avant Worker : il n'est pas redémarré
    let before = desk("rest_for_one", "", "  child Other\n  child Worker")
        + "Desk[Other].ask(Incr())\ncrash\np Desk[Other].ask(Incr())\n";
    assert!(run(&before).ends_with("planté\n2\n"));
    // Other est déclaré après Worker : il l'est
    let after = desk("rest_for_one", "", BOTH) + "Desk[Other].ask(Incr())\ncrash\np Desk[Other].ask(Incr())\n";
    assert!(run(&after).ends_with("planté\n1\n"));
}

#[test]
fn too_many_restarts_stop_the_agent_for_good() {
    let src = desk("one_for_one", ", max_restarts: 1, within: 60", BOTH) + "crash\ncrash\nDesk[Worker].ask(Incr())\n";
    let r = run_with(&src, vec![], &[]);
    assert!(r.output.contains("[superviseur Desk] `Worker` arrêté : 2 plantages en moins de 1min"), "{}", r.output);
    let e = r.err();
    assert_eq!(e.ty, "AgentDown");
    assert!(e.message.starts_with("l'agent `Worker` est arrêté"), "{}", e.message);
}

#[test]
fn restarts_outside_the_window_are_forgotten() {
    let src = desk("one_for_one", ", max_restarts: 1, within: 0.1", BOTH)
        + "crash\nsleep 0.2\ncrash\np Desk[Worker].ask(Incr())\n";
    let out = run(&src);
    assert!(!out.contains("arrêté"), "{out}");
    assert_eq!(out.matches("redémarré").count(), 2, "{out}");
    assert!(out.ends_with("planté\n1\n"), "{out}");
}

#[test]
fn a_child_is_a_single_shared_instance() {
    let src = desk("one_for_one", "", BOTH) + "Desk[Worker].ask(Incr())\np Desk[Worker].ask(Incr())\n";
    assert_eq!(run(&src), "2\n");
}

#[test]
fn only_declared_children_can_be_reached() {
    let src = desk("one_for_one", "", "  child Other") + "Desk[Worker].ask(Incr())\n";
    assert_eq!(run_err(&src, vec![]).ty, "NameError");
}

#[test]
fn invalid_strategy_is_an_error() {
    let src = desk("one_for_none", "", BOTH) + "Desk[Worker].ask(Incr())\n";
    let e = run_err(&src, vec![]);
    assert!(e.message.contains("stratégie de supervision inconnue `:one_for_none`"), "{}", e.message);
}
