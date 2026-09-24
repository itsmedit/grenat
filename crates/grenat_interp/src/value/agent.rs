//! Agents-acteurs et leur supervision.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Seul l'agent qui a planté redémarre.
    OneForOne,
    /// Tous les enfants du superviseur redémarrent.
    OneForAll,
    /// L'agent et les enfants déclarés après lui redémarrent.
    RestForOne,
}

#[derive(Debug, Clone)]
pub struct Supervision {
    pub supervisor: String,
    pub strategy: Strategy,
    pub max_restarts: usize,
    /// Fenêtre de comptage des redémarrages, en secondes.
    pub within: f64,
}

/// Un agent-acteur : son état et le verrou qui garantit qu'il traite un message à la fois.
pub struct AgentRef<'p> {
    pub id: u64,
    pub ty: Arc<str>,
    /// Remplacé par un état neuf quand le superviseur redémarre l'agent.
    pub state: Mutex<Arc<Object<'p>>>,
    /// Tenu pendant le traitement d'un message.
    pub turn: Mutex<()>,
    /// Tâche en train de traiter un message (détection d'interblocage).
    pub owner: Mutex<Option<u64>>,
    /// Messages en attente (répartition dans un pool).
    pub queued: AtomicUsize,
    pub budget: Option<Arc<Budget>>,
    pub supervision: Option<Supervision>,
    pub restarts: Mutex<Vec<Instant>>,
    /// Raison de l'arrêt définitif (trop de redémarrages).
    pub down: Mutex<Option<String>>,
}

impl AgentRef<'_> {
    pub fn load(&self) -> usize {
        self.queued.load(Ordering::Relaxed) + usize::from(self.owner.borrow().is_some())
    }
}
