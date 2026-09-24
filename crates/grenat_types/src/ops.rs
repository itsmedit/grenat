//! Binary operators.

use grenat_ast::{BinOp, Diagnostic, Expr, Span};

use crate::ty::{Ty, V, join};
use crate::*;

pub(crate) fn op_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Rem => "%",
        Pow => "**",
        Eq => "==",
        NotEq => "!=",
        Lt => "<",
        Le => "<=",
        Gt => ">",
        Ge => ">=",
        Cmp => "<=>",
        Match => "=~",
        And => "&&",
        Or => "||",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
    }
}

impl<'p> Checker<'p> {
    pub(crate) fn binary(&mut self, cx: &mut Ctx<'p>, op: BinOp, l: V, r: V, span: Span, lhs: &'p Expr) -> V {
        use Ty::*;
        let taint = l.taint.or(r.taint);
        let (lt, rt) = (l.ty.base().clone(), r.ty.base().clone());
        let ty = match op {
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Match => Bool,
            BinOp::Cmp => Int,
            BinOp::And | BinOp::Or => join(&l.ty, &r.ty),
            BinOp::Shl if matches!(lt, Array(_)) => {
                self.taint_container(cx, lhs, r.taint);
                l.ty.clone()
            }
            _ if lt.is_unknown() || rt.is_unknown() => Unknown,
            BinOp::Add if lt == Str && rt == Str => Str,
            BinOp::Add if lt == Str => {
                self.report(
                    Diagnostic::new(span, format!("cannot add `{rt}` to a string"))
                        .with_code(E_TYPE)
                        .with_help("use interpolation: \"…#{value}\""),
                );
                Str
            }
            BinOp::Mul if lt == Str && rt == Int => Str,
            BinOp::Add | BinOp::Sub if matches!(lt, Array(_)) && matches!(rt, Array(_)) => join(&lt, &rt),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::Pow if lt == Int && rt == Int => {
                Int
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::Pow
                if lt.is_numeric() && rt.is_numeric() =>
            {
                Float
            }
            BinOp::Add | BinOp::Sub if lt == Money && rt == Money => Money,
            BinOp::Add if lt == Duration && rt == Duration => Duration,
            BinOp::Mul if lt == Duration && rt == Int => Duration,
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr if lt == Int && rt == Int => Int,
            _ => {
                self.error(E_TYPE, span, format!("operator `{}` is not defined between `{lt}` and `{rt}`", op_str(op)));
                Unknown
            }
        };
        V { ty, taint }
    }
}
