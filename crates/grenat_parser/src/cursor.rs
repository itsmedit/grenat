//! The parser: token cursor, expectations, errors and recovery.

use grenat_ast::*;
use grenat_lexer::{Keyword as K, Token, TokenKind as T};

use crate::*;

pub(crate) const MAX_DIAGNOSTICS: usize = 100;

pub(crate) type PResult<T> = Result<T, ()>;

pub(crate) struct Parser<'d> {
    pub(crate) toks: Vec<Token>,
    pub(crate) pos: usize,
    pub(crate) diags: Vec<Diagnostic>,
    pub(crate) docs: &'d DocTable,
    /// True inside the arguments of a parenthesis-free call: `do` there belongs to the enclosing call.
    pub(crate) no_do: bool,
    /// True inside an index's brackets: `xs[2..]` is an endless range there.
    pub(crate) in_index: bool,
    /// The first `end` less indented than the line it closes (`end`, opener, keyword): when an
    /// `end` is missing later, it is the likely culprit and is shown with the error.
    pub(crate) misaligned_end: Option<(Span, Span, String)>,
}

impl<'d> Parser<'d> {
    pub(crate) fn new(toks: Vec<Token>, docs: &'d DocTable) -> Self {
        debug_assert!(matches!(toks.last(), Some(Token { kind: T::Eof, .. })));
        Parser { toks, pos: 0, diags: Vec::new(), docs, no_do: false, in_index: false, misaligned_end: None }
    }

    pub(crate) fn peek(&self) -> &Token {
        &self.toks[self.pos]
    }

    pub(crate) fn kind(&self) -> &T {
        &self.peek().kind
    }

    pub(crate) fn nth(&self, n: usize) -> &T {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].kind
    }

    pub(crate) fn span(&self) -> Span {
        self.peek().span
    }

    pub(crate) fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }

    pub(crate) fn bump(&mut self) -> Token {
        let tok = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    pub(crate) fn at(&self, kind: &T) -> bool {
        self.kind() == kind
    }

    /// Token glued to the previous one: `f(x)`, not `f (x)`.
    pub(crate) fn at_tight(&self, kind: &T) -> bool {
        self.at(kind) && !self.peek().space_before
    }

    pub(crate) fn at_kw(&self, kw: K) -> bool {
        *self.kind() == T::Kw(kw)
    }

    pub(crate) fn at_ident(&self, name: &str) -> bool {
        matches!(self.kind(), T::Ident(s) if s == name)
    }

    pub(crate) fn at_line_end(&self) -> bool {
        matches!(self.kind(), T::Newline | T::Eof)
    }

    pub(crate) fn eat(&mut self, kind: &T) -> bool {
        let found = self.at(kind);
        if found {
            self.bump();
        }
        found
    }

    pub(crate) fn eat_kw(&mut self, kw: K) -> bool {
        self.eat(&T::Kw(kw))
    }

    pub(crate) fn skip_newlines(&mut self) {
        while self.at(&T::Newline) {
            self.bump();
        }
    }

    pub(crate) fn recover_line(&mut self) {
        while !self.at_line_end() {
            self.bump();
        }
    }

    pub(crate) fn with_do<R>(&mut self, allowed: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.no_do;
        self.no_do = !allowed;
        let result = f(self);
        self.no_do = saved;
        result
    }

    pub(crate) fn report(&mut self, diag: Diagnostic) {
        if self.diags.len() < MAX_DIAGNOSTICS {
            self.diags.push(diag);
        }
    }

    pub(crate) fn fail<X>(&mut self, span: Span, message: impl Into<String>) -> PResult<X> {
        self.report(Diagnostic::new(span, message));
        Err(())
    }

    pub(crate) fn unexpected<X>(&mut self, expected: &str) -> PResult<X> {
        let message = format!("expected {expected}, found {}", self.kind().describe());
        self.fail(self.span(), message)
    }

    pub(crate) fn expect(&mut self, kind: T, expected: &str) -> PResult<Span> {
        if self.at(&kind) { Ok(self.bump().span) } else { self.unexpected(expected) }
    }

    pub(crate) fn expect_end(&mut self, opener: Span, what: &str) -> PResult<Span> {
        if self.at_kw(K::End) {
            let end = self.bump().span;
            if self.misaligned_end.is_none()
                && let Some(indent) = self.indent_if_first(end)
                && indent < self.line_indent(opener)
            {
                self.misaligned_end = Some((end, opener, what.to_string()));
            }
            return Ok(end);
        }
        let message = match self.kind() {
            T::Kw(kw) if is_definition(*kw) => {
                format!("expected `end` to close `{what}` before this `{}`", kw.as_str())
            }
            other => format!("expected `end` to close `{what}`, found {}", other.describe()),
        };
        let mut diag = Diagnostic::new(self.span(), message).with_note(opener, format!("`{what}` opened here"));
        if let Some((end, closed, closed_what)) = self.misaligned_end.take() {
            let line = self.docs.line_of(closed) + 1;
            let note = format!("this `end` closes the `{closed_what}` of line {line}, though it is less indented");
            diag = diag.with_note(end, note);
            diag.help = Some("an `end` may be missing above this one".into());
        }
        self.report(diag);
        Err(())
    }

    /// Indentation of the line holding `span`: the column of its first token.
    pub(crate) fn line_indent(&self, span: Span) -> usize {
        let line_start = self.docs.line_starts.get(self.docs.line_of(span)).copied().unwrap_or(0);
        let first = self.toks.partition_point(|t| (t.span.start as usize) < line_start);
        self.toks.get(first).map_or(0, |t| (t.span.start as usize).saturating_sub(line_start))
    }

    /// The indentation of `span` when it starts its line.
    pub(crate) fn indent_if_first(&self, span: Span) -> Option<usize> {
        let line_start = self.docs.line_starts.get(self.docs.line_of(span)).copied().unwrap_or(0);
        let first = self.toks.partition_point(|t| (t.span.start as usize) < line_start);
        let tok = self.toks.get(first)?;
        (tok.span == span).then(|| (span.start as usize).saturating_sub(line_start))
    }

    /// At a keyword that starts a definition (`def`, `class`, `agent`…), which no body contains:
    /// a body stops there, so that a missing `end` is reported at it, not at the end of the file.
    pub(crate) fn at_definition(&self) -> bool {
        match self.kind() {
            T::Kw(kw) => is_definition(*kw),
            T::Ident(name) => name == "native" && *self.nth(1) == T::Kw(K::Def),
            _ => false,
        }
    }

    pub(crate) fn ident(&mut self, expected: &str) -> PResult<Ident> {
        match self.kind().clone() {
            T::Ident(name) => Ok(Ident { name, span: self.bump().span }),
            _ => self.unexpected(expected),
        }
    }

    pub(crate) fn const_name(&mut self, expected: &str) -> PResult<Ident> {
        match self.kind().clone() {
            T::Const(name) => Ok(Ident { name, span: self.bump().span }),
            _ => self.unexpected(expected),
        }
    }

    /// After `.`: keywords are valid method names (`x.class`).
    pub(crate) fn method_name(&mut self) -> PResult<Ident> {
        let name = match self.kind() {
            T::Ident(s) | T::Const(s) => s.clone(),
            T::Kw(k) => k.as_str().to_string(),
            _ => return self.unexpected("a method name"),
        };
        Ok(Ident { name, span: self.bump().span })
    }
}

/// Keywords that start a top-level definition (or a member of one).
pub(crate) fn is_definition(kw: K) -> bool {
    matches!(
        kw,
        K::Def
            | K::Abstract
            | K::Prompt
            | K::Tool
            | K::Workflow
            | K::Struct
            | K::Class
            | K::Module
            | K::Enum
            | K::Agent
            | K::Supervisor
    )
}
