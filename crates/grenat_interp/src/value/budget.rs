//! Budgets de dépense (dollars, tokens, temps).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::*;

/// Plafond de dépense : tokens, dollars, temps. Partagé par les tâches qui le consomment.
pub struct Budget {
    pub max_usd: Option<f64>,
    pub max_tokens: Option<u64>,
    pub max_seconds: Option<f64>,
    spent_usd: Mutex<f64>,
    tokens: AtomicU64,
    started: Mutex<Instant>,
}

impl Budget {
    pub fn unlimited() -> Self {
        Budget {
            max_usd: None,
            max_tokens: None,
            max_seconds: None,
            spent_usd: Mutex::new(0.0),
            tokens: AtomicU64::new(0),
            started: Mutex::new(Instant::now()),
        }
    }

    pub fn spent(&self) -> f64 {
        *self.spent_usd.borrow()
    }

    pub fn tokens(&self) -> u64 {
        self.tokens.load(Ordering::Relaxed)
    }

    pub fn add(&self, usd: f64, tokens: u64) {
        *self.spent_usd.borrow_mut() += usd;
        self.tokens.fetch_add(tokens, Ordering::Relaxed);
    }

    pub fn restart_clock(&self) {
        *self.started.borrow_mut() = Instant::now();
    }

    /// Description du dépassement, s'il y en a un.
    pub fn exceeded(&self) -> Option<String> {
        if let Some(max) = self.max_usd
            && self.spent() > max
        {
            return Some(format!("{} dépensés, plafond {}", money(self.spent()), money(max)));
        }
        if let Some(max) = self.max_tokens
            && self.tokens() > max
        {
            return Some(format!("{} tokens consommés, plafond {max}", self.tokens()));
        }
        if let Some(max) = self.max_seconds
            && self.started.borrow().elapsed().as_secs_f64() > max
        {
            return Some(format!("temps écoulé, plafond {}", duration(max)));
        }
        None
    }
}
