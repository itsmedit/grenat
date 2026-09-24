//! Scheduling, parking and waking, at scale and in both worlds (tasks, threads).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::*;

fn config(workers: usize) -> Config {
    Config { workers, main_stack: 1 << 20, task_stack: 64 << 10 }
}

#[test]
fn run_returns_once_every_task_has_finished() {
    let done = AtomicUsize::new(0);
    let value = run(config(4), |spawner| {
        for _ in 0..100 {
            // tasks borrow from the enclosing scope: `run` outlives them
            spawner.spawn(|| {
                yield_now();
                done.fetch_add(1, Ordering::SeqCst);
            });
        }
        42
    });
    assert_eq!(value, 42);
    assert_eq!(done.load(Ordering::SeqCst), 100);
}

#[test]
fn a_hundred_thousand_tasks_on_two_threads() {
    let n = 100_000;
    let sum = run(config(2), |spawner| {
        let (sender, receiver) = channel();
        for i in 0..n {
            let sender = sender.clone();
            spawner.spawn(move || sender.send(i).expect("open"));
        }
        drop(sender);
        receiver.sum::<u64>()
    });
    assert_eq!(sum, n * (n - 1) / 2);
}

#[test]
fn a_green_lock_can_be_held_while_the_task_waits() {
    let lock = Mutex::new(0u64);
    run(config(3), |spawner| {
        for _ in 0..1000 {
            spawner.spawn(|| {
                let mut value = lock.lock();
                let seen = *value;
                // other tasks run meanwhile, and wait for the lock
                yield_now();
                *value = seen + 1;
            });
        }
    });
    assert_eq!(*lock.lock(), 1000);
}

#[test]
fn a_condition_wakes_its_waiters() {
    let state = Mutex::new(0usize);
    let changed = Condvar::new();
    let seen = run(config(2), |spawner| {
        for _ in 0..50 {
            spawner.spawn(|| {
                *state.lock() += 1;
                changed.notify_all();
            });
        }
        let mut count = state.lock();
        while *count < 50 {
            count = changed.wait(count);
        }
        *count
    });
    assert_eq!(seen, 50);
}

#[test]
fn sleeping_tasks_leave_their_worker_free() {
    let started = Instant::now();
    run(config(1), |spawner| {
        for _ in 0..1000 {
            spawner.spawn(|| sleep(Duration::from_millis(100)));
        }
    });
    // one worker, a thousand sleeps of 100 ms: overlapped
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn blocking_calls_leave_their_worker_free() {
    let started = Instant::now();
    let results = run(config(1), |spawner| {
        let (sender, receiver) = channel();
        for i in 0..50 {
            let sender = sender.clone();
            spawner.spawn(move || {
                let value = blocking(|| {
                    std::thread::sleep(Duration::from_millis(100));
                    i * 2
                });
                sender.send(value).expect("open");
            });
        }
        drop(sender);
        receiver.collect::<Vec<_>>()
    });
    assert_eq!(results.len(), 50);
    assert_eq!(results.iter().sum::<i32>(), (0..50).map(|i| i * 2).sum::<i32>());
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn a_task_has_the_stack_it_asked_for() {
    fn depth(n: u64) -> u64 {
        let pad = std::hint::black_box([n; 32]);
        if n == 0 { 0 } else { 1 + depth(n - 1) + pad[0] - n }
    }
    let config = Config { workers: 1, main_stack: 64 << 20, task_stack: 64 << 10 };
    // ~300 bytes a frame: 100 000 frames need ~30 MB, fine on the 64 MB main stack
    assert_eq!(run(config, |_| depth(100_000)), 100_000);
}

#[test]
fn the_primitives_also_work_between_threads() {
    let lock = Mutex::new(0);
    let changed = Condvar::new();
    let (sender, receiver) = channel();
    std::thread::scope(|scope| {
        for i in 0..8 {
            let sender = sender.clone();
            let (lock, changed) = (&lock, &changed);
            scope.spawn(move || {
                *lock.lock() += 1;
                changed.notify_all();
                sender.send(i).expect("open");
            });
        }
        drop(sender);
        let mut count = lock.lock();
        while *count < 8 {
            count = changed.wait(count);
        }
    });
    assert_eq!(receiver.count(), 8);
    let (sender, receiver) = channel();
    drop(receiver);
    assert_eq!(sender.send(1), Err(1), "no receiver");
    assert_eq!(blocking(|| 7), 7);
    assert!(!in_task());
}

#[test]
fn late_wake_ups_do_not_steal_a_lock_hand_over() {
    // timers firing after their sleeper already left wake tasks that wait for
    // something else: the lock must still hand over to every waiter
    let lock = Mutex::new(0u64);
    run(config(4), |spawner| {
        for _ in 0..2000 {
            spawner.spawn(|| {
                for _ in 0..10 {
                    sleep(Duration::from_micros(50));
                    let mut value = lock.lock();
                    yield_now();
                    *value += 1;
                }
            });
        }
    });
    assert_eq!(*lock.lock(), 20_000);
}
