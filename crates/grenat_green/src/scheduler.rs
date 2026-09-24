//! The workers, the run queue, and the current task.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult};

use crate::task::{Co, DONE, NOTIFIED, PARKED, QUEUED, RUNNING, Suspend, Task, Waiter};

/// Sizes of the scheduler.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// OS threads running tasks.
    pub workers: usize,
    /// Stack reserved for the first task.
    pub main_stack: usize,
    /// Stack reserved for every other task.
    pub task_stack: usize,
}

impl Default for Config {
    fn default() -> Self {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        Config { workers, main_stack: 64 << 20, task_stack: 1 << 20 }
    }
}

pub(crate) struct Shared {
    queue: Mutex<VecDeque<Arc<Task>>>,
    ready: Condvar,
    /// Tasks not finished yet: the workers stop when it reaches zero.
    live: AtomicUsize,
    task_stack: usize,
    /// Stacks of finished tasks, reused.
    stacks: Mutex<Vec<DefaultStack>>,
}

impl Shared {
    pub fn push(&self, task: Arc<Task>) {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).push_back(task);
        self.ready.notify_one();
    }

    fn stack(&self, size: usize) -> DefaultStack {
        if size == self.task_stack
            && let Some(stack) = self.stacks.lock().unwrap_or_else(PoisonError::into_inner).pop()
        {
            return stack;
        }
        DefaultStack::new(size).expect("reserving a task stack")
    }

    /// Queues a new task running `f` on a stack of `size` bytes.
    ///
    /// # Safety
    /// Whatever `f` borrows must outlive the task: [`run`] waits for all tasks.
    unsafe fn spawn(self: &Arc<Shared>, size: usize, f: Box<dyn FnOnce() + Send + '_>) {
        let stack = self.stack(size);
        // SAFETY: by contract, `f`'s borrows outlive the task
        let co: Co = unsafe {
            Coroutine::with_stack_unchecked(stack, move |yielder, ()| {
                current().expect("a task").yielder.set(yielder);
                f();
            })
        };
        let task = Arc::new(Task {
            state: QUEUED.into(),
            co: Some(co).into(),
            yielder: std::cell::Cell::new(std::ptr::null()),
            shared: self.clone(),
        });
        self.live.fetch_add(1, Ordering::AcqRel);
        self.push(task);
    }
}

thread_local! {
    /// The task this worker is running.
    static CURRENT: RefCell<Option<Arc<Task>>> = const { RefCell::new(None) };
}

// A task may resume on another thread, but the compiler assumes a function
// runs on one thread and may reuse the address of a thread-local computed
// before a suspension. `CURRENT` is therefore only read in functions that
// are never inlined: each call computes the address afresh.

#[inline(never)]
pub(crate) fn current() -> Option<Arc<Task>> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Running inside a green task.
#[inline(never)]
pub fn in_task() -> bool {
    CURRENT.with(|c| c.borrow().is_some())
}

#[inline(never)]
fn set_current(task: Option<Arc<Task>>) {
    CURRENT.with(|c| *c.borrow_mut() = task);
}

#[inline(never)]
pub(crate) fn current_waiter() -> Waiter {
    match current() {
        Some(task) => Waiter::Task(task),
        None => Waiter::Thread(std::thread::current()),
    }
}

fn suspend(why: Suspend) {
    let task = current().expect("a task");
    let yielder = task.yielder.get();
    drop(task);
    // SAFETY: set by the task when it started; it lives on the task's stack,
    // which is the stack running this code
    unsafe { (*yielder).suspend(why) };
}

/// Waits until woken (a task parks, a thread parks). May return spuriously:
/// callers check their condition again.
pub(crate) fn park() {
    if in_task() { suspend(Suspend::Park) } else { std::thread::park() }
}

/// Lets the other tasks of this worker run (no-op outside a task).
pub fn yield_now() {
    if in_task() {
        suspend(Suspend::Yield);
    }
}

/// Spawns green tasks that may borrow from the scope of [`run`].
pub struct Spawner<'env> {
    shared: Arc<Shared>,
    _env: PhantomData<&'env ()>,
}

impl<'env> Spawner<'env> {
    pub fn spawn(&self, f: impl FnOnce() + Send + 'env) {
        // SAFETY: `run` returns only once every task has finished
        unsafe { self.shared.spawn(self.shared.task_stack, Box::new(f)) }
    }
}

impl Clone for Spawner<'_> {
    fn clone(&self) -> Self {
        Spawner { shared: self.shared.clone(), _env: PhantomData }
    }
}

/// Runs `f` as the first task, on `config.workers` threads, and returns its
/// result once **every** task (including those spawned) has finished.
pub fn run<'env, T: Send + 'env>(config: Config, f: impl FnOnce(&Spawner<'env>) -> T + Send + 'env) -> T {
    let shared = Arc::new(Shared {
        queue: Mutex::new(VecDeque::new()),
        ready: Condvar::new(),
        live: AtomicUsize::new(0),
        task_stack: config.task_stack,
        stacks: Mutex::new(Vec::new()),
    });
    let result = Arc::new(Mutex::new(None));
    let spawner = Spawner { shared: shared.clone(), _env: PhantomData };
    let slot = result.clone();
    let main = move || {
        let value = f(&spawner);
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
    };
    // SAFETY: the workers below run until every task has finished
    unsafe { shared.spawn(config.main_stack, Box::new(main)) };
    std::thread::scope(|scope| {
        for i in 0..config.workers.max(1) {
            let shared = &shared;
            std::thread::Builder::new()
                .name(format!("grenat-worker-{i}"))
                .spawn_scoped(scope, move || work(shared))
                .expect("starting a worker");
        }
    });
    let value = result.lock().unwrap_or_else(PoisonError::into_inner).take();
    value.expect("the first task finished")
}

fn work(shared: &Arc<Shared>) {
    loop {
        let task = {
            let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(task) = queue.pop_front() {
                    break task;
                }
                if shared.live.load(Ordering::Acquire) == 0 {
                    return;
                }
                queue = shared.ready.wait(queue).unwrap_or_else(PoisonError::into_inner);
            }
        };
        task.state.store(RUNNING, Ordering::Release);
        set_current(Some(task.clone()));
        // SAFETY: this worker alone runs the task (it took it from the queue)
        let co = unsafe { (*task.co.get()).as_mut().expect("a coroutine") };
        let outcome = co.resume(());
        set_current(None);
        match outcome {
            CoroutineResult::Return(()) => {
                // SAFETY: as above; the task is finished
                let co = unsafe { (*task.co.get()).take().expect("a coroutine") };
                task.state.store(DONE, Ordering::Release);
                let stack = co.into_stack();
                shared.stacks.lock().unwrap_or_else(PoisonError::into_inner).push(stack);
                if shared.live.fetch_sub(1, Ordering::AcqRel) == 1 {
                    // the last task: wake every idle worker so that they stop
                    let _queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
                    shared.ready.notify_all();
                }
            }
            CoroutineResult::Yield(Suspend::Yield) => {
                task.state.store(QUEUED, Ordering::Release);
                shared.push(task);
            }
            CoroutineResult::Yield(Suspend::Park) => {
                // woken meanwhile: it must run again
                if task.state.compare_exchange(RUNNING, PARKED, Ordering::AcqRel, Ordering::Acquire).is_err() {
                    debug_assert_eq!(task.state.load(Ordering::Acquire), NOTIFIED);
                    task.state.store(QUEUED, Ordering::Release);
                    shared.push(task);
                }
            }
        }
    }
}
