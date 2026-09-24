//! Shapes: what a heap object contains, so that it can be released.

/// Kind of heap object, with the kinds of its slots.
#[derive(Debug)]
pub enum Shape {
    Str,
    /// Array whose elements are all of one kind.
    Array(Slot),
    /// Struct record, one slot per field.
    Record(Vec<Slot>),
}

/// Content of a value slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slot {
    /// `Int`, `Float` or `Bool`: nothing to release.
    Scalar,
    /// Pointer to an object of this shape. The shape outlives every object
    /// using it: the compiler owns all shapes for as long as its code runs.
    Heap(*const Shape),
}
