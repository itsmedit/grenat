//! Slots: at most so many calls in flight at once; the others queue, and
//! are let in first come, first served.

use std::collections::VecDeque;

/// A concurrency limit and its queue.
#[derive(Debug, Clone)]
pub struct Slots {
    max: usize,
    in_flight: usize,
    queue: VecDeque<u64>,
    next_ticket: u64,
}

impl Slots {
    pub fn new(max: usize) -> Slots {
        Slots { max: max.max(1), in_flight: 0, queue: VecDeque::new(), next_ticket: 0 }
    }

    /// Joins the queue: the ticket to show [`Slots::try_enter`].
    pub fn enqueue(&mut self) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.queue.push_back(ticket);
        ticket
    }

    /// Takes a slot if one is free and `ticket` is first in the queue.
    pub fn try_enter(&mut self, ticket: u64) -> bool {
        if self.in_flight < self.max && self.queue.front() == Some(&ticket) {
            self.queue.pop_front();
            self.in_flight += 1;
            return true;
        }
        false
    }

    /// Leaves the queue without a slot (the caller was cancelled).
    pub fn leave(&mut self, ticket: u64) {
        self.queue.retain(|t| *t != ticket);
    }

    /// Gives a slot back.
    pub fn release(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_come_first_served() {
        let mut slots = Slots::new(2);
        let (a, b, c, d) = (slots.enqueue(), slots.enqueue(), slots.enqueue(), slots.enqueue());
        // `b` cannot pass `a`
        assert!(!slots.try_enter(b));
        assert!(slots.try_enter(a));
        assert!(slots.try_enter(b));
        assert!(!slots.try_enter(c));
        assert_eq!(slots.in_flight(), 2);
        slots.release();
        // `c` left: `d` is first
        slots.leave(c);
        assert!(slots.try_enter(d));
        assert!(!slots.try_enter(c));
    }
}
