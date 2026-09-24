//! Shapes: what a heap object contains, so that it can be released.
//!
//! A shape is plain data with a fixed layout ([`layout::SHAPE_KIND`](crate::layout)
//! and following): the compiler emits the shapes of a program as data of its
//! code, in memory for the JIT or in the object file of an executable.

/// A string, an array (one slot: its elements) or a record (one slot per field).
#[repr(C)]
pub struct Shape {
    kind: u64,
    count: u64,
    slots: *const Slot,
}

/// Content of a value slot: null for a scalar (`Int`, `Float`, `Bool`),
/// otherwise the shape of the object it points to.
pub type Slot = *const Shape;

pub const STR: u64 = 0;
pub const ARRAY: u64 = 1;
pub const RECORD: u64 = 2;

impl Shape {
    /// # Safety
    /// `slots` must point to `count` slots that outlive the shape.
    pub const unsafe fn from_raw(kind: u64, slots: *const Slot, count: usize) -> Shape {
        Shape { kind, count: count as u64, slots }
    }

    pub const fn string() -> Shape {
        Shape { kind: STR, count: 0, slots: std::ptr::null() }
    }

    pub fn kind(&self) -> u64 {
        self.kind
    }

    /// The array's elements, or the record's fields.
    pub fn slots(&self) -> &[Slot] {
        if self.count == 0 {
            return &[];
        }
        // SAFETY: `count` slots, alive as long as the shape (see `from_raw`)
        unsafe { std::slice::from_raw_parts(self.slots, self.count as usize) }
    }
}
