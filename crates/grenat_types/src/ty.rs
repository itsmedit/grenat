//! Types as seen by the checker.

use std::fmt;

use grenat_ast::Span;

/// Static type. `Unknown` is compatible with everything: the checker is
/// gradual and never reports what it cannot prove.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Unknown,
    Nil,
    Bool,
    Int,
    Float,
    Str,
    Sym,
    Money,
    Duration,
    Range,
    Budget,
    Array(Box<Ty>),
    Hash(Box<Ty>, Box<Ty>),
    Opt(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    /// Declared type: struct, class, module, enum, agent, error, message.
    User(String),
    /// A type used as a value: `Researcher`, `File`.
    Type(String),
    /// A credential (`Credentials.fetch`): not a `String`, never for a model.
    Secret,
}

impl Ty {
    pub fn array(item: Ty) -> Ty {
        Ty::Array(Box::new(item))
    }

    pub fn opt(inner: Ty) -> Ty {
        match inner {
            Ty::Opt(_) | Ty::Unknown | Ty::Nil => inner,
            other => Ty::Opt(Box::new(other)),
        }
    }

    pub fn user(name: &str) -> Ty {
        Ty::User(name.to_string())
    }

    /// Underlying type of an optional (lenient member access on `T?`).
    pub fn base(&self) -> &Ty {
        match self {
            Ty::Opt(inner) => inner.base(),
            other => other,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unknown => f.write_str("?"),
            Ty::Nil => f.write_str("Nil"),
            Ty::Bool => f.write_str("Bool"),
            Ty::Int => f.write_str("Int"),
            Ty::Float => f.write_str("Float"),
            Ty::Str => f.write_str("String"),
            Ty::Sym => f.write_str("Symbol"),
            Ty::Money => f.write_str("Money"),
            Ty::Duration => f.write_str("Duration"),
            Ty::Range => f.write_str("Range"),
            Ty::Budget => f.write_str("Budget"),
            Ty::Array(t) => write!(f, "Array({t})"),
            Ty::Hash(k, v) => write!(f, "Hash({k}, {v})"),
            Ty::Opt(t) => write!(f, "{t}?"),
            Ty::Result(t, e) => write!(f, "Result({t}, {e})"),
            Ty::User(n) => f.write_str(n),
            Ty::Type(n) => write!(f, "type {n}"),
            Ty::Secret => f.write_str("Secret"),
        }
    }
}

/// Abstract value: a type and, if it comes from an LLM, where its taint comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct V {
    pub ty: Ty,
    pub taint: Option<Span>,
}

impl V {
    pub fn new(ty: Ty) -> V {
        V { ty, taint: None }
    }

    pub fn unknown() -> V {
        V::new(Ty::Unknown)
    }

    pub fn tainted(mut self, origin: Option<Span>) -> V {
        if self.taint.is_none() {
            self.taint = origin;
        }
        self
    }
}

/// Common type of two branches.
pub fn join(a: &Ty, b: &Ty) -> Ty {
    match (a, b) {
        (a, b) if a == b => a.clone(),
        (Ty::Unknown, _) | (_, Ty::Unknown) => Ty::Unknown,
        (Ty::Nil, t) | (t, Ty::Nil) => Ty::opt(t.base().clone()),
        (Ty::Opt(x), y) | (y, Ty::Opt(x)) if x.as_ref() == y => Ty::Opt(x.clone()),
        (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int) => Ty::Float,
        (Ty::Array(x), Ty::Array(y)) => Ty::array(join(x, y)),
        _ => Ty::Unknown,
    }
}

pub fn join_v(a: &V, b: &V) -> V {
    V { ty: join(&a.ty, &b.ty), taint: a.taint.or(b.taint) }
}
