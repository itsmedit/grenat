//! Memory layout of heap objects, as compiled code reads and writes them.
//!
//! Every value slot (array element, record field) is 8 bytes: an `Int`, the
//! bits of a `Float`, a `Bool` as 0 or 1, or a pointer to another object.

/// Reference count, first word of every object.
pub const RC: i32 = 0;
/// Length of a string (in bytes) or of an array (in elements).
pub const LEN: i32 = 8;
/// Pointer to the elements of an array.
pub const DATA: i32 = 24;
/// Size of a value slot.
pub const SLOT: i32 = 8;

/// Offset of field `index` in a record.
pub const fn field(index: usize) -> i32 {
    8 + SLOT * index as i32
}
