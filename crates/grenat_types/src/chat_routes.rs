//! The routes a program claims for chat, checked across the program as the
//! runtime checks them when they are declared, in either order:
//! `approvals_in_slack` is declared once; its interactivity route
//! (`path:`, `/slack/interactivity` by default) is no `on_webhook`'s path,
//! and no web route matches it (`post "/slack/interactivity" do … end`
//! would never be reached: the clicks are answered first). Literal paths
//! only; computed ones are the runtime's to refuse.

use std::collections::BTreeMap;

use grenat_ast::Span;

use crate::*;

/// The interactivity route of `approvals_in_slack`, unless `path:` says otherwise.
pub(crate) const CLICKS_PATH: &str = "/slack/interactivity";

/// What the declarations seen claimed, by their position (a function
/// checked twice records once).
#[derive(Default)]
pub(crate) struct ChatRoutes {
    /// `approvals_in_slack`, with its literal path (`None`: computed).
    slack: BTreeMap<(u32, u32), (Span, Option<String>)>,
    /// `on_webhook`'s literal paths.
    webhooks: BTreeMap<(u32, u32), (Span, String)>,
    /// The web routes' methods and literal paths.
    routes: BTreeMap<(u32, u32), (Span, String, String)>,
}

fn key(span: Span) -> (u32, u32) {
    (span.start, span.end)
}

/// Whether a route's `pattern` (`/a/:id`) matches `path`, as the server
/// matches them.
fn route_matches(pattern: &str, path: &str) -> bool {
    let segments = |p: &str| p.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (pattern, path) = (segments(pattern), segments(path));
    pattern.len() == path.len() && pattern.iter().zip(&path).all(|(p, part)| p.starts_with(':') || p == part)
}

impl<'p> Checker<'p> {
    pub(crate) fn claim_slack_route(&mut self, span: Span, path: Option<String>) {
        self.chat_routes.slack.insert(key(span), (span, path));
    }

    pub(crate) fn claim_webhook_route(&mut self, span: Span, path: String) {
        self.chat_routes.webhooks.insert(key(span), (span, path));
    }

    pub(crate) fn claim_web_route(&mut self, span: Span, method: &str, path: String) {
        self.chat_routes.routes.insert(key(span), (span, method.to_string(), path));
    }

    /// The claims compared, once every declaration was seen.
    pub(crate) fn chat_routes_claimed(&mut self) {
        let routes = std::mem::take(&mut self.chat_routes);
        let mut slack = routes.slack.values();
        let Some((declared, path)) = slack.next() else { return };
        for (again, _) in slack {
            self.error(E_TYPE, *again, "`approvals_in_slack` is declared once");
        }
        let Some(path) = path else { return };
        for (webhook, webhook_path) in routes.webhooks.values().filter(|(_, p)| p == path) {
            if webhook.start > declared.start {
                self.error(E_TYPE, *webhook, format!("`{webhook_path}` is the route of `approvals_in_slack`"));
            } else {
                let message = format!("`{path}` is already a webhook: give `approvals_in_slack` another `path:`");
                self.error(E_TYPE, *declared, message);
            }
        }
        for (route, method, pattern) in routes.routes.values().filter(|(_, _, p)| route_matches(p, path)) {
            let message = format!(
                "`{method} \"{pattern}\"` would never be reached: `{path}` is the route of `approvals_in_slack` (give it another `path:`)"
            );
            self.error(E_TYPE, *route, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_match_as_served() {
        assert!(route_matches("/slack/interactivity", "/slack/interactivity"));
        assert!(route_matches("/slack/:kind", "/slack/interactivity"));
        assert!(route_matches("/slack/interactivity/", "/slack/interactivity"));
        assert!(!route_matches("/slack", "/slack/interactivity"));
        assert!(!route_matches("/slack/events", "/slack/interactivity"));
    }
}
