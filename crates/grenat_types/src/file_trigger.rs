//! Files in, checked: `on_file "inbox/*.pdf", every:, settle:, existing: do
//! |path| … end` reads a folder — an `fs.read` effect on the directory a
//! literal pattern writes out (`inbox`), any `fs.read` grant for a computed
//! one, whose directory the runtime checks — and hands its block the path
//! of a file dropped there: a `String`, untrusted (its name is the
//! dropper's). A literal pattern is parsed here as the runtime parses it,
//! and the options refused as it refuses them; `deliver_file(path)` exists
//! inside a `test` block only.

use grenat_ast::{Block, Span};
use grenat_watch::Glob;

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// `on_file "inbox/*.pdf", settle: 2.s do |path| … end`.
    pub(crate) fn on_file_call(&mut self, cx: &mut Ctx<'p>, span: Span, argv: &[ArgV], block: Option<&'p Block>) -> V {
        const USAGE: &str = "`on_file \"inbox/*.pdf\" do |path| … end`";
        let positional: Vec<&ArgV> = argv.iter().filter(|a| a.name.is_none()).collect();
        match positional.as_slice() {
            [pattern] => {
                if secrets::is_secret(&pattern.v.ty) || !self.compat(&pattern.v.ty, &Ty::Str) {
                    let message = format!("`on_file` expects a pattern (a `String`), got `{}`", pattern.v.ty);
                    self.error(E_TYPE, pattern.span, message);
                }
                if let Some(origin) = pattern.v.taint {
                    self.taint_violation(pattern.span, origin, "on_file", "fs.read");
                }
                let glob = pattern.lit.as_deref().map(Glob::parse);
                if let Some(Err(why)) = &glob {
                    self.error(E_TYPE, pattern.span, why.clone());
                }
                // reading the folder: `fs.read` on the directory of a literal pattern
                let arg = match glob {
                    Some(Ok(glob)) => Some(glob.base().to_string_lossy().into_owned()),
                    _ => None,
                };
                // declaring reads nothing: a workflow needs no `step` for it
                cx.require_effect(Eff { path: "fs.read".into(), arg, origin: span });
            }
            _ => self.error(E_TYPE, span, format!("`on_file` expects a pattern: {USAGE}")),
        }
        for arg in argv.iter().filter(|a| a.name.is_some()) {
            self.on_file_option(arg);
        }
        if block.is_none() {
            self.error(E_TYPE, span, format!("`on_file` expects a block: {USAGE}"));
        }
        self.walk_block(cx, block, &[V { ty: Ty::Str, taint: Some(span) }]);
        V::new(Ty::Nil)
    }

    /// `every:` a positive duration, `settle:` one not negative, `existing:` a boolean.
    fn on_file_option(&mut self, arg: &ArgV) {
        let name = arg.name.as_deref().unwrap_or_default();
        let (fits, what) = match name {
            "every" | "settle" => {
                let duration = matches!(arg.v.ty, Ty::Int | Ty::Float) || self.compat(&arg.v.ty, &Ty::Duration);
                let in_range = arg.number.is_none_or(|s| s.is_finite() && (s > 0.0 || (name == "settle" && s == 0.0)));
                let what = if name == "every" {
                    "a positive duration (`every: 30.s`)"
                } else {
                    "a duration, zero or more (`settle: 2.s`)"
                };
                (duration && in_range, what)
            }
            "existing" => (self.compat(&arg.v.ty, &Ty::Bool), "true or false"),
            _ => {
                let message = format!("`on_file` has no option `{name}:` (it takes `every:`, `settle:`, `existing:`)");
                self.error(E_TYPE, arg.span, message);
                return;
            }
        };
        if !fits {
            self.error(E_TYPE, arg.span, format!("`on_file` expects `{name}:` {what}, got `{}`", arg.v.ty));
        }
    }

    /// `deliver_file("inbox/a.pdf")`: what became of the file.
    pub(crate) fn deliver_file_call(&mut self, cx: &Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        self.only_in_tests(cx, span, "deliver_file", "it hands an `on_file` handler a path, as a dropped file would");
        match argv {
            [path] if path.name.is_none() => {
                if secrets::is_secret(&path.v.ty) {
                    self.secret_as_string(path.span, "deliver_file", "path");
                } else if !self.compat(&path.v.ty, &Ty::Str) {
                    let message = format!("`deliver_file` expects a path (a `String`), got `{}`", path.v.ty);
                    self.error(E_TYPE, path.span, message);
                }
            }
            _ => self.error(E_TYPE, span, "`deliver_file` expects a path: `deliver_file(\"inbox/a.pdf\")`"),
        }
        V::new(Ty::Hash(Box::new(Ty::Str), Box::new(Ty::opt(Ty::Str))))
    }
}
