//! A lock and a condition variable that park the waiting task.

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::PoisonError;

use crate::scheduler::{current_waiter, park};
use crate::task::Waiter;

/// A mutual exclusion lock whose waiters park; its guard may be held across
/// suspension points (unlike an OS lock).
pub struct Mutex<T: ?Sized> {
    state: std::sync::Mutex<LockState>,
    value: UnsafeCell<T>,
}

/// The waiters of a lock. Wake-ups may be spurious (a timer that fired late,
/// a notification meant for an earlier wait): each waiting attempt has a
/// ticket, removed when the waiter runs again, so that `unlock` never spends
/// its wake-up on a task that no longer waits.
#[derive(Default)]
struct LockState {
    locked: bool,
    waiters: VecDeque<(u64, Waiter)>,
    next_ticket: u64,
}

// SAFETY: access to `value` is serialized by `locked`
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Mutex<T> {
        Mutex {
            state: std::sync::Mutex::new(LockState { locked: false, waiters: VecDeque::new(), next_ticket: 0 }),
            value: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> MutexGuard<'_, T> {
        loop {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !state.locked {
                state.locked = true;
                return MutexGuard { mutex: self };
            }
            let ticket = state.next_ticket;
            state.next_ticket += 1;
            state.waiters.push_back((ticket, current_waiter()));
            drop(state);
            park();
            // woken, possibly spuriously: this attempt no longer waits
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.waiters.retain(|(t, _)| *t != ticket);
        }
    }

    fn unlock(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.locked = false;
        if let Some((_, waiter)) = state.waiters.pop_front() {
            waiter.wake();
        }
    }
}

pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock
        unsafe { &*self.mutex.value.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the lock
        unsafe { &mut *self.mutex.value.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

/// Waits for a condition protected by a [`Mutex`]. Wake-ups may be spurious:
/// check the condition in a loop.
#[derive(Default)]
pub struct Condvar {
    waiters: std::sync::Mutex<(Vec<(u64, Waiter)>, u64)>,
}

impl Condvar {
    pub const fn new() -> Condvar {
        Condvar { waiters: std::sync::Mutex::new((Vec::new(), 0)) }
    }

    /// Releases the lock, waits for a notification, takes the lock again.
    pub fn wait<'a, T: ?Sized>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        // registered before the lock is released: a notification cannot be missed
        let ticket = {
            let mut waiters = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
            waiters.1 += 1;
            let ticket = waiters.1;
            waiters.0.push((ticket, current_waiter()));
            ticket
        };
        let mutex = guard.mutex;
        drop(guard);
        park();
        self.waiters.lock().unwrap_or_else(PoisonError::into_inner).0.retain(|(t, _)| *t != ticket);
        mutex.lock()
    }

    pub fn notify_all(&self) {
        let waiters = std::mem::take(&mut self.waiters.lock().unwrap_or_else(PoisonError::into_inner).0);
        for (_, waiter) in waiters {
            waiter.wake();
        }
    }
}
