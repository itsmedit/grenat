//! M:N green threads: many tasks on a few OS threads.
//!
//! A task is a stackful coroutine (its stack is reserved, and only the pages
//! it touches are committed), run by a pool of worker threads, one per core.
//! When a task waits — for a lock, a message, other tasks, a timer, a
//! blocking call — it parks and its worker runs another task: waiting costs
//! no OS thread.
//!
//! The synchronization primitives of this crate ([`Mutex`], [`Condvar`],
//! [`channel`]) park the current task; outside a green task they block the
//! calling thread, so the same code runs in both worlds.
//!
//! A task may resume on another thread than the one it parked on: code
//! running in a task must not hold anything tied to a thread (a guard of an
//! OS lock, a reference into thread-local storage) across a point where it
//! may park.

mod blocking;
mod channel;
mod scheduler;
mod sync;
mod task;
mod timer;

pub use blocking::blocking;
pub use channel::{Receiver, Sender, channel};
pub use scheduler::{Config, Spawner, in_task, run, yield_now};
pub use sync::{Condvar, Mutex, MutexGuard};
pub use timer::sleep;

#[cfg(test)]
mod tests;
