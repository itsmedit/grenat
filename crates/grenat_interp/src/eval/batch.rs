//! `xs.batch_map { |x| … }`: the model calls of the block go through the
//! provider's batch API — half the price, answered later.
//!
//! Each element runs on a task of its own. A task that calls a model
//! queues its request and waits; once every task is waiting or done, the
//! queued requests go out as one batch, and each answer wakes its task,
//! which goes on (and may call a model again: another round). Tasks that a
//! batched task starts (`parallel_map`…) call models directly.

use grenat_llm::{LlmError, ModelConfig, Request, Response};
use serde_json::Value as Json;

use crate::prelude::*;

/// A request waiting for its batch, and who waits for its answer.
struct Queued {
    model: ModelConfig,
    system: Option<String>,
    messages: Vec<Json>,
    tools: Vec<grenat_llm::ToolSpec>,
    output_schema: Option<Json>,
    answer: grenat_green::Sender<Result<Response, LlmError>>,
}

#[derive(Default)]
struct State {
    /// Tasks running (neither waiting for an answer nor done).
    running: usize,
    queued: Vec<Queued>,
}

/// The batch a task's model calls go to.
pub(crate) struct BatchRun {
    state: grenat_green::Mutex<State>,
    changed: grenat_green::Condvar,
}

impl BatchRun {
    /// Queues `request` and waits for its answer (the task sleeps meanwhile).
    pub(crate) fn call(&self, request: &Request) -> Result<Response, LlmError> {
        let (answer, receiver) = grenat_green::channel();
        {
            let mut state = self.state.lock();
            state.queued.push(Queued {
                model: request.model.clone(),
                system: request.system.clone(),
                messages: request.messages.clone(),
                tools: request.tools.clone(),
                output_schema: request.output_schema.clone(),
                answer,
            });
            state.running -= 1;
        }
        self.changed.notify_all();
        receiver.recv().unwrap_or_else(|| Err(LlmError { message: "the batch ended without an answer".into() }))
    }

    fn finished(&self) {
        self.state.lock().running -= 1;
        self.changed.notify_all();
    }
}

impl<'p> Interp<'p> {
    pub(crate) fn batch_map(&mut self, items: Vec<Value<'p>>, block: Value<'p>) -> R<'p> {
        let count = items.len();
        let run = Arc::new(BatchRun {
            state: grenat_green::Mutex::new(State { running: count, queued: Vec::new() }),
            changed: grenat_green::Condvar::new(),
        });
        let (sender, receiver) = grenat_green::channel();
        for (i, item) in items.into_iter().enumerate() {
            let mut child = self.fork();
            child.batch = Some(run.clone());
            let (run, sender, block) = (run.clone(), sender.clone(), block.clone());
            self.spawn_task(child, move |task| {
                let result = task.call_block(&block, vec![item]);
                let _ = sender.send((i, result));
                run.finished();
            });
        }
        drop(sender);
        // rounds: wait until no task runs, send what they queued
        loop {
            let queued = {
                let mut state = run.state.lock();
                while state.running > 0 {
                    state = run.changed.wait(state);
                }
                let queued = std::mem::take(&mut state.queued);
                state.running += queued.len();
                queued
            };
            if queued.is_empty() {
                break;
            }
            self.send_batch(queued)?;
        }
        let mut results: Vec<Option<R<'p>>> = (0..count).map(|_| None).collect();
        for (i, result) in receiver {
            results[i] = Some(result);
        }
        let mut values = Vec::with_capacity(count);
        for result in results {
            values.push(result.expect("every task sends its result")?);
        }
        Ok(Value::array(values))
    }

    /// One round: the queued requests, grouped by who answers them.
    fn send_batch(&mut self, queued: Vec<Queued>) -> Result<(), Ctrl<'p>> {
        if self.log {
            self.write_err(&format!("[batch] {} request(s)\n", queued.len()));
        }
        let mut groups: Vec<(Arc<dyn grenat_llm::Provider>, Vec<Queued>)> = Vec::new();
        for q in queued {
            let provider = self.provider(&q.model)?;
            match groups.iter_mut().find(|(p, _)| Arc::ptr_eq(p, &provider)) {
                Some((_, group)) => group.push(q),
                None => groups.push((provider, vec![q])),
            }
        }
        for (provider, group) in groups {
            let requests: Vec<Request> = group
                .iter()
                .map(|q| Request {
                    model: &q.model,
                    system: q.system.clone(),
                    messages: q.messages.clone(),
                    tools: q.tools.clone(),
                    output_schema: q.output_schema.clone(),
                })
                .collect();
            let answers = grenat_green::blocking(|| provider.batch(&requests));
            match answers {
                Ok(answers) => {
                    for (q, answer) in group.iter().zip(answers) {
                        let _ = q.answer.send(answer);
                    }
                }
                Err(e) => {
                    for q in &group {
                        let _ = q.answer.send(Err(e.clone()));
                    }
                }
            }
        }
        Ok(())
    }
}
