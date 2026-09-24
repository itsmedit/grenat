//! The Grenat parser: recursive descent for statements and declarations,
//! Pratt parsing for binary operators.
//!
//! Ruby-style rules implemented here:
//! - a parenthesis-free call (`puts x`, `spawn Researcher`) is recognized when an
//!   identifier is followed, after whitespace, by a token that can start an argument;
//! - in the arguments of such a call, `do … end` belongs to the enclosing call
//!   (`within budget(usd: 1) do … end`), `{ … }` to the nearest one;
//! - `if`/`unless`/`while`/`until`/`rescue` after a statement are modifiers.
//!
//! The parser recovers from errors (by skipping to the next line) so it can
//! report several problems in one pass.

mod blocks;
mod calls;
mod control;
mod cursor;
mod docs;
mod exprs;
mod items;
mod patterns;
mod stmts;
mod types;

use grenat_ast::Program;
use grenat_lexer::lex;

pub(crate) use calls::*;
pub(crate) use cursor::*;
pub(crate) use docs::*;
pub(crate) use exprs::*;

pub use grenat_ast::Diagnostic;

#[derive(Debug)]
pub struct Parsed {
    pub program: Program,
    /// Lexer errors, then parser errors.
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse(src: &str) -> Parsed {
    let lexed = lex(src);
    let mut diagnostics: Vec<Diagnostic> =
        lexed.errors.into_iter().map(|e| Diagnostic::new(e.span, e.message)).collect();
    let docs = DocTable::new(src, &lexed.comments);
    let mut parser = Parser::new(lexed.tokens, &docs);
    let program = parser.program();
    diagnostics.extend(parser.diags);
    Parsed { program, diagnostics }
}
