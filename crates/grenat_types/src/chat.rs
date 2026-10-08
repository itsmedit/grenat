//! Chat, checked: webhooks signed as GitHub, Slack or Microsoft Teams sign
//! (`on_webhook path, signature: :slack, secret: … do |req| … end`), the
//! approvals decided in Slack (`approvals_in_slack channel:, token:,
//! signing_secret:, allowed:, path:` — `net("slack.com")`), `Slack.post`'s
//! arguments (`net("slack.com")` and nothing untrusted, see
//! `methods::static_call`), and `deliver_slack_click`, inside a `test`
//! block only. Options are refused here as the runtime refuses them; a
//! token, a secret or a signing secret is a trusted `String` or `Secret`.

use grenat_ast::{Block, Span};

use crate::ty::{Ty, V};
use crate::*;

/// The host of Slack's Web API.
pub(crate) const SLACK_HOST: &str = "slack.com";

/// The signatures `on_webhook` checks.
const SIGNATURES: [&str; 3] = ["github", "slack", "teams"];

impl<'p> Checker<'p> {
    /// `on_webhook "/github", secret: …, signature: :github do |req| … end`
    /// (or `token:`, or nothing).
    pub(crate) fn on_webhook_call(
        &mut self,
        cx: &mut Ctx<'p>,
        span: Span,
        argv: &[ArgV],
        block: Option<&'p Block>,
    ) -> V {
        const USAGE: &str = "`on_webhook \"/github\", signature: :github, secret: … do |req| … end`";
        let named = |n: &str| argv.iter().find(|a| a.name.as_deref() == Some(n));
        match argv.iter().filter(|a| a.name.is_none()).collect::<Vec<_>>().as_slice() {
            [path] => {
                if secrets::is_secret(&path.v.ty) || !self.compat(&path.v.ty, &Ty::Str) {
                    self.error(
                        E_TYPE,
                        path.span,
                        format!("`on_webhook` expects a path (a `String`), got `{}`", path.v.ty),
                    );
                } else if path.lit.as_deref().is_some_and(|p| !p.starts_with('/')) {
                    self.error(E_TYPE, path.span, "a webhook path starts with `/`");
                }
            }
            _ => self.error(E_TYPE, span, format!("`on_webhook` expects a path: {USAGE}")),
        }
        for arg in argv.iter().filter(|a| a.name.is_some()) {
            let name = arg.name.as_deref().unwrap_or_default();
            match name {
                "signature" => match arg.symbol.as_deref() {
                    Some(kind) if SIGNATURES.contains(&kind) => {}
                    Some(kind) => {
                        let message =
                            format!("`on_webhook` knows no `signature: :{kind}` (`:github`, `:slack`, `:teams`)");
                        self.error(E_TYPE, arg.span, message);
                    }
                    None if !self.compat(&arg.v.ty, &Ty::Sym) => {
                        let message =
                            format!("`on_webhook` expects `signature:` a symbol (`:slack`), got `{}`", arg.v.ty);
                        self.error(E_TYPE, arg.span, message);
                    }
                    None => {}
                },
                "secret" | "token" => self.credential_option("on_webhook", arg),
                _ => {
                    let message = format!(
                        "`on_webhook` has no option `{name}:` (it takes `signature:` with `secret:`, or `token:`)"
                    );
                    self.error(E_TYPE, arg.span, message);
                }
            }
        }
        let (signature, secret, token) = (named("signature"), named("secret"), named("token"));
        if signature.is_some() != secret.is_some() || (token.is_some() && secret.is_some()) {
            let message = "`on_webhook` takes `secret:` with `signature: :github`, `:slack` or `:teams`, or `token:`";
            self.error(E_TYPE, span, message);
        }
        if block.is_none() {
            self.error(E_TYPE, span, format!("`on_webhook` expects a block: {USAGE}"));
        }
        self.walk_block(cx, block, &[V::new(Ty::User(builtins::REQUEST.into()))]);
        V::new(Ty::Nil)
    }

    /// A token or a secret: a trusted `String` or `Secret`.
    fn credential_option(&mut self, owner: &str, arg: &ArgV) {
        let name = arg.name.as_deref().unwrap_or_default();
        if !secrets::is_secret(&arg.v.ty) && !self.compat(&arg.v.ty, &Ty::Str) {
            let message = format!("`{owner}` expects `{name}:` a `String` or a `Secret`, got `{}`", arg.v.ty);
            self.error(E_TYPE, arg.span, message);
        }
        if let Some(origin) = arg.v.taint {
            self.taint_violation(arg.span, origin, owner, "net");
        }
    }

    /// `approvals_in_slack channel: "#approvals", token: …, signing_secret: …`.
    pub(crate) fn approvals_in_slack_call(&mut self, cx: &mut Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        const OWNER: &str = "approvals_in_slack";
        for arg in argv {
            let Some(name) = arg.name.as_deref() else {
                self.error(
                    E_TYPE,
                    arg.span,
                    "`approvals_in_slack` takes options: `channel:`, `token:`, `signing_secret:`",
                );
                continue;
            };
            match name {
                "token" | "signing_secret" => self.credential_option(OWNER, arg),
                "channel" | "path" | "allowed" => {
                    let expected = if name == "allowed" { Ty::array(Ty::Str) } else { Ty::Str };
                    if secrets::is_secret(&arg.v.ty) {
                        self.secret_as_string(arg.span, OWNER, &format!("{name}:"));
                    } else if !self.compat(&arg.v.ty, &expected) {
                        let message = format!("`{OWNER}` expects `{name}:` a `{expected}`, got `{}`", arg.v.ty);
                        self.error(E_TYPE, arg.span, message);
                    } else if let Some(origin) = arg.v.taint {
                        self.taint_violation(arg.span, origin, OWNER, "net");
                    } else if name == "path" && arg.lit.as_deref().is_some_and(|p| !p.starts_with('/')) {
                        self.error(E_TYPE, arg.span, "`approvals_in_slack` expects `path:` a route, starting with `/`");
                    } else if name == "channel" && arg.lit.as_deref().is_some_and(|c| c.trim().is_empty()) {
                        self.error(
                            E_TYPE,
                            arg.span,
                            "`approvals_in_slack` expects `channel:` a channel, not a blank one",
                        );
                    }
                }
                _ => {
                    let message = format!(
                        "`{OWNER}` has no option `{name}:` (it takes `channel:`, `token:`, `signing_secret:`, `allowed:`, `path:`)"
                    );
                    self.error(E_TYPE, arg.span, message);
                }
            }
        }
        for required in ["channel", "token", "signing_secret"] {
            if !argv.iter().any(|a| a.name.as_deref() == Some(required)) {
                self.error(E_TYPE, span, format!("`{OWNER}` expects `{required}:`"));
            }
        }
        // posting and updating messages: declaring opens nothing
        cx.require_effect(Eff { path: "net".into(), arg: Some(SLACK_HOST.into()), origin: span });
        V::new(Ty::Nil)
    }

    /// `Slack.post(channel, text, token:)`: a channel and a text (never a
    /// secret), and a token.
    pub(crate) fn slack_post_args(&mut self, span: Span, argv: &[ArgV]) {
        let positional: Vec<&ArgV> = argv.iter().filter(|a| a.name.is_none()).collect();
        if positional.len() != 2 {
            self.error(
                E_TYPE,
                span,
                "`Slack.post` expects a channel and a text: `Slack.post(\"#ops\", \"Deploy done\")`",
            );
        }
        for (arg, slot) in positional.iter().zip(["channel", "text"]) {
            if secrets::is_secret(&arg.v.ty) {
                self.secret_as_string(arg.span, "Slack.post", slot);
            } else if !self.compat(&arg.v.ty, &Ty::Str) {
                let message = format!("`Slack.post` expects a `String` for `{slot}`, got `{}`", arg.v.ty);
                self.error(E_TYPE, arg.span, message);
            }
        }
        for arg in argv.iter().filter(|a| a.name.is_some()) {
            match arg.name.as_deref() {
                Some("token") if secrets::is_secret(&arg.v.ty) || self.compat(&arg.v.ty, &Ty::Str) => {}
                Some("token") => {
                    let message = format!("`Slack.post` expects `token:` a `String` or a `Secret`, got `{}`", arg.v.ty);
                    self.error(E_TYPE, arg.span, message);
                }
                Some(name) => {
                    self.error(E_TYPE, arg.span, format!("`Slack.post` has no option `{name}:` (it takes `token:`)"));
                }
                None => {}
            }
        }
    }

    /// `deliver_slack_click(id, action: :approve, user: "U0123ABC")`: what
    /// the click did.
    pub(crate) fn deliver_slack_click_call(&mut self, cx: &Ctx<'p>, span: Span, argv: &[ArgV]) -> V {
        self.only_in_tests(cx, span, "deliver_slack_click", "it clicks an approval's button, as a user in Slack would");
        match argv.iter().filter(|a| a.name.is_none()).collect::<Vec<_>>().as_slice() {
            [id] if self.compat(&id.v.ty, &Ty::Int) => {}
            [id] => self.error(
                E_TYPE,
                id.span,
                format!("`deliver_slack_click` expects an approval's id, got `{}`", id.v.ty),
            ),
            _ => self.error(E_TYPE, span, "`deliver_slack_click` expects an approval's id"),
        }
        for arg in argv.iter().filter(|a| a.name.is_some()) {
            match arg.name.as_deref() {
                Some("action") => match arg.symbol.as_deref() {
                    Some("approve" | "deny") => {}
                    _ if arg.symbol.is_none() && self.compat(&arg.v.ty, &Ty::Sym) => {}
                    _ => self.error(E_TYPE, arg.span, "`deliver_slack_click` expects `action: :approve` or `:deny`"),
                },
                Some("user") if secrets::is_secret(&arg.v.ty) => {
                    self.secret_as_string(arg.span, "deliver_slack_click", "user:")
                }
                Some("user") if self.compat(&arg.v.ty, &Ty::Str) => {}
                Some("user") => {
                    let message = format!("`deliver_slack_click` expects `user:` a Slack user ID, got `{}`", arg.v.ty);
                    self.error(E_TYPE, arg.span, message);
                }
                Some(name) => {
                    let message =
                        format!("`deliver_slack_click` has no option `{name}:` (it takes `action:` and `user:`)");
                    self.error(E_TYPE, arg.span, message);
                }
                None => {}
            }
        }
        V::new(Ty::Hash(Box::new(Ty::Str), Box::new(Ty::Unknown)))
    }
}
