//! `grenat fmt`: the canonical layout of Grenat code.
//!
//! The program is printed back from its syntax tree: two-space indentation,
//! spaces around operators, the parentheses the grammar needs and no more,
//! one blank line at most, long arrays, hashes and argument lists broken one
//! item per line. What the syntax leaves to the author is kept as written:
//! literals (`2_000_000`, escapes), modifiers (`x if c`), `unless`, `{ }` or
//! `do … end` blocks, one-line `if … then … end`. Comments stay above the
//! code they preceded, or at the end of its line.
//!
//! Formatting never changes a program: the result must parse to the same
//! tree (positions aside), keep every comment, and be stable (formatting it
//! again changes nothing). Otherwise the source is left untouched.

mod exprs;
mod items;
mod printer;
mod stmts;
mod verify;

use printer::Printer;

/// Why a file was not formatted.
#[derive(Debug, Clone, PartialEq)]
pub enum FmtError {
    /// The source does not parse (see `grenat check`).
    Syntax,
    /// The formatter would have changed the program: a bug, reported as such.
    Unsafe(String),
}

/// The formatted source.
pub fn format(src: &str) -> Result<String, FmtError> {
    let out = print(src)?;
    verify::same_program(src, &out).map_err(FmtError::Unsafe)?;
    let again = print(&out)?;
    if again != out {
        return Err(FmtError::Unsafe("formatting is not stable".into()));
    }
    Ok(out)
}

/// The printed source, without the safety checks (for debugging the formatter).
#[doc(hidden)]
pub fn print_unchecked(src: &str) -> Result<String, FmtError> {
    print(src)
}

fn print(src: &str) -> Result<String, FmtError> {
    let parsed = grenat_parser::parse(src);
    if !parsed.diagnostics.is_empty() {
        return Err(FmtError::Syntax);
    }
    let comments = grenat_lexer::lex(src).comments;
    let mut printer = Printer::new(src, comments);
    printer.program(&parsed.program);
    Ok(printer.finish())
}

#[cfg(test)]
mod tests;
