//! Databases reached over the network, checked: `Db.connect(url)` and
//! `database url` with a literal PostgreSQL URL whose host is not this
//! machine are `net` on that host (`uses net("db.acme.io")` or `net` must
//! grant it, E0300) — read by `grenat_db_url`, as the runtime reads it.
//! SQLite and local servers (`localhost`, `127.0.0.1`, `::1`, a Unix
//! socket) need no `net`. A URL from configuration (`Credentials.fetch`,
//! `Env.fetch`) is the runtime's to check, when it connects
//! (`CapabilityError`): no effect here.
//!
//! As for `on_email`, the effect falls on the code around: at the top
//! level, where `database` is declared, nothing is restricted. Connecting
//! gives the same handle on every run: a workflow needs no `step` for it.

use grenat_ast::Span;

use crate::*;

impl<'p> Checker<'p> {
    /// The `net` effects of connecting to the database `argv` names first.
    pub(crate) fn database_url(&mut self, cx: &mut Ctx<'p>, span: Span, argv: &[ArgV]) {
        let Some(url) = argv.iter().find(|a| a.name.is_none()).and_then(|a| a.lit.as_deref()) else { return };
        for host in grenat_db_url::remote_hosts(url) {
            cx.require_effect(Eff { path: "net".into(), arg: Some(host), origin: span });
        }
    }
}
