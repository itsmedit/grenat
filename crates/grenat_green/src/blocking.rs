//! Blocking calls (network, standard input) off the workers.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Mutex, PoisonError};

use crate::scheduler::{current_waiter, in_task, park};

/// Runs `f`, which may block for long, on its own OS thread while the current
/// task parks; outside a task, runs it in place.
pub fn blocking<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    if !in_task() {
        return f();
    }
    let slot: Arc<Mutex<Option<std::thread::Result<R>>>> = Arc::new(Mutex::new(None));
    let waiter = current_waiter();
    let done = slot.clone();
    let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
        // a panic is the task's, raised again there: never a task parked forever
        let value = catch_unwind(AssertUnwindSafe(f));
        *done.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
        waiter.wake();
    });
    // SAFETY: this task does not return before `job` has stored its result,
    // after which the thread no longer touches anything `f` borrowed
    let job: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(job) };
    std::thread::Builder::new().name("grenat-blocking".into()).spawn(job).expect("blocking thread");
    loop {
        if let Some(value) = slot.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return value.unwrap_or_else(|panic| resume_unwind(panic));
        }
        park();
    }
}
