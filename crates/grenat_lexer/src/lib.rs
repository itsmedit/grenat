//! The Grenat lexer.
//!
//! Hand-written rather than generated: `"#{…}"` interpolation and `<<~ID`
//! heredocs need modes that a regular lexer handles poorly.
//!
//! Ruby-style conventions handled here:
//! - `name:` (no space) is a **label** (named argument, field); `:name` is a **symbol**;
//! - a trailing `?` or `!` on a lowercase identifier is part of the name (`empty?`, `save!`);
//! - a newline followed by `.` or `&.` continues the expression (multi-line chaining);
//! - comments are removed from the token stream but kept aside
//!   (`##` comments are documentation, passed on to LLMs).

mod lexer;
mod strings;
mod token;

pub use lexer::lex;
pub(crate) use lexer::*;
pub(crate) use strings::*;
pub use token::*;

#[cfg(test)]
mod tests;
