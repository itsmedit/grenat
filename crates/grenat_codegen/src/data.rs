//! Values crossing the boundary between the interpreter and native code.
//!
//! They are copied both ways: native objects never escape a native call,
//! which keeps their reference counts thread-local (see `grenat_runtime`).

/// A value given to or returned by native code.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Array(Vec<Data>),
    /// A struct value, fields in declaration order.
    Record { ty: String, fields: Vec<(String, Data)> },
    /// The very array given as argument number `n`: arrays are references,
    /// so an array passed twice, or returned, is the same object.
    Alias(usize),
}

/// A successful native call.
#[derive(Debug, Clone, PartialEq)]
pub struct Returned {
    pub value: Data,
    /// Content of every array argument after the call, which the interpreter
    /// writes back since native code worked on a copy. `None` for other
    /// arguments, for aliases, and when the function modifies no array.
    pub arrays: Vec<Option<Vec<Data>>>,
}
