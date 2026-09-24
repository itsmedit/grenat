//! Lock access with `RefCell`'s method names (`borrow`, `borrow_mut`).

use std::sync::{Mutex, MutexGuard};

/// Access to values shared between tasks, with `RefCell`'s method names.
/// A poisoned lock (from a task that panicked) remains usable.
pub trait Locked<T> {
    fn borrow(&self) -> MutexGuard<'_, T>;
    fn borrow_mut(&self) -> MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn borrow(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn borrow_mut(&self) -> MutexGuard<'_, T> {
        self.borrow()
    }
}
