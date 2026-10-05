//! Time, checked: the arguments of `Time.parse(text)`, `Time.iso(t)`,
//! `Time.date(t)`, `Time.weekday(t)` and `Time.at(year, month, day, …)` (an
//! instant is a `Float` of epoch seconds), and `freeze_time(instant) do … end`,
//! which exists inside a `test` block only.

use grenat_ast::{Block, Diagnostic, Span};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// `Time.name(…)`: how many arguments, of which types.
    pub(crate) fn time_args(&mut self, span: Span, name: &str, argv: &[ArgV]) {
        let (expected, usage): (&[Ty], &str) = match name {
            "now" | "today" => (&[], "no argument"),
            "parse" => (&[Ty::Str], "a text: `Time.parse(\"2026-09-28T08:00:00Z\")`"),
            "iso" | "date" | "weekday" => (&[Ty::Float], "an instant (epoch seconds): `Time.iso(Time.now)`"),
            "at" => (
                &[Ty::Int, Ty::Int, Ty::Int, Ty::Int, Ty::Int, Ty::Float],
                "`Time.at(year, month, day, hour = 0, min = 0, sec = 0)`",
            ),
            _ => return,
        };
        let required = if name == "at" { 3 } else { expected.len() };
        if let Some(arg) = argv.iter().find(|a| a.name.is_some()) {
            let message = format!("`Time.{name}` takes no named argument: it expects {usage}");
            self.error(E_TYPE, arg.span, message);
            return;
        }
        if !(required..=expected.len()).contains(&argv.len()) {
            self.error(E_TYPE, span, format!("`Time.{name}` expects {usage}"));
            return;
        }
        for (arg, ty) in argv.iter().zip(expected) {
            if !self.compat(&arg.v.ty, ty) {
                self.error(E_TYPE, arg.span, format!("`Time.{name}` expects `{ty}`, got `{}`: {usage}", arg.v.ty));
            }
        }
    }

    /// `freeze_time("2026-09-28T08:00:00Z") do … end`: the block's value.
    pub(crate) fn freeze_time_call(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        argv: &[ArgV],
        block: Option<&'p Block>,
    ) -> V {
        if cx.tests == 0 {
            self.report(
                Diagnostic::new(span, "`freeze_time` can only be used inside a test (`test \"…\" do … end`)")
                    .with_code(E_DECL)
                    .with_help("outside tests, pass the instant as a parameter"),
            );
        }
        let instant = match argv {
            [arg] if arg.name.is_none() => Some(arg),
            _ => None,
        };
        match instant {
            Some(arg) if !self.compat(&arg.v.ty, &Ty::Str) && !self.compat(&arg.v.ty, &Ty::Float) => {
                let message =
                    format!("`freeze_time` expects an instant (a `String` or epoch seconds), got `{}`", arg.v.ty);
                self.error(E_TYPE, arg.span, message);
            }
            Some(_) if block.is_some() => {}
            _ => self.error(
                E_TYPE,
                span,
                "`freeze_time` expects an instant and a block: `freeze_time(\"2026-09-28T08:00:00Z\") do … end`",
            ),
        }
        self.walk_block(cx, block, &[]).unwrap_or_else(V::unknown)
    }
}
