//! Indexing and slicing, typed: `xs[i]` is an item, `h[k]` a value, and a
//! slice — `s[start, length]`, `s[a..b]`, `xs[start, length]`, `xs[a..b]`
//! — is `String?` or `Array(T)?`, `nil` when it starts past the end (as in
//! Ruby; an item read stays `T`, as `xs[i]` always was). A secret is never
//! indexed: it serves whole. Slices are read only: `xs[i] = v` and
//! `h[k] = v` assign, `xs[0, 2] = v`, `xs[1..] = v` and `s[0] = "z"` do
//! not (a string is never changed in place).

use grenat_ast::Span;

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// `target[index…] = value`: an array's item or a hash's key, never a
    /// slice, a string or a secret.
    pub(crate) fn index_assign(&mut self, target: &V, index: &[V], span: Span) {
        let receiver = target.ty.base();
        if secrets::is_secret(receiver) {
            self.slice(target, index, span);
            return;
        }
        let slice = index.len() == 2 || matches!(index, [k] if matches!(k.ty.base(), Ty::Range));
        match receiver {
            Ty::Str => self.report(
                Diagnostic::new(span, "a string cannot be changed in place")
                    .with_code(E_TYPE)
                    .with_help("build a new one: `s = \"#{s[0]}z\"`"),
            ),
            Ty::Array(_) if slice => self.report(
                Diagnostic::new(span, "a slice cannot be assigned")
                    .with_code(E_TYPE)
                    .with_help("build a new array: `xs = [9] + xs[2..]`"),
            ),
            _ => {}
        }
    }

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
