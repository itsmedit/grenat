//! Structures de contrôle : `if`/`unless`, `while`/`until`, `case`.

use grenat_ast::*;
use grenat_lexer::{Keyword as K, TokenKind as T};

use crate::*;

impl<'d> Parser<'d> {
    pub(crate) fn if_expr(&mut self, negate: bool) -> PResult<Expr> {
        let start = self.bump().span;
        let e = self.if_rest(start, negate)?;
        let end = self.expect_end(start, if negate { "unless" } else { "if" })?;
        Ok(Expr { span: start.to(end), ..e })
    }

    /// Condition, branche `then`, puis `elsif`/`else` ; ne consomme pas le `end`.
    pub(crate) fn if_rest(&mut self, start: Span, negate: bool) -> PResult<Expr> {
        let cond = self.expr_stmt()?;
        let cond = if negate { not(cond) } else { cond };
        self.eat_kw(K::Then);
        let then = self.stmts(&[K::Elsif, K::Else, K::End]);
        let else_ = if self.at_kw(K::Elsif) {
            let elsif = self.bump().span;
            Some(vec![self.if_rest(elsif, false)?])
        } else if self.eat_kw(K::Else) {
            Some(self.stmts(&[K::End]))
        } else {
            None
        };
        let kind = ExprKind::If { cond: Box::new(cond), then, else_ };
        Ok(Expr::new(kind, start.to(self.prev_span())))
    }

    pub(crate) fn while_expr(&mut self, until: bool) -> PResult<Expr> {
        let start = self.bump().span;
        let cond = self.with_do(false, |p| p.expr_stmt())?;
        let cond = if until { not(cond) } else { cond };
        self.eat_kw(K::Do);
        let body = self.stmts(&[K::End]);
        let end = self.expect_end(start, if until { "until" } else { "while" })?;
        Ok(Expr::new(ExprKind::While { cond: Box::new(cond), body }, start.to(end)))
    }

    pub(crate) fn case_expr(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let subject = if self.at_line_end() { None } else { Some(Box::new(self.expr_stmt()?)) };
        self.skip_newlines();

        let mut arms = Vec::new();
        loop {
            let arm_start = self.span();
            let test = if self.eat_kw(K::In) {
                ArmTest::In(self.pattern()?)
            } else if self.eat_kw(K::When) {
                let mut values = vec![self.expr()?];
                while self.eat(&T::Comma) {
                    self.skip_newlines();
                    values.push(self.expr()?);
                }
                ArmTest::When(values)
            } else {
                break;
            };
            let guard = if self.eat_kw(K::If) {
                Some(self.expr_stmt()?)
            } else if self.eat_kw(K::Unless) {
                Some(not(self.expr_stmt()?))
            } else {
                None
            };
            self.eat_kw(K::Then);
            let body = self.stmts(&[K::In, K::When, K::Else, K::End]);
            arms.push(CaseArm { test, guard, body, span: arm_start.to(self.prev_span()) });
        }
        if arms.is_empty() {
            self.report(Diagnostic::new(self.span(), "au moins une branche `in` ou `when` attendue"));
        }
        let else_ = if self.eat_kw(K::Else) { Some(self.stmts(&[K::End])) } else { None };
        let end = self.expect_end(start, "case")?;
        Ok(Expr::new(ExprKind::Case { subject, arms, else_ }, start.to(end)))
    }
}
