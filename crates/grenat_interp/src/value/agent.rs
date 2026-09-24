//! Actor agents and their supervision.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Only the crashed agent restarts.
    OneForOne,
    /// Every child of the supervisor restarts.
    OneForAll,
    /// The agent and the children declared after it restart.
    RestForOne,
}

#[derive(Debug, Clone)]
pub struct Supervision {
    pub supervisor: String,
    pub strategy: Strategy,
    pub max_restarts: usize,
    /// Window over which restarts are counted, in seconds.
    pub within: f64,
}

/// An actor agent: its state and the lock that makes it handle one message at a time.
pub struct AgentRef<'p> {
    pub id: u64,
    pub ty: Arc<str>,
    /// Replaced by a fresh state when the supervisor restarts the agent.
    pub state: Mutex<Arc<Object<'p>>>,
    /// Held while a message is being handled.
    pub turn: Mutex<()>,
    /// Task currently handling a message (deadlock detection).
    pub owner: Mutex<Option<u64>>,
    /// Pending messages (pool load balancing).
    pub queued: AtomicUsize,
    pub budget: Option<Arc<Budget>>,
    pub supervision: Option<Supervision>,
    pub restarts: Mutex<Vec<Instant>>,
    /// Why the agent was stopped for good (too many restarts).
    pub down: Mutex<Option<String>>,
}

impl AgentRef<'_> {
    pub fn load(&self) -> usize {
        self.queued.load(Ordering::Relaxed) + usize::from(self.owner.borrow().is_some())
    }
}
