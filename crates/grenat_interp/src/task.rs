//! Concurrent tasks: spawning, waiting, cancellation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::value::{Locked, new_scope};
use crate::*;

/// Task launched on a thread of the run's scope.
pub(crate) type Task<'p> = Box<dyn FnOnce() + Send + 'p>;
pub(crate) type Spawner<'p> = Arc<dyn Fn(Task<'p>) + Send + Sync + 'p>;

/// Stack of secondary tasks (the main task has 512 MB, see the CLI).
pub(crate) const TASK_STACK: usize = 128 * 1024 * 1024;
pub(crate) const TASK_DEPTH: usize = 5_000;

pub(crate) fn spawner<'s, 'e>(scope: &'s std::thread::Scope<'s, 'e>) -> Spawner<'s> {
    Arc::new(move |task: Task<'s>| {
        std::thread::Builder::new().stack_size(TASK_STACK).spawn_scoped(scope, task).expect("spawning a task");
    })
}

impl<'p> Interp<'p> {
    /// New task inheriting this one's budgets, capabilities, agents and cancellation flags.
    pub(crate) fn fork(&self) -> Interp<'p> {
        Interp {
            shared: self.shared.clone(),
            frames: vec![Frame { self_val: None, scope: new_scope(None) }],
            prompts: Vec::new(),
            agents: self.agents.clone(),
            budgets: self.budgets.clone(),
            capabilities: self.capabilities.clone(),
            depth: 0,
            max_depth: TASK_DEPTH,
            cancel: self.cancel.clone(),
            task_id: self.next_id.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Runs `work` in a new task.
    pub(crate) fn spawn_task(&self, child: Interp<'p>, work: impl FnOnce(&mut Interp<'p>) + Send + 'p) {
        *self.active.borrow_mut() += 1;
        let shared = self.shared.clone();
        (self.spawner)(Box::new(move || {
            let mut child = child;
            work(&mut child);
            drop(child);
            let mut active = shared.active.borrow_mut();
            *active -= 1;
            shared.idle.notify_all();
        }));
    }

    /// Waits for secondary tasks to finish (end of program, end of test).
    pub(crate) fn wait_for_tasks(&self) {
        let mut active = self.active.borrow_mut();
        while *active > 0 {
            active = self.idle.wait(active).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.iter().any(|flag| flag.load(Ordering::Relaxed))
    }

    pub(crate) fn check_cancel(&self) -> Result<(), Ctrl<'p>> {
        if self.cancelled() { raise("Cancelled", "task cancelled") } else { Ok(()) }
    }
}
