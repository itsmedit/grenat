//! What keeps the tests of a file apart (`grenat test`): each test starts
//! from the state the script left — a new in-memory database with every
//! migration applied, journals of its own, no double, no email delivered,
//! no request sent, no frozen clock, full rate-limit buckets — and nothing
//! a test declared or started survives it (an approval handler, approvals
//! in Slack, a trigger, a route, an MCP server, a supervisor's children).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use crate::prelude::*;

/// What the script declared, before the first test: each test ends by
/// putting it back.
pub(crate) struct ScriptState<'p> {
    approver: Option<Value<'p>>,
    slack_approvals: Option<crate::slack::SlackApprovals>,
    children: HashMap<(String, String), Arc<AgentRef<'p>>>,
    mcp_servers: HashMap<String, Arc<crate::eval::mcp::McpServer>>,
    schedules: usize,
    webhooks: usize,
    mailboxes: usize,
    folders: usize,
    routes: usize,
    exposures: usize,
    migrations: usize,
}

impl<'p> Interp<'p> {
    /// What the script declared (taken once it has run).
    pub(crate) fn script_state(&self) -> ScriptState<'p> {
        ScriptState {
            approver: self.approver.borrow().clone(),
            slack_approvals: self.slack_approvals.borrow().clone(),
            children: self.children.borrow().clone(),
            mcp_servers: self.mcp_servers.borrow().clone(),
            schedules: self.schedules.borrow().len(),
            webhooks: self.webhooks.borrow().len(),
            mailboxes: self.mailboxes.borrow().len(),
            folders: self.folders.borrow().len(),
            routes: self.routes.borrow().len(),
            exposures: self.exposures.borrow().len(),
            migrations: self.migrations.borrow().len(),
        }
    }

    /// Before a test: a database of its own, migrated (the operations
    /// store — jobs, approvals, model calls, events, files handled — is
    /// in it), and journals of its own: no test resumes another's workflows.
    pub(crate) fn begin_test(&mut self, journal: &std::path::Path) -> Result<(), Ctrl<'p>> {
        *self.journal_dir.borrow_mut() = journal.to_path_buf();
        self.testing.store(true, Ordering::SeqCst);
        self.fresh_test_database()
    }

    /// After a test: its doubles, what it recorded and what it declared
    /// are gone; the script's declarations are back.
    pub(crate) fn end_test(&mut self, script: &ScriptState<'p>) {
        self.testing.store(false, Ordering::SeqCst);
        // each test declares its own doubles
        self.mocks.borrow_mut().clear();
        self.embedding_mocks.borrow_mut().clear();
        self.transcription_mocks.borrow_mut().clear();
        self.http_stubs.borrow_mut().clear();
        self.shell_stubs.borrow_mut().clear();
        self.mcp_stubs.borrow_mut().clear();
        self.ssh_stubs.borrow_mut().clear();
        self.mail_failure.borrow_mut().take();
        self.env_double.borrow_mut().clear();
        self.credentials_double.borrow_mut().take();
        self.human_double.borrow_mut().take();
        self.frozen_clock.borrow_mut().take();
        // and sees only what it sent
        self.deliveries.borrow_mut().clear();
        self.sent_requests.borrow_mut().clear();
        *self.slack_replays.borrow_mut() = Default::default();
        self.unpriced.borrow_mut().clear();
        self.end_test_limits();
        // what the test declared or started goes with it
        self.app_db.borrow_mut().take();
        self.approver.borrow_mut().clone_from(&script.approver);
        self.slack_approvals.borrow_mut().clone_from(&script.slack_approvals);
        self.children.borrow_mut().clone_from(&script.children);
        self.mcp_servers.borrow_mut().clone_from(&script.mcp_servers);
        self.schedules.borrow_mut().truncate(script.schedules);
        self.webhooks.borrow_mut().truncate(script.webhooks);
        self.mailboxes.borrow_mut().truncate(script.mailboxes);
        self.folders.borrow_mut().truncate(script.folders);
        self.routes.borrow_mut().truncate(script.routes);
        self.exposures.borrow_mut().truncate(script.exposures);
        self.migrations.borrow_mut().truncate(script.migrations);
    }
}
