//! Scalar types and values: the only data native code handles in this first slice.

use std::fmt;

use cranelift_codegen::ir::{Type, types};

/// Type of a value native code can hold in a register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarTy {
    Int,
    Float,
    Bool,
}

impl ScalarTy {
    /// From a Grenat type annotation name (`Int`, `Float`, `Bool`).
    pub fn from_name(name: &str) -> Option<ScalarTy> {
        match name {
            "Int" => Some(ScalarTy::Int),
            "Float" => Some(ScalarTy::Float),
            "Bool" => Some(ScalarTy::Bool),
            _ => None,
        }
    }

    pub(crate) fn clif(self) -> Type {
        match self {
            ScalarTy::Int => types::I64,
            ScalarTy::Float => types::F64,
            ScalarTy::Bool => types::I8,
        }
    }
}

impl fmt::Display for ScalarTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ScalarTy::Int => "Int",
            ScalarTy::Float => "Float",
            ScalarTy::Bool => "Bool",
        })
    }
}

/// A scalar value crossing the boundary between the interpreter and native code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar {
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl Scalar {
    pub fn ty(self) -> ScalarTy {
        match self {
            Scalar::Int(_) => ScalarTy::Int,
            Scalar::Float(_) => ScalarTy::Float,
            Scalar::Bool(_) => ScalarTy::Bool,
        }
    }

    /// 64-bit encoding used by the uniform call trampoline.
    pub(crate) fn to_bits(self) -> u64 {
        match self {
            Scalar::Int(n) => n as u64,
            Scalar::Float(f) => f.to_bits(),
            Scalar::Bool(b) => u64::from(b),
        }
    }

    pub(crate) fn from_bits(ty: ScalarTy, bits: u64) -> Scalar {
        match ty {
            ScalarTy::Int => Scalar::Int(bits as i64),
            ScalarTy::Float => Scalar::Float(f64::from_bits(bits)),
            ScalarTy::Bool => Scalar::Bool(bits & 0xff != 0),
        }
    }
}
