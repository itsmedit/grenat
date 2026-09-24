//! Statements: bodies, `rescue`, modifiers, multiple assignment, `and`/`or`/`not`.

use grenat_ast::*;
use grenat_lexer::{Keyword as K, TokenKind as T};

use crate::*;

impl<'d> Parser<'d> {
    /// Statements, then `rescue`/`ensure` clauses; does not consume the `end`.
    pub(crate) fn body(&mut self, stops: &[K]) -> PResult<Body> {
        let start = self.span();
        let mut all_stops = vec![K::Rescue, K::Ensure, K::End];
        all_stops.extend_from_slice(stops);
        let stmts = self.stmts(&all_stops);
        let mut rescues = Vec::new();
        while self.at_kw(K::Rescue) {
            rescues.push(self.rescue_clause()?);
        }
        let ensure = if self.eat_kw(K::Ensure) { Some(self.stmts(&[K::End])) } else { None };
        Ok(Body { stmts, rescues, ensure, span: start.to(self.prev_span()) })
    }

    pub(crate) fn stmts(&mut self, stops: &[K]) -> Vec<Expr> {
        self.with_do(true, |p| {
            let mut out = Vec::new();
            loop {
                p.skip_newlines();
                match p.kind() {
                    T::Kw(k) if stops.contains(k) => break,
                    T::RBrace | T::RParen | T::Eof => break,
                    _ if p.diags.len() >= MAX_DIAGNOSTICS => break,
                    _ => {}
                }
                let before = p.pos;
                match p.stmt() {
                    Ok(e) => {
                        out.push(e);
                        let at_end = matches!(p.kind(), T::Newline | T::Eof | T::RBrace)
                            || matches!(p.kind(), T::Kw(k) if stops.contains(k));
                        if !at_end {
                            let _ = p.unexpected::<()>("end of line");
                            p.recover_line();
                        }
                    }
                    Err(()) => p.recover_line(),
                }
                if p.pos == before {
                    p.bump();
                }
            }
            out
        })
    }

    pub(crate) fn rescue_clause(&mut self) -> PResult<Rescue> {
        let start = self.bump().span;
        let mut types = Vec::new();
        if matches!(self.kind(), T::Const(_)) {
            types.push(self.ty()?);
            while self.eat(&T::Comma) {
                types.push(self.ty()?);
            }
        }
        let binding = if self.eat(&T::FatArrow) { Some(self.ident("a variable name")?) } else { None };
        self.eat_kw(K::Then);
        let body = self.stmts(&[K::Rescue, K::Ensure, K::End]);
        Ok(Rescue { types, binding, body, span: start.to(self.prev_span()) })
    }

    /// Statement: an expression followed by optional modifiers (`x if y`).
    pub(crate) fn stmt(&mut self) -> PResult<Expr> {
        let mut e = match self.multi_assign()? {
            Some(e) => e,
            None => self.expr_stmt()?,
        };
        while let T::Kw(kw @ (K::If | K::Unless | K::While | K::Until | K::Rescue)) = *self.kind() {
            let kw_span = self.bump().span;
            let rhs = self.expr_stmt()?;
            let span = e.span.to(rhs.span);
            let kind = match kw {
                K::If => ExprKind::If { cond: Box::new(rhs), then: vec![e], else_: None },
                K::Unless => ExprKind::If { cond: Box::new(not(rhs)), then: vec![e], else_: None },
                K::While => ExprKind::While { cond: Box::new(rhs), body: vec![e] },
                K::Until => ExprKind::While { cond: Box::new(not(rhs)), body: vec![e] },
                _ => {
                    let rescue =
                        Rescue { types: Vec::new(), binding: None, span: kw_span.to(rhs.span), body: vec![rhs] };
                    ExprKind::Begin(Body { stmts: vec![e], rescues: vec![rescue], ensure: None, span })
                }
            };
            e = Expr::new(kind, span);
        }
        Ok(e)
    }

    /// `a, b = value`
    pub(crate) fn multi_assign(&mut self) -> PResult<Option<Expr>> {
        let mut i = 0;
        loop {
            if !matches!(self.nth(i), T::Ident(_) | T::IVar(_)) {
                return Ok(None);
            }
            match self.nth(i + 1) {
                T::Comma => i += 2,
                T::Eq if i > 0 => break,
                _ => return Ok(None),
            }
        }
        let start = self.span();
        let mut targets = Vec::new();
        loop {
            let tok = self.bump();
            let kind = match tok.kind {
                T::Ident(name) => ExprKind::Var(name),
                T::IVar(name) => ExprKind::IVar(name),
                _ => unreachable!(),
            };
            targets.push(Expr::new(kind, tok.span));
            if self.bump().kind == T::Eq {
                break;
            }
        }
        self.skip_newlines();
        let value = self.expr()?;
        let span = start.to(value.span);
        Ok(Some(Expr::new(ExprKind::MultiAssign { targets, value: Box::new(value) }, span)))
    }

    /// The `and` / `or` / `not` level (lower than assignment, as in Ruby).
    pub(crate) fn expr_stmt(&mut self) -> PResult<Expr> {
        let mut lhs = self.not_expr()?;
        loop {
            let op = if self.at_kw(K::And) {
                BinOp::And
            } else if self.at_kw(K::Or) {
                BinOp::Or
            } else {
                break;
            };
            self.bump();
            self.skip_newlines();
            let rhs = self.not_expr()?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr::new(ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span);
        }
        Ok(lhs)
    }

    pub(crate) fn not_expr(&mut self) -> PResult<Expr> {
        if self.at_kw(K::Not) {
            let start = self.bump().span;
            let e = self.not_expr()?;
            let span = start.to(e.span);
            return Ok(Expr { span, ..not(e) });
        }
        self.expr()
    }
}
