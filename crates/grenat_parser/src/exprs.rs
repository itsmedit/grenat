//! Expressions: assignment, operators (Pratt), postfix forms, primaries, strings.

use grenat_ast::*;
use grenat_lexer::{Keyword as K, StrPart, Token, TokenKind as T};

use crate::*;

pub(crate) enum Infix {
    Bin(BinOp),
    Range(bool),
}

pub(crate) fn not(e: Expr) -> Expr {
    let span = e.span;
    Expr::new(ExprKind::Unary { op: UnOp::Not, expr: Box::new(e) }, span)
}

pub(crate) fn is_assignable(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Var(_) | ExprKind::IVar(_) | ExprKind::Const(_) | ExprKind::Index { .. } => true,
        ExprKind::Call { recv: Some(_), args, block: None, parens: false, .. } => args.is_empty(),
        _ => false,
    }
}

impl<'d> Parser<'d> {
    /// Assignment (right-associative), then binary operators.
    pub(crate) fn expr(&mut self) -> PResult<Expr> {
        let lhs = self.binary(0)?;
        let op = match self.kind() {
            T::Eq => None,
            T::PlusEq => Some(BinOp::Add),
            T::MinusEq => Some(BinOp::Sub),
            T::StarEq => Some(BinOp::Mul),
            T::SlashEq => Some(BinOp::Div),
            T::OrOrEq => Some(BinOp::Or),
            T::AndAndEq => Some(BinOp::And),
            _ => return Ok(lhs),
        };
        if !is_assignable(&lhs) {
            return self.fail(lhs.span, "this expression cannot be assigned to");
        }
        self.bump();
        self.skip_newlines();
        let value = self.expr()?;
        let span = lhs.span.to(value.span);
        let (target, value) = (Box::new(lhs), Box::new(value));
        let kind = match op {
            None => ExprKind::Assign { target, value },
            Some(op) => ExprKind::OpAssign { op, target, value },
        };
        Ok(Expr::new(kind, span))
    }

    /// (operator, left binding power, right binding power) — Pratt.
    pub(crate) fn infix(&self) -> Option<(Infix, u8, u8)> {
        use BinOp::*;
        let bin = |op, l, r| Some((Infix::Bin(op), l, r));
        match self.kind() {
            T::DotDot => Some((Infix::Range(true), 2, 3)),
            T::DotDotDot => Some((Infix::Range(false), 2, 3)),
            T::OrOr => bin(Or, 4, 5),
            T::AndAnd => bin(And, 6, 7),
            T::EqEq => bin(Eq, 8, 9),
            T::NotEq => bin(NotEq, 8, 9),
            T::Match => bin(Match, 8, 9),
            T::Cmp => bin(Cmp, 8, 9),
            T::Lt => bin(Lt, 10, 11),
            T::Le => bin(Le, 10, 11),
            T::Gt => bin(Gt, 10, 11),
            T::Ge => bin(Ge, 10, 11),
            T::Pipe => bin(BitOr, 12, 13),
            T::Caret => bin(BitXor, 12, 13),
            T::Amp => bin(BitAnd, 14, 15),
            T::Shl => bin(Shl, 16, 17),
            T::Shr => bin(Shr, 16, 17),
            T::Plus => bin(Add, 18, 19),
            T::Minus => bin(Sub, 18, 19),
            T::Star => bin(Mul, 20, 21),
            T::Slash => bin(Div, 20, 21),
            T::Percent => bin(Rem, 20, 21),
            T::StarStar => bin(Pow, 25, 24),
            _ => None,
        }
    }

    pub(crate) fn binary(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        while let Some((op, left_bp, right_bp)) = self.infix() {
            if left_bp < min_bp {
                break;
            }
            self.bump();
            self.skip_newlines();
            let rhs = self.binary(right_bp)?;
            let span = lhs.span.to(rhs.span);
            let (lhs_box, rhs_box) = (Box::new(lhs), Box::new(rhs));
            let kind = match op {
                Infix::Bin(op) => ExprKind::Binary { op, lhs: lhs_box, rhs: rhs_box },
                Infix::Range(inclusive) => ExprKind::Range { lo: lhs_box, hi: rhs_box, inclusive },
            };
            lhs = Expr::new(kind, span);
        }
        Ok(lhs)
    }

    pub(crate) fn unary(&mut self) -> PResult<Expr> {
        let op = match self.kind() {
            T::Minus => UnOp::Neg,
            T::Bang => UnOp::Not,
            _ => return self.postfix(),
        };
        let start = self.bump().span;
        // `-3.abs`: a glued negative literal, as in Ruby (but `-2 ** 2` is -(2 ** 2))
        if op == UnOp::Neg && !self.peek().space_before && *self.nth(1) != T::StarStar {
            let literal = match self.kind() {
                T::Int(n) => n.checked_neg().map(ExprKind::Int),
                T::Float(f) => Some(ExprKind::Float(-f)),
                _ => None,
            };
            if let Some(kind) = literal {
                let span = start.to(self.bump().span);
                return self.postfix_from(Expr::new(kind, span));
            }
        }
        let e = if op == UnOp::Neg { self.binary(24)? } else { self.unary()? };
        let span = start.to(e.span);
        Ok(Expr::new(ExprKind::Unary { op, expr: Box::new(e) }, span))
    }

    pub(crate) fn postfix(&mut self) -> PResult<Expr> {
        let e = self.primary()?;
        self.postfix_from(e)
    }

    pub(crate) fn postfix_from(&mut self, mut e: Expr) -> PResult<Expr> {
        loop {
            match self.kind() {
                T::Dot | T::SafeDot => {
                    let safe = self.bump().kind == T::SafeDot;
                    e = self.method_call(e, safe)?;
                }
                T::LBracket if !self.peek().space_before => {
                    self.bump();
                    let args = self.expr_list(T::RBracket, "`]`")?;
                    let span = e.span.to(self.prev_span());
                    e = Expr::new(ExprKind::Index { recv: Box::new(e), args }, span);
                }
                T::Question if !self.peek().space_before => {
                    let span = e.span.to(self.bump().span);
                    e = Expr::new(ExprKind::Try(Box::new(e)), span);
                }
                T::LBrace if takes_block(&e) => {
                    let block = self.brace_block()?;
                    e = attach_block(e, block);
                }
                T::Kw(K::Do) if !self.no_do && takes_block(&e) => {
                    let block = self.do_block()?;
                    e = attach_block(e, block);
                }
                _ => break,
            }
        }
        Ok(e)
    }

    pub(crate) fn primary(&mut self) -> PResult<Expr> {
        let tok = self.peek().clone();
        let simple = |p: &mut Self, kind| {
            p.bump();
            Ok(Expr::new(kind, tok.span))
        };
        match tok.kind {
            T::Int(n) => simple(self, ExprKind::Int(n)),
            T::Float(n) => simple(self, ExprKind::Float(n)),
            T::Symbol(ref s) => simple(self, ExprKind::Symbol(s.clone())),
            T::IVar(ref s) => simple(self, ExprKind::IVar(s.clone())),
            T::Kw(K::True) => simple(self, ExprKind::Bool(true)),
            T::Kw(K::False) => simple(self, ExprKind::Bool(false)),
            T::Kw(K::Nil) => simple(self, ExprKind::Nil),
            T::Kw(K::SelfKw) => simple(self, ExprKind::SelfRef),
            T::Str(parts) => {
                self.bump();
                Ok(self.string(parts, tok.span))
            }
            T::Ident(_) => self.ident_expr(),
            T::Const(_) => self.const_expr(),
            T::LParen => {
                self.bump();
                let inner = self.with_do(true, |p| {
                    p.skip_newlines();
                    let e = p.stmt()?;
                    p.skip_newlines();
                    p.expect(T::RParen, "`)`")?;
                    Ok(e)
                })?;
                Ok(Expr { span: tok.span.to(self.prev_span()), ..inner })
            }
            T::LBracket => {
                self.bump();
                let items = self.expr_list(T::RBracket, "`]`")?;
                Ok(Expr::new(ExprKind::Array(items), tok.span.to(self.prev_span())))
            }
            T::LBrace => self.hash(),
            T::Kw(K::If) => self.if_expr(false),
            T::Kw(K::Unless) => self.if_expr(true),
            T::Kw(K::While) => self.while_expr(false),
            T::Kw(K::Until) => self.while_expr(true),
            T::Kw(K::Case) => self.case_expr(),
            T::Kw(K::Begin) => {
                self.bump();
                let body = self.body(&[])?;
                let end = self.expect_end(tok.span, "begin")?;
                Ok(Expr::new(ExprKind::Begin(body), tok.span.to(end)))
            }
            T::Kw(K::Return | K::Break | K::Next) => self.jump(),
            T::Kw(K::End) => self.fail(tok.span, "`end` without an opening block"),
            T::Kw(
                kw @ (K::Def
                | K::Abstract
                | K::Prompt
                | K::Tool
                | K::Workflow
                | K::Struct
                | K::Class
                | K::Module
                | K::Enum
                | K::Agent
                | K::Supervisor),
            ) => self.fail(tok.span, format!("`{}` is only allowed at the top level", kw.as_str())),
            T::Label(name) => self.fail(tok.span, format!("unexpected named argument `{name}:`")),
            _ => self.unexpected("an expression"),
        }
    }

    pub(crate) fn string(&mut self, parts: Vec<StrPart>, span: Span) -> Expr {
        let segs = parts
            .into_iter()
            .map(|part| match part {
                StrPart::Lit(text) => StrSeg::Lit(text),
                StrPart::Interp(tokens, span) => StrSeg::Interp(self.interpolation(tokens, span)),
            })
            .collect();
        Expr::new(ExprKind::Str(segs), span)
    }

    pub(crate) fn interpolation(&mut self, tokens: Vec<Token>, span: Span) -> Expr {
        let mut sub = Parser::new(tokens, self.docs);
        let mut stmts = sub.stmts(&[]);
        if !sub.at(&T::Eof) {
            let _ = sub.unexpected::<()>("`}`");
        }
        self.diags.append(&mut sub.diags);
        if stmts.len() > 1 {
            self.report(Diagnostic::new(span, "expected a single expression in `#{…}`"));
        }
        stmts.pop().unwrap_or_else(|| Expr::new(ExprKind::Str(Vec::new()), span))
    }

    pub(crate) fn ident_expr(&mut self) -> PResult<Expr> {
        let tok = self.bump();
        let T::Ident(name) = tok.kind else { unreachable!() };
        let name = Ident { name, span: tok.span };
        let (args, block, parens) = if self.at_tight(&T::LParen) {
            let (args, block) = self.paren_args()?;
            (args, block, true)
        } else if self.can_start_command_arg() {
            let (args, block) = self.command_args()?;
            (args, block, false)
        } else {
            return Ok(Expr::new(ExprKind::Var(name.name), tok.span));
        };
        let span = tok.span.to(self.prev_span());
        Ok(Expr::new(ExprKind::Call { recv: None, name, args, block: block.map(Box::new), safe: false, parens }, span))
    }

    /// `Foo`, `A::B`, or `Research(topic: t)` (construction / type call).
    pub(crate) fn const_expr(&mut self) -> PResult<Expr> {
        let start = self.span();
        let mut path = vec![self.const_name("a constant")?];
        while self.at(&T::ColonColon) && matches!(self.nth(1), T::Const(_)) {
            self.bump();
            path.push(self.const_name("a constant")?);
        }
        if !self.at_tight(&T::LParen) {
            return Ok(Expr::new(ExprKind::Const(path), start.to(self.prev_span())));
        }
        let name = path.pop().expect("non-empty path");
        let recv = path.last().map(|last| Box::new(Expr::new(ExprKind::Const(path.clone()), start.to(last.span))));
        let (args, block) = self.paren_args()?;
        let kind = ExprKind::Call { recv, name, args, block: block.map(Box::new), safe: false, parens: true };
        Ok(Expr::new(kind, start.to(self.prev_span())))
    }

    pub(crate) fn hash(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let entries = self.with_do(true, |p| {
            let mut entries = Vec::new();
            loop {
                p.skip_newlines();
                if p.at(&T::RBrace) {
                    break;
                }
                if let T::Label(name) = p.kind().clone() {
                    let key = Expr::new(ExprKind::Symbol(name.clone()), p.bump().span);
                    if p.at(&T::Comma) || p.at(&T::RBrace) {
                        // `{query:}`: the variable of the same name (same span as its key)
                        entries.push((key.clone(), Expr::new(ExprKind::Var(name), key.span)));
                    } else {
                        p.skip_newlines();
                        entries.push((key, p.expr()?));
                    }
                } else {
                    let key = p.expr()?;
                    p.expect(T::FatArrow, "`=>`")?;
                    p.skip_newlines();
                    entries.push((key, p.expr()?));
                }
                p.skip_newlines();
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.skip_newlines();
            p.expect(T::RBrace, "`}`")?;
            Ok(entries)
        })?;
        Ok(Expr::new(ExprKind::Hash(entries), start.to(self.prev_span())))
    }

    pub(crate) fn jump(&mut self) -> PResult<Expr> {
        let tok = self.bump();
        let has_value = !matches!(
            self.kind(),
            T::Newline
                | T::Eof
                | T::RBrace
                | T::RParen
                | T::Kw(
                    K::If
                        | K::Unless
                        | K::While
                        | K::Until
                        | K::Rescue
                        | K::End
                        | K::Else
                        | K::Elsif
                        | K::In
                        | K::When
                        | K::Then
                        | K::Do
                        | K::Ensure
                )
        );
        let value = if has_value { Some(Box::new(self.expr()?)) } else { None };
        let span = tok.span.to(self.prev_span());
        let kind = match tok.kind {
            T::Kw(K::Return) => ExprKind::Return(value),
            T::Kw(K::Break) => ExprKind::Break(value),
            _ => ExprKind::Next(value),
        };
        Ok(Expr::new(kind, span))
    }
}
