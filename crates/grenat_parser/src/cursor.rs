//! Le parser : curseur sur les tokens, attentes, erreurs et récupération.

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
    /// Vrai dans les arguments d'un appel sans parenthèses : `do` y appartient à l'appel englobant.
    pub(crate) no_do: bool,
}

impl<'d> Parser<'d> {
    pub(crate) fn new(toks: Vec<Token>, docs: &'d DocTable) -> Self {
        debug_assert!(matches!(toks.last(), Some(Token { kind: T::Eof, .. })));
        Parser { toks, pos: 0, diags: Vec::new(), docs, no_do: false }
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

    /// Token collé au précédent : `f(x)` et non `f (x)`.
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
        let message = format!("attendu : {expected} ; trouvé : {}", self.kind().describe());
        self.fail(self.span(), message)
    }

    pub(crate) fn expect(&mut self, kind: T, expected: &str) -> PResult<Span> {
        if self.at(&kind) { Ok(self.bump().span) } else { self.unexpected(expected) }
    }

    pub(crate) fn expect_end(&mut self, opener: Span, what: &str) -> PResult<Span> {
        if self.at_kw(K::End) {
            return Ok(self.bump().span);
        }
        let message = format!("`end` attendu pour fermer `{what}` ; trouvé : {}", self.kind().describe());
        self.report(Diagnostic::new(self.span(), message).with_note(opener, format!("`{what}` ouvert ici")));
        Err(())
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

    /// Après `.` : les mots-clés sont des noms de méthode valides (`x.class`).
    pub(crate) fn method_name(&mut self) -> PResult<Ident> {
        let name = match self.kind() {
            T::Ident(s) | T::Const(s) => s.clone(),
            T::Kw(k) => k.as_str().to_string(),
            _ => return self.unexpected("un nom de méthode"),
        };
        Ok(Ident { name, span: self.bump().span })
    }
}
