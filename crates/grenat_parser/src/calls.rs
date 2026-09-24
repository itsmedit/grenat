//! Calls: with or without parentheses, named arguments, `&.` short blocks.

use grenat_ast::*;
use grenat_lexer::{Keyword as K, TokenKind as T};

use crate::*;

pub(crate) enum ArgOrBlock {
    Arg(Arg),
    Block(Block),
}

pub(crate) fn takes_block(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Var(_) | ExprKind::Call { block: None, .. })
}

pub(crate) fn attach_block(e: Expr, block: Block) -> Expr {
    let span = e.span.to(block.span);
    let block = Some(Box::new(block));
    let kind = match e.kind {
        ExprKind::Var(name) => ExprKind::Call {
            recv: None,
            name: Ident { name, span: e.span },
            args: Vec::new(),
            block,
            safe: false,
            parens: false,
        },
        ExprKind::Call { recv, name, args, safe, parens, .. } => {
            ExprKind::Call { recv, name, args, block, safe, parens }
        }
        _ => unreachable!("attach_block on an expression that takes no block"),
    };
    Expr::new(kind, span)
}

impl<'d> Parser<'d> {
    pub(crate) fn method_call(&mut self, recv: Expr, safe: bool) -> PResult<Expr> {
        let name = self.method_name()?;
        let (args, block, parens) = if self.at_tight(&T::LParen) {
            let (args, block) = self.paren_args()?;
            (args, block, true)
        } else if self.can_start_command_arg() {
            let (args, block) = self.command_args()?;
            (args, block, false)
        } else {
            (Vec::new(), None, false)
        };
        let span = recv.span.to(self.prev_span());
        let kind = ExprKind::Call { recv: Some(Box::new(recv)), name, args, block: block.map(Box::new), safe, parens };
        Ok(Expr::new(kind, span))
    }

    /// An identifier followed, after whitespace, by one of these tokens is a parenthesis-free call.
    pub(crate) fn can_start_command_arg(&self) -> bool {
        let tok = self.peek();
        tok.space_before
            && matches!(
                tok.kind,
                T::Int(_)
                    | T::Float(_)
                    | T::Str(_)
                    | T::Symbol(_)
                    | T::Label(_)
                    | T::Ident(_)
                    | T::Const(_)
                    | T::IVar(_)
                    | T::LBracket
                    | T::LParen
                    | T::Kw(K::True | K::False | K::Nil | K::SelfKw)
            )
    }

    /// Parenthesis-free arguments: `do` is not consumed inside them and goes to this call.
    pub(crate) fn command_args(&mut self) -> PResult<(Vec<Arg>, Option<Block>)> {
        let (args, mut block) = self.with_do(false, |p| p.arg_list())?;
        if block.is_none() {
            if self.at(&T::LBrace) {
                block = Some(self.brace_block()?);
            } else if !self.no_do && self.at_kw(K::Do) {
                block = Some(self.do_block()?);
            }
        }
        Ok((args, block))
    }

    pub(crate) fn arg_list(&mut self) -> PResult<(Vec<Arg>, Option<Block>)> {
        let mut args = Vec::new();
        let mut block = None;
        loop {
            match self.arg()? {
                ArgOrBlock::Arg(arg) => args.push(arg),
                ArgOrBlock::Block(b) => block = Some(b),
            }
            if !self.eat(&T::Comma) {
                break;
            }
            self.skip_newlines();
        }
        Ok((args, block))
    }

    pub(crate) fn paren_args(&mut self) -> PResult<(Vec<Arg>, Option<Block>)> {
        self.expect(T::LParen, "`(`")?;
        self.with_do(true, |p| {
            let mut args = Vec::new();
            let mut block = None;
            loop {
                p.skip_newlines();
                if p.at(&T::RParen) {
                    break;
                }
                match p.arg()? {
                    ArgOrBlock::Arg(arg) => args.push(arg),
                    ArgOrBlock::Block(b) => block = Some(b),
                }
                p.skip_newlines();
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.skip_newlines();
            p.expect(T::RParen, "`)`")?;
            Ok((args, block))
        })
    }

    pub(crate) fn arg(&mut self) -> PResult<ArgOrBlock> {
        match self.kind().clone() {
            T::Label(name) => {
                let name = Ident { name, span: self.bump().span };
                let punned =
                    matches!(self.kind(), T::Comma | T::RParen | T::RBracket | T::RBrace | T::Newline | T::Eof)
                        || self.at_kw(K::Do);
                let value = if punned { None } else { Some(self.expr()?) };
                Ok(ArgOrBlock::Arg(Arg::Named { name, value }))
            }
            T::Amp => {
                self.bump();
                Ok(ArgOrBlock::Arg(Arg::BlockPass(self.unary()?)))
            }
            // `&.sent?`: short block over the implicit parameter `it`
            T::SafeDot => {
                let start = self.bump().span;
                let call = self.method_call(Expr::new(ExprKind::It, start), false)?;
                let body = self.postfix_from(call)?;
                let span = start.to(body.span);
                let it = Param { name: Ident { name: "it".into(), span: start }, ty: None, default: None, span: start };
                Ok(ArgOrBlock::Block(Block { params: vec![it], body: Body::new(vec![body], span), span }))
            }
            _ => Ok(ArgOrBlock::Arg(Arg::Pos(self.expr()?))),
        }
    }

    pub(crate) fn expr_list(&mut self, close: T, expected: &str) -> PResult<Vec<Expr>> {
        self.with_do(true, |p| {
            let mut items = Vec::new();
            loop {
                p.skip_newlines();
                if p.at(&close) {
                    break;
                }
                items.push(p.expr()?);
                p.skip_newlines();
                if !p.eat(&T::Comma) {
                    break;
                }
            }
            p.skip_newlines();
            p.expect(close, expected)?;
            Ok(items)
        })
    }
}
