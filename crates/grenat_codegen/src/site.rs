//! Sites: where native code stops early, for error messages.
//!
//! Every trap and deoptimization gets a site number, written into the call's
//! context when it is taken; a standalone executable (which has no
//! interpreter to run the call again) reports the error at that site.

use grenat_ast::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    /// The compiled function, and where it is defined.
    pub function: String,
    pub function_span: Span,
    /// The expression being evaluated.
    pub span: Span,
    /// For a deoptimization: what native code cannot represent.
    pub reason: Option<&'static str>,
}
