//! Calling contract between native functions and the interpreter.
//!
//! Every compiled function takes its parameters (scalars, or pointers to
//! objects it then owns) followed by two integers, the current recursion
//! `depth` and its `limit`, and returns two values: its result and a status
//! (0, a [`Trap`] code, or `DEOPT`). Native code never unwinds: a failing
//! callee returns a non-zero status and every caller releases what it holds
//! and returns it immediately. Only the trampoline, at the boundary with the
//! interpreter, reads and writes a [`Context`].

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

/// Status of a call whose result native code cannot represent (`nil`…):
/// the interpreter runs the call again instead.
pub(crate) const DEOPT: i64 = 4;

/// Why a native call gave no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// A runtime error, identical to the interpreter's.
    Trap(Trap),
    /// Beyond what native code represents: interpret the call.
    Deopt,
}

impl Failure {
    pub(crate) fn from_status(status: i64) -> Failure {
        if status == DEOPT { Failure::Deopt } else { Failure::Trap(Trap::from_status(status)) }
    }
}

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
