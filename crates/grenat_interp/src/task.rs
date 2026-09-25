//! Concurrent tasks: spawning, waiting, cancellation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::stack::{StackGuard, TASK_STACK};
use crate::value::new_scope;
use crate::*;

/// A task: a green thread of the run (see `grenat_green`).
pub(crate) type Task<'p> = Box<dyn FnOnce() + Send + 'p>;
pub(crate) type Spawner<'p> = Arc<dyn Fn(Task<'p>) + Send + Sync + 'p>;

/// Call depth limit of secondary tasks (their stack is smaller, see `stack`).
pub(crate) const TASK_DEPTH: usize = 2_000;

/// Checks of cancellation between two yields to the other tasks.
const STEPS_PER_YIELD: u32 = 4096;

pub(crate) fn spawner<'e>(green: &grenat_green::Spawner<'e>) -> Spawner<'e> {
    let green = green.clone();
    Arc::new(move |task: Task<'e>| green.spawn(task))
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
            steps: Default::default(),
            task_id: self.next_id.fetch_add(1, Ordering::Relaxed),
            // measured once the task's own thread starts
            stack: StackGuard::default(),
            workflows: self.workflows.clone(),
            providers: self.providers.clone(),
            // tasks a batched task starts call models directly
            batch: None,
        }
    }

    /// Runs `work` in a new task.
    pub(crate) fn spawn_task(&self, child: Interp<'p>, work: impl FnOnce(&mut Interp<'p>) + Send + 'p) {
        *self.active.lock() += 1;
        let shared = self.shared.clone();
        (self.spawner)(Box::new(move || {
            let mut child = child;
            child.stack = StackGuard::here(TASK_STACK);
            work(&mut child);
            drop(child);
            let mut active = shared.active.lock();
            *active -= 1;
            shared.idle.notify_all();
        }));
    }

    /// Waits for secondary tasks to finish (end of program, end of test).
    pub(crate) fn wait_for_tasks(&self) {
        let mut active = self.active.lock();
        while *active > 0 {
            active = self.idle.wait(active);
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.iter().any(|flag| flag.load(Ordering::Relaxed))
    }

    /// At every call and loop iteration: stops a cancelled task, and now and
    /// then lets the other tasks of this worker run (green threads are
    /// cooperative).
    pub(crate) fn check_cancel(&self) -> Result<(), Ctrl<'p>> {
        let steps = self.steps.get().wrapping_add(1);
        self.steps.set(steps);
        if steps.is_multiple_of(STEPS_PER_YIELD) {
            grenat_green::yield_now();
        }
        if self.cancelled() { raise("Cancelled", "task cancelled") } else { Ok(()) }
    }
}
