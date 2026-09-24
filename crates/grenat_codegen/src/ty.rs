//! Types of the values native code handles.

use cranelift_codegen::ir::{Type, types};

/// Index of a struct in [`Structs`](crate::structs::Structs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct StructId(pub usize);

/// Type of a value native code can hold.
///
/// Arrays do not nest and struct fields are never arrays: an array is a
/// mutable reference, and only arrays passed directly as arguments can be
/// copied back to the interpreter after a call (see [`jit`](crate::jit)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Array(Elem),
    Struct(StructId),
}

/// Type of an array element or of a struct field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Elem {
    Int,
    Float,
    Bool,
    Str,
    Struct(StructId),
    /// Element type of `[]` not known yet (resolved by inference).
    Unknown,
}

impl Ty {
    /// Cranelift type of a value in a register: objects are pointers.
    pub fn clif(self) -> Type {
        match self {
            Ty::Float => types::F64,
            Ty::Bool => types::I8,
            _ => types::I64,
        }
    }

    /// A reference-counted object.
    pub fn is_heap(self) -> bool {
        matches!(self, Ty::Str | Ty::Array(_) | Ty::Struct(_))
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    /// Contains a type still to infer.
    pub fn is_unknown(self) -> bool {
        self == Ty::Array(Elem::Unknown)
    }

    /// `Some` for types that can be array elements or struct fields.
    pub fn elem(self) -> Option<Elem> {
        Some(match self {
            Ty::Int => Elem::Int,
            Ty::Float => Elem::Float,
            Ty::Bool => Elem::Bool,
            Ty::Str => Elem::Str,
            Ty::Struct(id) => Elem::Struct(id),
            Ty::Array(_) => return None,
        })
    }
}

impl Elem {
    /// `None` while unknown.
    pub fn ty(self) -> Option<Ty> {
        Some(match self {
            Elem::Int => Ty::Int,
            Elem::Float => Ty::Float,
            Elem::Bool => Ty::Bool,
            Elem::Str => Ty::Str,
            Elem::Struct(id) => Ty::Struct(id),
            Elem::Unknown => return None,
        })
    }

    pub fn is_heap(self) -> bool {
        matches!(self, Elem::Str | Elem::Struct(_))
    }
}
