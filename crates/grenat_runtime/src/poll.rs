//! Checkpoints of long-running native code.
//!
//! Native code reads a flag of its call's [`Poll`] at every function entry
//! and loop iteration. A ticker thread raises the flag of every running
//! native call every [`TICK`]; native code then calls [`grenat_poll`], which
//! asks the host whether to stop (a cancelled task) — and gives the host a
//! place to let other tasks run. Reading a flag that rarely changes costs
//! almost nothing, unlike counting steps, which chains every call to the
//! previous one.
//!
//! The hook travels with the call, not in thread-local storage: a task may
//! resume on another thread.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, Once};
use std::time::Duration;

use crate::layout;

/// Period of the ticker.
pub const TICK: Duration = Duration::from_millis(10);

/// The checkpoint of a native call.
#[repr(C)]
pub struct Poll<'a> {
    /// Raised by the ticker, read by native code (offset [`layout::POLL_REQUESTED`]).
    requested: AtomicU8,
    /// `true`: stop (the task was cancelled).
    hook: &'a dyn Fn() -> bool,
}

/// Addresses of the flags of the running native calls.
static RUNNING: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static TICKER: Once = Once::new();

impl<'a> Poll<'a> {
    pub fn new(hook: &'a dyn Fn() -> bool) -> Poll<'a> {
        const { assert!(layout::POLL_REQUESTED == 0) };
        // raised from the start: a task cancelled before the call stops at once
        Poll { requested: AtomicU8::new(1), hook }
    }
}

/// Runs `f` while the ticker raises `poll` every [`TICK`] (`poll` must not
/// move meanwhile: `f` runs the native code that reads it).
pub fn polled<R>(poll: &Poll, f: impl FnOnce() -> R) -> R {
    TICKER.call_once(|| {
        std::thread::Builder::new()
            .name("grenat-ticker".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(TICK);
                    for &flag in RUNNING.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                        // SAFETY: registered flags are alive: they are removed (under this lock) before being dropped
                        unsafe { (*(flag as *const AtomicU8)).store(1, Ordering::Relaxed) };
                    }
                }
            })
            .expect("ticker thread");
    });
    let flag = &poll.requested as *const AtomicU8 as usize;
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).push(flag);
    // unregistered even if `f` panics
    struct Registered(usize);
    impl Drop for Registered {
        fn drop(&mut self) {
            let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = running.iter().position(|&f| f == self.0) {
                running.swap_remove(i);
            }
        }
    }
    let _registered = Registered(flag);
    f()
}

/// Called by native code when its checkpoint is requested: 1 to stop, 0 to go on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_poll(poll: *mut Poll) -> i64 {
    // SAFETY: the `Poll` of the running native call
    let poll = unsafe { &*poll };
    poll.requested.store(0, Ordering::Relaxed);
    i64::from((poll.hook)())
}
