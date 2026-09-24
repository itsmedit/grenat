//! Supervision: restarting crashed agents, strategies, restart limits.

mod common;

use common::*;

/// Two children, `Worker` (which can crash) and `Other`; `strategy` and `options` are spliced in.
fn desk(strategy: &str, options: &str, children: &str) -> String {
    format!(
        "\
agent Worker
  @n: Int = 0
  on Incr -> Int
    @n += 1
  end
  on Crash
    raise ArgumentError, \"boom\"
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
  puts \"crashed\"
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
    assert!(out.contains("[supervisor Desk] `Worker` restarted after ArgumentError: boom\n"), "{out}");
    // Worker starts from a fresh state, Other keeps its own
    assert!(out.ends_with("crashed\n1\n2\n"), "{out}");
}

#[test]
fn one_for_all_restarts_every_child() {
    let src = desk("one_for_all", "", BOTH)
        + "Desk[Other].ask(Incr())\nDesk[Other].ask(Incr())\ncrash\np Desk[Other].ask(Incr())\n";
    let out = run(&src);
    assert!(out.contains("`Other` restarted"), "{out}");
    assert!(out.ends_with("crashed\n1\n"), "{out}");
}

#[test]
fn rest_for_one_restarts_the_crashed_agent_and_the_later_ones() {
    // Other is declared before Worker: it is not restarted
    let before = desk("rest_for_one", "", "  child Other\n  child Worker")
        + "Desk[Other].ask(Incr())\ncrash\np Desk[Other].ask(Incr())\n";
    assert!(run(&before).ends_with("crashed\n2\n"));
    // Other is declared after Worker: it is
    let after = desk("rest_for_one", "", BOTH) + "Desk[Other].ask(Incr())\ncrash\np Desk[Other].ask(Incr())\n";
    assert!(run(&after).ends_with("crashed\n1\n"));
}

#[test]
fn too_many_restarts_stop_the_agent_for_good() {
    let src = desk("one_for_one", ", max_restarts: 1, within: 60", BOTH) + "crash\ncrash\nDesk[Worker].ask(Incr())\n";
    let r = run_with(&src, vec![], &[]);
    assert!(r.output.contains("[supervisor Desk] `Worker` stopped: 2 crashes within 1min"), "{}", r.output);
    let e = r.err();
    assert_eq!(e.ty, "AgentDown");
    assert!(e.message.starts_with("agent `Worker` is down"), "{}", e.message);
}

#[test]
fn restarts_outside_the_window_are_forgotten() {
    let src = desk("one_for_one", ", max_restarts: 1, within: 0.1", BOTH)
        + "crash\nsleep 0.2\ncrash\np Desk[Worker].ask(Incr())\n";
    let out = run(&src);
    assert!(!out.contains("stopped"), "{out}");
    assert_eq!(out.matches("restarted").count(), 2, "{out}");
    assert!(out.ends_with("crashed\n1\n"), "{out}");
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
    assert!(e.message.contains("unknown supervision strategy `:one_for_none`"), "{}", e.message);
}
