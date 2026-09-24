//! Counts of objects on the current thread: live ones prove the absence of
//! leaks, allocations prove reuse.

use std::cell::Cell;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

pub(crate) fn allocated() {
    LIVE.with(|n| n.set(n.get() + 1));
    ALLOCATIONS.with(|n| n.set(n.get() + 1));
}

pub(crate) fn freed() {
    LIVE.with(|n| n.set(n.get() - 1));
}

/// Objects allocated and not yet freed by this thread. Every native call
/// frees everything it allocates, so it leaves this number unchanged.
pub fn live_objects() -> isize {
    LIVE.with(Cell::get)
}

/// Objects allocated so far by this thread (memory reused in place is not
/// allocated again).
pub fn allocations() -> usize {
    ALLOCATIONS.with(Cell::get)
}
