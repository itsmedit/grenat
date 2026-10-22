//! Arrays as sets: `a & b`, `a | b`, `a - b` and their methods
//! (`intersection`, `union`, `difference`, `intersect?`). Both sides are
//! arrays of the same items: `Array(T) op Array(T)` is an `Array(T)`.

use grenat_ast::Span;

use crate::context::ArgV;
use crate::ty::{Ty, V, join};
use crate::*;

/// The methods of an array taken as a set.
pub(crate) fn is_set_method(name: &str) -> bool {
    matches!(name, "intersection" | "union" | "difference" | "intersect?")
}

/// The array that `lt op rt` makes, when both are arrays of the same
/// items (`Int` and `Float` mix, as `1 == 1.0`); `None` otherwise.
pub(crate) fn combined(lt: &Ty, rt: &Ty) -> Option<Ty> {
    let (Ty::Array(x), Ty::Array(y)) = (lt.base(), rt.base()) else { return None };
    let items = join(x, y);
    let unrelated = items.is_unknown() && !x.is_unknown() && !y.is_unknown();
    (!unrelated).then(|| Ty::array(items))
}

impl<'p> Checker<'p> {
    /// `xs.union(ys, …)` and the others: each argument an array of the
    /// same items; an untrusted one makes the result untrusted.
    pub(crate) fn set_method(&mut self, recv: &V, name: &str, span: Span, argv: &[ArgV]) -> V {
        let mut ty = recv.ty.base().clone();
        let mut taint = recv.taint;
        if name == "intersect?" && argv.len() != 1 {
            self.error(E_TYPE, span, "`intersect?` expects one array: `xs.intersect?(ys)`");
        }
        for arg in argv {
            taint = taint.or(arg.v.taint);
            if arg.v.ty.is_unknown() {
                ty = Ty::Unknown;
                continue;
            }
            match combined(&ty, &arg.v.ty) {
                Some(next) => ty = next,
                None if ty.is_unknown() => {}
                None => {
                    self.error(E_TYPE, arg.span, format!("`{name}` expects an array like `{ty}`, not `{}`", arg.v.ty));
                    ty = Ty::Unknown;
                }
            }
        }
        let ty = if name == "intersect?" { Ty::Bool } else { ty };
        V { ty, taint }
    }
}
