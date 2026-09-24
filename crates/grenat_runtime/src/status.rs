//! Statuses of native calls: how a native function tells its caller that it
//! stopped early, and the Grenat error each one stands for.

/// `OverflowError`
pub const OVERFLOW: i64 = 1;
/// `ZeroDivisionError`
pub const DIVISION_BY_ZERO: i64 = 2;
/// `StackOverflow`
pub const STACK_OVERFLOW: i64 = 3;
/// A value native code cannot represent (`nil`): the interpreter runs the
/// call again, a standalone executable reports it.
pub const DEOPT: i64 = 4;
/// The task was cancelled.
pub const CANCELLED: i64 = 5;
/// `exit`: the program ends (its code is in the call's context).
pub const EXIT: i64 = 6;

/// Error type and message of a status that is an error, identical to the
/// interpreter's.
pub fn error(status: i64) -> Option<(&'static str, &'static str)> {
    Some(match status {
        OVERFLOW => ("OverflowError", "integer overflow"),
        DIVISION_BY_ZERO => ("ZeroDivisionError", "division by zero"),
        STACK_OVERFLOW => ("StackOverflow", "recursion too deep"),
        CANCELLED => ("Cancelled", "task cancelled"),
        _ => return None,
    })
}
