//! Contrôle d'exécution (exceptions, `return`, `break`…) et arguments d'appel.

use std::sync::Arc;

use crate::value::{ErrorVal, Value};

pub(crate) enum Ctrl<'p> {
    Raise(Arc<ErrorVal<'p>>),
    Return(Value<'p>),
    Break(Value<'p>),
    Next(Value<'p>),
    Exit(i32),
}

impl Ctrl<'_> {
    /// Erreur Grenat portée par ce contrôle, s'il s'agit d'une exception.
    pub fn error_type(&self) -> Option<&str> {
        match self {
            Ctrl::Raise(e) => Some(&e.ty),
            _ => None,
        }
    }
}

pub(crate) type R<'p> = Result<Value<'p>, Ctrl<'p>>;

pub(crate) fn raise<'p, T>(ty: &str, message: impl Into<String>) -> Result<T, Ctrl<'p>> {
    Err(Ctrl::Raise(Arc::new(ErrorVal::new(ty, message))))
}

#[derive(Default)]
pub(crate) struct Args<'p> {
    pub pos: Vec<Value<'p>>,
    pub named: Vec<(String, Value<'p>)>,
    pub block: Option<Value<'p>>,
}

impl Args<'_> {
    pub fn is_empty(&self) -> bool {
        self.pos.is_empty() && self.named.is_empty() && self.block.is_none()
    }
}
