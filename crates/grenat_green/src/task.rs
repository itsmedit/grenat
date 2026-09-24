//! A task, its state, and how to wake it.

use std::cell::{Cell, UnsafeCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, Yielder};

use crate::scheduler::Shared;

/// Why a task gives its worker back.
pub(crate) enum Suspend {
    /// Still runnable: back to the queue.
    Yield,
    /// Waiting: runnable again once woken.
    Park,
}

pub(crate) type Co = Coroutine<(), Suspend, (), DefaultStack>;

// States of a task. Only the worker running a task touches its coroutine.
/// In the run queue.
pub(crate) const QUEUED: u8 = 0;
pub(crate) const RUNNING: u8 = 1;
pub(crate) const PARKED: u8 = 2;
/// Woken while still running (before it could park): it must not park.
pub(crate) const NOTIFIED: u8 = 3;
pub(crate) const DONE: u8 = 4;

pub(crate) struct Task {
    pub state: AtomicU8,
    pub co: UnsafeCell<Option<Co>>,
    /// Set by the task when it starts: it lives on the task's own stack.
    pub yielder: Cell<*const Yielder<(), Suspend>>,
    pub shared: Arc<Shared>,
}

// SAFETY: the coroutine and the yielder are only touched by the worker that
// runs the task (the state machine hands a task to one worker at a time), and
// by the task itself. What a task keeps on its stack across a suspension is
// `Send` by the contract of this crate (nothing tied to a thread).
unsafe impl Send for Task {}
unsafe impl Sync for Task {}

impl Task {
    /// Makes a parked task runnable (or keeps a running one from parking).
    pub fn wake(self: &Arc<Task>) {
        loop {
            match self.state.load(Ordering::Acquire) {
                PARKED => {
                    if self.state.compare_exchange(PARKED, QUEUED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                        self.shared.push(self.clone());
                        return;
                    }
                }
                RUNNING => {
                    if self.state.compare_exchange(RUNNING, NOTIFIED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                        return;
                    }
                }
                _ => return,
            }
        }
    }
}

/// Something waiting: a green task, or an OS thread outside the scheduler.
#[derive(Clone)]
pub(crate) enum Waiter {
    Task(Arc<Task>),
    Thread(std::thread::Thread),
}

impl Waiter {
    pub fn wake(&self) {
        match self {
            Waiter::Task(task) => task.wake(),
            Waiter::Thread(thread) => thread.unpark(),
        }
    }
}
