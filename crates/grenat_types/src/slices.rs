//! Indexing and slicing, typed: `xs[i]` is an item, `h[k]` a value, and a
//! slice — `s[start, length]`, `s[a..b]`, `xs[start, length]`, `xs[a..b]`
//! — is `String?` or `Array(T)?`, `nil` when it starts past the end (as in
//! Ruby; an item read stays `T`, as `xs[i]` always was). A secret is never
//! indexed: it serves whole.

use grenat_ast::Span;

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// The type of `target[index…]` for a string, an array or a hash; `None`
    /// for the other receivers.
    pub(crate) fn slice(&mut self, target: &V, index: &[V], span: Span) -> Option<Ty> {
        let receiver = target.ty.base();
        if secrets::is_secret(receiver) {
            self.report(
                Diagnostic::new(span, "a secret cannot be indexed nor sliced")
                    .with_code(E_SECRET)
                    .with_help("a secret serves whole: in an HTTP header or URL, or a connection"),
            );
            return Some(Ty::Unknown);
        }
        let (item, slice) = match receiver {
            Ty::Str => (Ty::Str, Ty::Str),
            Ty::Array(t) => ((**t).clone(), receiver.clone()),
            Ty::Hash(..) => {
                if index.len() != 1 {
                    self.error(E_TYPE, span, format!("a hash is indexed by one key, got {}", index.len()));
                }
                return None;
            }
            _ => return None,
        };
        Some(match index {
            [start, length] => {
                for v in [start, length] {
                    if !self.compat(&v.ty, &Ty::Int) {
                        self.error(E_TYPE, span, format!("a slice `[start, length]` takes two `Int`s, got `{}`", v.ty));
                    }
                }
                Ty::opt(slice)
            }
            [key] => match key.ty.base() {
                Ty::Range => Ty::opt(slice),
                Ty::Int | Ty::Unknown => item,
                other => {
                    self.error(
                        E_TYPE,
                        span,
                        format!("`{receiver}` is indexed by an `Int` or a `Range`, got `{other}`"),
                    );
                    Ty::Unknown
                }
            },
            _ => {
                self.error(E_TYPE, span, format!("an index takes one or two values, got {}", index.len()));
                Ty::Unknown
            }
        })
    }
}
