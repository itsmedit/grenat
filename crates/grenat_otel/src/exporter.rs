//! The batching exporter: ended spans wait in a bounded queue, and a
//! thread of their own sends them in batches — when a batch is full, when
//! the delay has passed, or when asked to flush.
//!
//! The program never waits for it: queuing a span takes a lock held only
//! to push or pop, and a span that finds the queue full is dropped and
//! counted. A collector down or slow costs the exporter's thread its time
//! (each export bounded by its timeout), never the program's. A batch the
//! collector may take later (`429`, `503`, unreachable…) is sent again a
//! few times (see [`crate::retry`]); one that fails otherwise, or every
//! time, is dropped and counted. Spans a collector rejects in an accepted
//! batch (a partial success) are counted as dropped, and said once.
//! Flushing waits at most the time it is given.
//!
//! Nothing queued, the thread sleeps until a span comes (no timer runs:
//! an idle exporter costs nothing); the delay starts with the first span.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::answer::Export;
use crate::span::{Attributes, Span};
use crate::transport::Transport;
use crate::{encode, retry};

/// The shortest delay: spans are batched for a millisecond at least.
pub const MIN_DELAY: Duration = Duration::from_millis(1);

/// What the exporter did so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// Spans the collector accepted.
    pub exported: u64,
    /// Spans dropped: the queue was full, or the collector rejected them.
    pub dropped: u64,
    /// Of those dropped, the spans the collector rejected (a partial success).
    pub rejected: u64,
    /// What the collector said of the spans it rejected first.
    pub rejection: Option<String>,
    /// Spans of batches the collector did not accept (or never answered).
    pub failed: u64,
    /// Why the last batch failed, if one did.
    pub last_error: Option<String>,
    /// Batches sent again.
    pub retries: u64,
    /// Times the exporter's thread went round: idle, it sleeps.
    pub rounds: u64,
}

/// How the exporter batches.
#[derive(Debug, Clone, Copy)]
pub struct Batching {
    pub max_queue: usize,
    pub max_batch: usize,
    pub delay: Duration,
}

struct State {
    spans: VecDeque<Span>,
    /// Flushes asked, and the last one done: a flush is done once every
    /// span queued before it was handled.
    asked: u64,
    done: u64,
    closing: bool,
    stats: Stats,
}

struct Queue {
    state: Mutex<State>,
    /// Wakes the exporter's thread.
    work: Condvar,
    /// Wakes those waiting for a flush.
    flushed: Condvar,
    batching: Batching,
}

impl Queue {
    /// The state, even if a thread panicked holding it.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Spans queued, sent by a thread of their own.
pub struct Exporter {
    queue: Arc<Queue>,
}

impl Exporter {
    /// Starts the exporter's thread: batches of `resource`'s spans go
    /// through `transport`. `None` if the thread cannot start.
    pub fn start(resource: Attributes, transport: Box<dyn Transport>, batching: Batching) -> Option<Exporter> {
        let batching = Batching {
            max_queue: batching.max_queue.max(1),
            max_batch: batching.max_batch.max(1),
            delay: batching.delay.max(MIN_DELAY),
        };
        let queue = Arc::new(Queue {
            state: Mutex::new(State {
                spans: VecDeque::new(),
                asked: 0,
                done: 0,
                closing: false,
                stats: Stats::default(),
            }),
            work: Condvar::new(),
            flushed: Condvar::new(),
            batching,
        });
        let worker = queue.clone();
        std::thread::Builder::new()
            .name("grenat-otel".into())
            .spawn(move || export_loop(&worker, &resource, transport.as_ref()))
            .ok()?;
        Some(Exporter { queue })
    }

    /// Queues an ended span; dropped (and counted) if the queue is full.
    pub fn push(&self, span: Span) {
        let mut state = self.queue.lock();
        if state.closing || state.spans.len() >= self.queue.batching.max_queue {
            state.stats.dropped += 1;
            return;
        }
        state.spans.push_back(span);
        // the first span starts the delay; a full batch ends it
        if state.spans.len() == 1 || state.spans.len() >= self.queue.batching.max_batch {
            self.queue.work.notify_one();
        }
    }

    /// Sends what is queued now, waiting `timeout` at most: whether it was
    /// all handled (exported or failed) in time.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.queue.lock();
        state.asked += 1;
        let mine = state.asked;
        self.queue.work.notify_one();
        while state.done < mine {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self.queue.flushed.wait_timeout(state, left).unwrap_or_else(|p| p.into_inner()).0;
        }
        true
    }

    /// Flushes (waiting `timeout` at most), then stops: spans ended later
    /// are dropped.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let flushed = self.flush(timeout);
        let mut state = self.queue.lock();
        state.closing = true;
        self.queue.work.notify_one();
        flushed
    }

    pub fn stats(&self) -> Stats {
        self.queue.lock().stats.clone()
    }
}

/// The exporter's thread: sleeps until a span, a flush or the end; then
/// waits for a full batch, the delay, a flush or the end; sends; and again.
fn export_loop(queue: &Queue, resource: &Attributes, transport: &dyn Transport) {
    let batching = queue.batching;
    loop {
        let (batch, flush, closing) = {
            let mut state = queue.lock();
            state.stats.rounds += 1;
            let idle = |state: &State| state.asked == state.done && !state.closing;
            while state.spans.is_empty() && idle(&state) {
                state = queue.work.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            let deadline = Instant::now() + batching.delay;
            while state.spans.len() < batching.max_batch && idle(&state) {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                state = queue.work.wait_timeout(state, left).unwrap_or_else(|p| p.into_inner()).0;
            }
            let take = state.spans.len().min(batching.max_batch);
            let batch: Vec<Span> = state.spans.drain(..take).collect();
            (batch, state.asked, state.closing)
        };
        if !batch.is_empty() {
            let body = encode::request(resource, &batch).to_string().into_bytes();
            let sent = export(queue, transport, &body);
            let mut state = queue.lock();
            let spans = batch.len() as u64;
            match sent {
                Export::Accepted { rejected, message } => {
                    let rejected = rejected.min(spans);
                    state.stats.exported += spans - rejected;
                    if rejected > 0 {
                        state.stats.dropped += rejected;
                        state.stats.rejected += rejected;
                        if state.stats.rejection.is_none() {
                            let why = message.unwrap_or_else(|| "no reason given".into());
                            eprintln!("warning: OpenTelemetry: the collector rejected {rejected} spans ({why})");
                            state.stats.rejection = Some(why);
                        }
                    }
                }
                Export::Retry { why, .. } | Export::Failed(why) => {
                    state.stats.failed += spans;
                    state.stats.last_error = Some(why);
                }
            }
        }
        let mut state = queue.lock();
        if state.spans.is_empty() {
            // every span queued before these flushes is handled
            state.done = state.done.max(flush);
            queue.flushed.notify_all();
            if closing {
                return;
            }
        }
    }
}

/// Sends `body`, again while the collector may take it later: what became
/// of it in the end (a [`Export::Retry`] when the attempts ran out).
fn export(queue: &Queue, transport: &dyn Transport, body: &[u8]) -> Export {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match transport.send(body) {
            Export::Retry { why, retry_after } => {
                let Some(wait) = retry::delay(attempts, retry_after, crate::ids::jitter()) else {
                    return Export::Retry { why, retry_after };
                };
                queue.lock().stats.retries += 1;
                std::thread::sleep(wait);
            }
            done => return done,
        }
    }
}
