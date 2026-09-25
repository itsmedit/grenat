//! Values handled by the interpreter.
//!
//! `'p` is the lifetime of the program: closures point straight at the AST's
//! blocks, without copying them.

mod agent;
mod budget;
pub mod codec;
mod display;
mod equality;
mod error;
mod locked;
mod scope;

use std::sync::{Arc, Mutex};

use grenat_ast::Body;

pub use agent::*;
pub use budget::*;
pub use display::*;
pub use equality::*;
pub use error::*;
pub use locked::*;
pub use scope::*;

pub type Fields<'p> = Vec<(Arc<str>, Value<'p>)>;

pub fn field<'a, 'p>(fields: &'a Fields<'p>, name: &str) -> Option<&'a Value<'p>> {
    fields.iter().find(|(n, _)| &**n == name).map(|(_, v)| v)
}

#[derive(Clone)]
pub enum Value<'p> {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Amount in dollars (LLM costs).
    Money(f64),
    /// Duration in seconds (`10.min`).
    Duration(f64),
    Str(Arc<str>),
    Symbol(Arc<str>),
    Array(Arc<Mutex<Vec<Value<'p>>>>),
    Hash(Arc<Mutex<Vec<(Value<'p>, Value<'p>)>>>),
    Range(i64, i64, bool),
    /// `struct` instance or agent message: immutable value.
    Record(Arc<Record<'p>>),
    /// `class` or `agent` instance: mutable reference.
    Object(Arc<Object<'p>>),
    /// `enum` variant (including `Ok` / `Err`).
    Variant(Arc<Variant<'p>>),
    Closure(Arc<Closure<'p>>),
    /// A type or module used as a value (`Researcher`, `File`).
    Type(Arc<str>),
    Error(Arc<ErrorVal<'p>>),
    Budget(Arc<Budget>),
    /// Reference to an actor agent (`spawn Writer`).
    Agent(Arc<AgentRef<'p>>),
    /// Group of interchangeable agents (`spawn_pool(Writer, size: 4)`).
    Pool(Arc<Vec<Arc<AgentRef<'p>>>>),
    /// Produced by an LLM, not validated: `~T`.
    Tainted(Arc<Value<'p>>),
}

pub struct Record<'p> {
    pub ty: Arc<str>,
    pub fields: Fields<'p>,
}

pub struct Object<'p> {
    pub ty: Arc<str>,
    pub fields: Mutex<Fields<'p>>,
    pub is_agent: bool,
}

pub struct Variant<'p> {
    pub enum_name: Arc<str>,
    pub name: Arc<str>,
    pub fields: Fields<'p>,
}

pub struct Closure<'p> {
    pub params: Vec<String>,
    pub body: &'p Body,
    pub scope: Scope<'p>,
    pub self_val: Option<Value<'p>>,
    /// Block called on a tainted value: its arguments are tainted too.
    pub taint_args: bool,
}

impl<'p> Value<'p> {
    pub fn str(s: impl AsRef<str>) -> Self {
        Value::Str(s.as_ref().into())
    }

    pub fn array(items: Vec<Value<'p>>) -> Self {
        Value::Array(Arc::new(Mutex::new(items)))
    }

    pub fn record(ty: &str, fields: Fields<'p>) -> Self {
        Value::Record(Arc::new(Record { ty: ty.into(), fields }))
    }

    pub fn ok(value: Value<'p>) -> Self {
        Value::Variant(Arc::new(Variant {
            enum_name: "Result".into(),
            name: "Ok".into(),
            fields: vec![("value".into(), value)],
        }))
    }

    pub fn err(error: Value<'p>) -> Self {
        Value::Variant(Arc::new(Variant {
            enum_name: "Result".into(),
            name: "Err".into(),
            fields: vec![("error".into(), error)],
        }))
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Nil | Value::Bool(false) => false,
            Value::Tainted(inner) => inner.truthy(),
            _ => true,
        }
    }

    pub fn is_tainted(&self) -> bool {
        matches!(self, Value::Tainted(_))
    }

    /// Taint anywhere inside the value (arrays, fields…).
    pub fn contains_taint(&self) -> bool {
        match self {
            Value::Tainted(_) => true,
            Value::Array(items) => items.borrow().iter().any(Value::contains_taint),
            Value::Hash(entries) => entries.borrow().iter().any(|(k, v)| k.contains_taint() || v.contains_taint()),
            Value::Record(r) => r.fields.iter().any(|(_, v)| v.contains_taint()),
            Value::Variant(v) => v.fields.iter().any(|(_, v)| v.contains_taint()),
            _ => false,
        }
    }

    pub fn untainted(&self) -> &Value<'p> {
        match self {
            Value::Tainted(inner) => inner.untainted(),
            other => other,
        }
    }

    pub fn taint(self) -> Self {
        match self {
            Value::Tainted(_) | Value::Nil => self,
            other => Value::Tainted(Arc::new(other)),
        }
    }
}
