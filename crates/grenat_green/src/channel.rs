//! A multi-producer, single-consumer channel whose receiver parks.

use std::collections::VecDeque;
use std::sync::{Arc, PoisonError};

use crate::scheduler::{current_waiter, park};
use crate::task::Waiter;

struct State<T> {
    items: VecDeque<T>,
    senders: usize,
    /// The receiver still exists.
    open: bool,
    receiver: Option<Waiter>,
}

type Shared<T> = Arc<std::sync::Mutex<State<T>>>;

pub struct Sender<T> {
    shared: Shared<T>,
}

pub struct Receiver<T> {
    shared: Shared<T>,
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(std::sync::Mutex::new(State { items: VecDeque::new(), senders: 1, open: true, receiver: None }));
    (Sender { shared: shared.clone() }, Receiver { shared })
}

impl<T> Sender<T> {
    /// Gives the value back if the receiver is gone.
    pub fn send(&self, value: T) -> Result<(), T> {
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.open {
            return Err(value);
        }
        state.items.push_back(value);
        if let Some(waiter) = state.receiver.take() {
            waiter.wake();
        }
        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner).senders += 1;
        Sender { shared: self.shared.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        state.senders -= 1;
        if state.senders == 0
            && let Some(waiter) = state.receiver.take()
        {
            waiter.wake();
        }
    }
}

impl<T> Receiver<T> {
    /// The next value; `None` once every sender is gone and nothing is left.
    pub fn recv(&self) -> Option<T> {
        loop {
            let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(value) = state.items.pop_front() {
                return Some(value);
            }
            if state.senders == 0 {
                return None;
            }
            state.receiver = Some(current_waiter());
            drop(state);
            park();
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        state.open = false;
        state.items.clear();
    }
}

impl<T> Iterator for Receiver<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.recv()
    }
}
