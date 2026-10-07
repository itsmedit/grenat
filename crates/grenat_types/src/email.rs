//! Email in, checked: `on_email url, every:, move_to:, token: do |email| … end`
//! reaches an IMAP server — a `net` effect on the host of a literal URL
//! (any `net` grant for a configured one, whose host the runtime checks) —
//! and hands its block an `IncomingEmail`, every field untrusted;
//! `deliver_email(from:, subject:, …)` exists inside a `test` block only.

use grenat_ast::{Block, Span};

use crate::ty::{Ty, V};
use crate::*;

impl<'p> Checker<'p> {
    /// `on_email url, every: 1.minute, move_to: "Done" do |email| … end`.
    pub(crate) fn on_email_call(&mut self, cx: &mut Ctx<'p>, span: Span, argv: &[ArgV], block: Option<&'p Block>) -> V {
        const USAGE: &str = "`on_email url, every: 1.minute do |email| … end`";
        let positional: Vec<&ArgV> = argv.iter().filter(|a| a.name.is_none()).collect();
        match positional.as_slice() {
            [url] => {
                if !secrets::is_secret(&url.v.ty) && !self.compat(&url.v.ty, &Ty::Str) {
                    let message =
                        format!("`on_email` expects a mailbox URL (a `String` or a `Secret`), got `{}`", url.v.ty);
                    self.error(E_TYPE, url.span, message);
                }
                if let Some(origin) = url.v.taint {
                    self.taint_violation(url.span, origin, "on_email", "net");
                }
                // reading the mailbox: `net` on the host of a literal `imaps://` URL
                let arg = url.lit.as_deref().and_then(crate::effects::server_host).map(str::to_string);
                // declaring opens nothing: a workflow needs no `step` for it
                cx.require_effect(Eff { path: "net".into(), arg, origin: span });
            }
            _ => self.error(E_TYPE, span, format!("`on_email` expects a mailbox URL: {USAGE}")),
        }
        for arg in argv.iter().filter(|a| a.name.is_some()) {
            let name = arg.name.as_deref().unwrap_or_default();
            let expected = match name {
                "every" => Ty::Duration,
                "move_to" => Ty::Str,
                "token" => Ty::Sym,
                _ => {
                    let message =
                        format!("`on_email` has no option `{name}:` (it takes `every:`, `move_to:`, `token:`)");
                    self.error(E_TYPE, arg.span, message);
                    continue;
                }
            };
            let fits = match name {
                "every" => matches!(arg.v.ty, Ty::Int | Ty::Float) || self.compat(&arg.v.ty, &expected),
                _ => !secrets::is_secret(&arg.v.ty) && self.compat(&arg.v.ty, &expected),
            };
            if !fits {
                let what = match name {
                    "every" => "a duration (`every: 1.minute`)",
                    "move_to" => "a folder (`move_to: \"Done\"`)",
                    _ => "a function (`token: :outlook_token`)",
                };
                self.error(E_TYPE, arg.span, format!("`on_email` expects `{name}:` {what}, got `{}`", arg.v.ty));
            }
        }
        if block.is_none() {
            self.error(E_TYPE, span, format!("`on_email` expects a block: {USAGE}"));
        }
        self.walk_block(cx, block, &[V::new(Ty::User(builtins::INCOMING_EMAIL.into()))]);
        V::new(Ty::Nil)
    }

    /// `deliver_email(from: "ada@acme.com", subject: "…", text: "…",
    /// attachments: [Pdf.read("a.pdf")])`: what became of the message.
    pub(crate) fn deliver_email_call(&mut self, cx: &Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        self.only_in_tests(cx, span, "deliver_email", "it hands an `on_email` handler a message, as a mailbox would");
        let addresses = Ty::array(Ty::Str);
        let attachments = Ty::array(Ty::User(builtins::ATTACHMENT.into()));
        for arg in argv {
            let Some(name) = arg.name.as_deref() else {
                let message =
                    "`deliver_email` takes named fields: `deliver_email(from: \"ada@acme.com\", text: \"…\")`";
                self.error(E_TYPE, arg.span, message);
                continue;
            };
            let fits = match name {
                "from" | "from_name" | "reply_to" | "subject" | "message_id" | "text" | "html" | "folder" => {
                    self.compat(&arg.v.ty, &Ty::Str)
                }
                "to" | "cc" => self.compat(&arg.v.ty, &Ty::Str) || self.compat(&arg.v.ty, &addresses),
                "date" => matches!(arg.v.ty, Ty::Int | Ty::Float | Ty::Str | Ty::Unknown),
                "attachments" => self.compat(&arg.v.ty, &attachments),
                _ => {
                    let message = format!(
                        "`deliver_email` has no field `{name}:` (from, from_name, to, cc, reply_to, subject, date, message_id, text, html, attachments, folder)"
                    );
                    self.error(E_TYPE, arg.span, message);
                    continue;
                }
            };
            if secrets::is_secret(&arg.v.ty) {
                self.secret_as_string(arg.span, "deliver_email", name);
            } else if !fits {
                let message = format!("`deliver_email` expects another type for `{name}:`, got `{}`", arg.v.ty);
                self.error(E_TYPE, arg.span, message);
            }
        }
        if argv.iter().all(|a| a.name.is_some()) && !argv.iter().any(|a| a.name.as_deref() == Some("from")) {
            self.error(E_TYPE, span, "`deliver_email` expects `from:`: `deliver_email(from: \"ada@acme.com\", …)`");
        }
        V::new(Ty::Hash(Box::new(Ty::Str), Box::new(Ty::opt(Ty::Str))))
    }
}
