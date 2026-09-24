//! Grenat errors raised at run time.

use std::sync::{Arc, Mutex};

use grenat_ast::Span;

use super::*;

pub struct ErrorVal<'p> {
    pub ty: Arc<str>,
    pub message: String,
    pub fields: Fields<'p>,
    span: Mutex<Option<Span>>,
    /// Call stack: (function, call site).
    pub trace: Mutex<Vec<(String, Span)>>,
}

impl<'p> ErrorVal<'p> {
    pub fn span(&self) -> Option<Span> {
        *self.span.borrow()
    }

    pub fn set_span(&self, span: Span) {
        *self.span.borrow_mut() = Some(span);
    }

    pub fn new(ty: &str, message: impl Into<String>) -> Self {
        ErrorVal {
            ty: ty.into(),
            message: message.into(),
            fields: Vec::new(),
            span: Mutex::new(None),
            trace: Mutex::new(Vec::new()),
        }
    }
}
