//! Accès aux verrous avec les noms de `RefCell` (`borrow`, `borrow_mut`).

use std::sync::{Mutex, MutexGuard};

/// Accès aux valeurs partagées entre tâches, avec les noms de `RefCell`.
/// Un verrou empoisonné (tâche qui a paniqué) reste utilisable.
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
