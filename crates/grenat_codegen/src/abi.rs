//! Calling contract between native functions and the interpreter.
//!
//! Every compiled function takes its parameters (scalars, or pointers to
//! objects it then owns) followed by the current recursion `depth` and the
//! address of the call's [`Context`] (recursion limit, checkpoint), and returns two values: its result and a status
//! (0, a [`Trap`] code, or `DEOPT`). Native code never unwinds: a failing
//! callee returns a non-zero status and every caller releases what it holds
//! and returns it immediately. Only the trampoline, at the boundary with the
//! interpreter, writes the status into the [`Context`].

use std::fmt;

use grenat_runtime::status;

pub(crate) use grenat_runtime::abi::{
    Context, EXIT_CODE_OFFSET, LIMIT_OFFSET, POLL_OFFSET, SITE_OFFSET, STATUS_OFFSET,
};
pub(crate) use grenat_runtime::status::DEOPT;

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
    Overflow = status::OVERFLOW as isize,
    /// `ZeroDivisionError`
    DivisionByZero = status::DIVISION_BY_ZERO as isize,
    /// `StackOverflow`
    StackOverflow = status::STACK_OVERFLOW as isize,
    /// `Cancelled`: the task was cancelled while native code ran.
    Cancelled = status::CANCELLED as isize,
}

impl Trap {
    pub(crate) fn from_status(status: i64) -> Trap {
        match status {
            status::OVERFLOW => Trap::Overflow,
            status::DIVISION_BY_ZERO => Trap::DivisionByZero,
            status::CANCELLED => Trap::Cancelled,
            _ => Trap::StackOverflow,
        }
    }

    /// Grenat error type and message, identical to the interpreter's.
    pub fn error(self) -> (&'static str, &'static str) {
        status::error(self as i64).expect("an error status")
    }
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (ty, message) = self.error();
        write!(f, "{ty}: {message}")
    }
}
