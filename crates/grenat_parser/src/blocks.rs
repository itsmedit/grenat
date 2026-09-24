//! Blocs `{ |x| … }` et `do |x| … end`.

use grenat_ast::*;
use grenat_lexer::TokenKind as T;

use crate::*;

impl<'d> Parser<'d> {
    pub(crate) fn brace_block(&mut self) -> PResult<Block> {
        let start = self.bump().span;
        let params = self.block_params()?;
        let body_start = self.span();
        let stmts = self.stmts(&[]);
        let end = self.expect(T::RBrace, "`}`")?;
        Ok(Block { params, body: Body::new(stmts, body_start.to(self.prev_span())), span: start.to(end) })
    }

    pub(crate) fn do_block(&mut self) -> PResult<Block> {
        let start = self.bump().span;
        let params = self.block_params()?;
        let body = self.body(&[])?;
        let end = self.expect_end(start, "do")?;
        Ok(Block { params, body, span: start.to(end) })
    }

    /// `|a, b|`, `|x: Int|` ou rien.
    pub(crate) fn block_params(&mut self) -> PResult<Vec<Param>> {
        if self.eat(&T::OrOr) || !self.eat(&T::Pipe) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        loop {
            let start = self.span();
            let (name, ty) = match self.kind().clone() {
                T::Ident(name) => {
                    self.bump();
                    (name, None)
                }
                T::Label(name) => {
                    self.bump();
                    (name, Some(self.ty()?))
                }
                _ => return self.unexpected("un paramètre de bloc"),
            };
            params.push(Param {
                name: Ident { name, span: start },
                ty,
                default: None,
                span: start.to(self.prev_span()),
            });
            if !self.eat(&T::Comma) {
                break;
            }
        }
        self.expect(T::Pipe, "`|`")?;
        Ok(params)
    }
}
