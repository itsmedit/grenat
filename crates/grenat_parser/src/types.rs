//! Type annotations: `Array(T)`, `T?`, `~T`, and sizes (`Vector(1536)`).

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
        let mut path = vec![self.const_name("a type")?];
        while self.eat(&T::ColonColon) {
            path.push(self.const_name("a type")?);
        }
        let mut args = Vec::new();
        if self.at_tight(&T::LParen) {
            self.bump();
            loop {
                self.skip_newlines();
                if self.at(&T::RParen) {
                    break;
                }
                args.push(self.ty_arg()?);
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

    /// A type's argument: a type, or a size (`Vector(1536)`).
    fn ty_arg(&mut self) -> PResult<Type> {
        if let T::Int(n) = *self.kind() {
            let span = self.bump().span;
            // the lexer reads no sign: an integer literal is never negative
            return Ok(Type::Size(n.unsigned_abs(), span));
        }
        self.ty()
    }
}
