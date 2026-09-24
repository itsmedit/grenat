//! Annotations de type : `Array(T)`, `T?`, `~T`.

use grenat_ast::*;
use grenat_lexer::TokenKind as T;

use crate::*;

impl<'d> Parser<'d> {
    pub(crate) fn ty(&mut self) -> PResult<Type> {
        if self.at(&T::Tilde) {
            let start = self.bump().span;
            let inner = self.ty()?;
            let span = start.to(inner.span());
            return Ok(Type::Tainted(Box::new(inner), span));
        }
        let start = self.span();
        let mut path = vec![self.const_name("un type")?];
        while self.eat(&T::ColonColon) {
            path.push(self.const_name("un type")?);
        }
        let mut args = Vec::new();
        if self.at_tight(&T::LParen) {
            self.bump();
            loop {
                self.skip_newlines();
                if self.at(&T::RParen) {
                    break;
                }
                args.push(self.ty()?);
                self.skip_newlines();
                if !self.eat(&T::Comma) {
                    break;
                }
            }
            self.skip_newlines();
            self.expect(T::RParen, "`)`")?;
        }
        let mut ty = Type::Named { path, args, span: start.to(self.prev_span()) };
        while self.at_tight(&T::Question) {
            let span = ty.span().to(self.bump().span);
            ty = Type::Optional(Box::new(ty), span);
        }
        Ok(ty)
    }
}
