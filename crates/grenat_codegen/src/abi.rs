//! Calling contract between native functions and the interpreter.
//!
//! Every compiled function takes its scalar parameters followed by two
//! integers, the current recursion `depth` and its `limit`, and returns two
//! values: its result and a status (0, or a [`Trap`] code). Nothing touches
//! memory: native code never unwinds, a failing callee returns a non-zero
//! status and every caller returns it immediately. Only the trampoline, at
//! the boundary with the interpreter, reads and writes a [`Context`].

use std::fmt;

/// Exchanged with the trampoline of a compiled function.
#[repr(C)]
#[derive(Debug, Default)]
pub(crate) struct Context {
    /// Written by native code: 0, or a [`Trap`] code.
    pub status: i64,
    /// Depth at which [`Trap::StackOverflow`] is raised (the interpreter's remaining budget).
    pub limit: i64,
}

pub(crate) const STATUS_OFFSET: i32 = 0;
pub(crate) const LIMIT_OFFSET: i32 = 8;

/// A runtime error raised by native code; mirrors the interpreter's errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trap {
    /// `OverflowError`
    Overflow = 1,
    /// `ZeroDivisionError`
    DivisionByZero = 2,
    /// `StackOverflow`
    StackOverflow = 3,
}

impl Trap {
    pub(crate) fn from_status(status: i64) -> Trap {
        match status {
            1 => Trap::Overflow,
            2 => Trap::DivisionByZero,
            _ => Trap::StackOverflow,
        }
    }

    /// Grenat error type and message, identical to the interpreter's.
    pub fn error(self) -> (&'static str, &'static str) {
        match self {
            Trap::Overflow => ("OverflowError", "integer overflow"),
            Trap::DivisionByZero => ("ZeroDivisionError", "division by zero"),
            Trap::StackOverflow => ("StackOverflow", "recursion too deep"),
        }
    }
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (ty, message) = self.error();
        write!(f, "{ty}: {message}")
    }
}
