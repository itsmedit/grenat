//! Runtime of native Grenat code.
//!
//! Compiled code manipulates heap objects (strings, arrays, struct records)
//! through the `extern "C"` functions of this crate, listed by [`symbols`].
//! Every object starts with its reference count, an `i64` at offset 0, which
//! compiled code increments and decrements inline (Perceus: the compiler
//! inserts every `dup` and `drop`, there is no garbage collector).
//!
//! Counts are not atomic: a native object never leaves the thread that runs
//! the native call. Values cross the boundary with the interpreter by copy
//! (see `grenat_codegen`), so no object is ever shared between threads.
//!
//! How an object's children are released is described by a [`Shape`], built
//! once per type by the compiler and passed to the release functions.

mod array;
mod format;
pub mod layout;
mod live;
mod poll;
mod record;
mod release;
mod shape;
mod string;
mod symbols;

pub use array::Arr;
pub use format::float as format_float;
pub use live::{allocations, live_objects};
pub use poll::{Poll, TICK as POLL_TICK, polled};
pub use record::Record;
pub use release::{release, retain};
pub use shape::{Shape, Slot};
pub use string::Str;
pub use symbols::symbols;

#[cfg(test)]
mod tests;
