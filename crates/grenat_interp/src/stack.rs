//! Stack sizes of the interpreter's threads, and the guard that turns a real
//! stack exhaustion into a `StackOverflow` error instead of a crash.
//!
//! The interpreter walks the AST recursively, so the native stack used per
//! Grenat call depends on how nested the called code is: a call-count limit
//! alone cannot protect it. The guard measures the stack actually consumed.

/// Stack of the thread running the program (and `main`).
pub(crate) const MAIN_STACK: usize = 512 * 1024 * 1024;
/// Stack of each secondary task (`parallel_map`, `race`, `tell`).
pub(crate) const TASK_STACK: usize = 128 * 1024 * 1024;
/// Kept free below the limit for the native code that runs between two checks.
const MARGIN: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StackGuard {
    /// Address near the top of this thread's stack; 0 until the thread starts.
    base: usize,
    budget: usize,
}

impl StackGuard {
    /// For the current thread, whose stack holds `size` bytes.
    pub fn here(size: usize) -> StackGuard {
        StackGuard { base: stack_pointer(), budget: size.saturating_sub(MARGIN) }
    }

    /// Stacks grow downward on every supported target.
    pub fn exceeded(&self) -> bool {
        self.base != 0 && self.base.saturating_sub(stack_pointer()) > self.budget
    }
}

#[inline(never)]
fn stack_pointer() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}
