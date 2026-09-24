//! Agents-acteurs : un message à la fois, pools, `tell`, interblocages.

mod common;

use std::time::Duration;

use common::*;

const COUNTER: &str = "\
agent Counter
  @n: Int = 0
  on Incr
    v = @n
    sleep 0.02
    @n = v + 1
  end
  on Get -> Int
    @n
  end
end
";

#[test]
fn an_agent_handles_one_message_at_a_time() {
    // sans exclusion mutuelle, les lectures-écritures concurrentes perdraient des incréments
    let src = format!(
        "{COUNTER}c = spawn Counter\n(1..10).to_a.parallel_map(limit: 10) {{ |_| c.ask(Incr()) }}\np c.ask(Get())\n"
    );
    assert_eq!(run(&src), "10\n");
}

#[test]
fn different_agents_work_in_parallel() {
    let src = "\
agent Slow
  on Work -> Int
    sleep 0.3
    1
  end
end
agents = [spawn(Slow), spawn(Slow), spawn(Slow)]
p agents.parallel_map(limit: 3) { |a| a.ask(Work()) }
";
    let r = run_with(src, vec![], &[]);
    assert_eq!(r.output, "[1, 1, 1]\n");
    assert!(r.elapsed < Duration::from_millis(800), "{:?}", r.elapsed);
}

#[test]
fn a_pool_spreads_messages_over_its_agents() {
    let src = "\
agent Worker
  @calls: Int = 0
  on Work -> Int
    @calls += 1
    sleep 0.2
    @calls
  end
end
pool = spawn_pool(Worker, size: 4)
p (1..4).to_a.parallel_map(limit: 4) { |_| pool.ask(Work()) }
";
    let r = run_with(src, vec![], &[]);
    // chaque agent n'a reçu qu'un message ; un seul agent aurait répondu 1, 2, 3, 4
    assert_eq!(r.output, "[1, 1, 1, 1]\n");
    assert!(r.elapsed < Duration::from_millis(700), "{:?}", r.elapsed);
}

#[test]
fn tell_runs_in_the_background_and_the_program_waits_for_it() {
    let src = "\
agent Logger
  on Log(text: String)
    sleep 0.2
    puts \"journal : #{text}\"
  end
end
l = spawn Logger
l.tell(Log(text: \"a\"))
puts \"après tell\"
";
    assert_eq!(run(src), "après tell\njournal : a\n");
}

#[test]
fn tell_failures_are_reported() {
    let src = "\
agent Fragile
  on Break
    raise ArgumentError, \"boum\"
  end
end
spawn(Fragile).tell(Break())
";
    assert!(run(src).contains("[tell] l'agent `Fragile` a échoué : ArgumentError : boum"));
}

#[test]
fn asking_yourself_is_a_detected_deadlock() {
    let src = "\
agent Echo
  on Loop(me: Echo)
    me.ask(Ping())
  end
  on Ping -> Int
    1
  end
end
e = spawn Echo
e.ask(Loop(me: e))
";
    let e = run_err(src, vec![]);
    assert_eq!(e.ty, "DeadlockError");
    assert!(e.message.contains("`Echo` s'envoie un message à lui-même"), "{}", e.message);
}

#[test]
fn a_waiting_cycle_is_a_detected_deadlock() {
    let src = "\
agent A
  on Start(b: B, me: A) -> Int
    b.ask(Relay(a: me))
  end
  on Ping -> Int
    1
  end
end
agent B
  on Relay(a: A) -> Int
    a.ask(Ping())
  end
end
a = spawn A
b = spawn B
a.ask(Start(b:, me: a))
";
    let e = run_err(src, vec![]);
    assert_eq!(e.ty, "DeadlockError");
    assert_eq!(e.message, "cycle d'attente entre agents : A → B → A");
}

#[test]
fn a_cycle_across_tasks_is_detected_and_the_other_task_proceeds() {
    let src = "\
agent A
  on Go(other: B) -> Int
    sleep 0.2
    other.ask(Pong())
  end
  on Pong -> Int
    1
  end
end
agent B
  on Go(other: A) -> Int
    sleep 0.2
    other.ask(Pong())
  end
  on Pong -> Int
    2
  end
end
a = spawn A
b = spawn B
begin
  [1, 2].parallel_map(limit: 2) { |i| if i == 1 then a.ask(Go(other: b)) else b.ask(Go(other: a)) end }
rescue DeadlockError => e
  puts e.message
end
";
    let out = run(src);
    assert!(out.starts_with("cycle d'attente entre agents : "), "{out}");
}

#[test]
fn a_crash_in_an_unsupervised_agent_keeps_its_state() {
    let src = "\
agent Worker
  @n: Int = 0
  on Incr -> Int
    @n += 1
  end
  on Crash
    raise ArgumentError, \"boum\"
  end
end
w = spawn Worker
w.ask(Incr())
begin
  w.ask(Crash())
rescue ArgumentError
end
p w.ask(Incr())
";
    assert_eq!(run(src), "2\n");
}

#[test]
fn messages_to_unknown_handlers_are_errors() {
    let src = format!("{COUNTER}spawn(Counter).ask(Incr(1))\n");
    assert_eq!(run_err(&src, vec![]).ty, "ArgumentError");
}
