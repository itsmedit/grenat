//! Motifs de `case … in`.

use grenat_ast::*;
use grenat_lexer::{Keyword as K, TokenKind as T};

use crate::*;

impl<'d> Parser<'d> {
    pub(crate) fn pattern(&mut self) -> PResult<Pattern> {
        let first = self.pattern_atom()?;
        if !self.at(&T::Pipe) {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat(&T::Pipe) {
            alts.push(self.pattern_atom()?);
        }
        let span = alts[0].span.to(alts[alts.len() - 1].span);
        Ok(Pattern { kind: PatternKind::Or(alts), span })
    }

    pub(crate) fn pattern_atom(&mut self) -> PResult<Pattern> {
        let start = self.span();
        let kind = match self.kind().clone() {
            T::Ident(name) => {
                self.bump();
                if name == "_" { PatternKind::Wildcard } else { PatternKind::Bind(name) }
            }
            T::Int(_) | T::Float(_) | T::Str(_) | T::Symbol(_) | T::Kw(K::True | K::False | K::Nil) => {
                PatternKind::Lit(self.primary()?)
            }
            T::Minus => PatternKind::Lit(self.unary()?),
            T::Const(_) => {
                let mut path = vec![self.const_name("une constante")?];
                while self.eat(&T::ColonColon) {
                    path.push(self.const_name("une constante")?);
                }
                let fields = if self.at_tight(&T::LParen) { Some(self.pattern_fields()?) } else { None };
                PatternKind::Const { path, fields }
            }
            T::LBracket => {
                self.bump();
                let mut items = Vec::new();
                loop {
                    self.skip_newlines();
                    if self.at(&T::RBracket) {
                        break;
                    }
                    items.push(self.pattern()?);
                    if !self.eat(&T::Comma) {
                        break;
                    }
                }
                self.skip_newlines();
                self.expect(T::RBracket, "`]`")?;
                PatternKind::Array(items)
            }
            _ => return self.unexpected("un motif"),
        };
        Ok(Pattern { kind, span: start.to(self.prev_span()) })
    }

    /// `(r)`, `(w, h)`, `(category: Spam)`, `(category:)`
    pub(crate) fn pattern_fields(&mut self) -> PResult<Vec<PatField>> {
        self.bump();
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&T::RParen) {
                break;
            }
            if let T::Label(name) = self.kind().clone() {
                let span = self.bump().span;
                let pattern = if matches!(self.kind(), T::Comma | T::RParen) {
                    Pattern { kind: PatternKind::Bind(name.clone()), span }
                } else {
                    self.pattern()?
                };
                fields.push(PatField { name: Some(Ident { name, span }), pattern });
            } else {
                fields.push(PatField { name: None, pattern: self.pattern()? });
            }
            self.skip_newlines();
            if !self.eat(&T::Comma) {
                break;
            }
        }
        self.skip_newlines();
        self.expect(T::RParen, "`)`")?;
        Ok(fields)
    }
}
