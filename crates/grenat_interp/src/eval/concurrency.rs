//! Structured concurrency: `parallel_map` and `race`.

use crate::prelude::*;

impl<'p> Interp<'p> {
    // ── Concurrency ──────────────────────────────────────────

    /// `xs.parallel_map(limit: n) { |x| … }`: at most `limit` tasks at once, results in order.
    pub(crate) fn parallel_map(&mut self, items: Vec<Value<'p>>, block: Value<'p>, limit: usize) -> R<'p> {
        if items.len() <= 1 || limit <= 1 {
            let results =
                items.into_iter().map(|item| self.call_block(&block, vec![item])).collect::<Result<Vec<_>, _>>()?;
            return Ok(Value::array(results));
        }
        let count = items.len();
        let items = Arc::new(items);
        let next = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel();
        for _ in 0..limit.min(count) {
            let mut child = self.fork();
            child.cancel.push(stop.clone());
            let (items, next, stop, sender, block) =
                (items.clone(), next.clone(), stop.clone(), sender.clone(), block.clone());
            self.spawn_task(child, move |task| {
                loop {
                    let i = next.fetch_add(1, AtomicOrdering::Relaxed);
                    if i >= count || task.cancelled() {
                        break;
                    }
                    let result = task.call_block(&block, vec![items[i].clone()]);
                    let failed = result.is_err();
                    if sender.send((i, result)).is_err() || failed {
                        // the first error cancels the rest
                        stop.store(true, AtomicOrdering::Relaxed);
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut results: Vec<Option<Value<'p>>> = vec![None; count];
        let mut first_error: Option<(usize, Ctrl<'p>)> = None;
        for (i, result) in receiver {
            match result {
                Ok(v) => results[i] = Some(v),
                Err(Ctrl::Break(v)) => {
                    stop.store(true, AtomicOrdering::Relaxed);
                    return Ok(v);
                }
                Err(e) if e.error_type() == Some("Cancelled") => {}
                Err(e) => {
                    if first_error.as_ref().is_none_or(|(j, _)| i < *j) {
                        first_error = Some((i, e));
                    }
                }
            }
        }
        if let Some((_, e)) = first_error {
            return Err(e);
        }
        self.check_cancel()?;
        Ok(Value::array(results.into_iter().map(|v| v.unwrap_or(Value::Nil)).collect()))
    }

    pub(crate) fn race(&mut self, block: &'p Block) -> R<'p> {
        let branches = &block.body.stmts;
        if branches.len() <= 1 {
            let scope = new_scope(Some(self.scope().clone()));
            self.push_frame(self.self_val(), scope)?;
            let result = branches.first().map_or(Ok(Value::Nil), |first| self.eval(first));
            self.pop_frame();
            return result;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel();
        for (i, branch) in branches.iter().enumerate() {
            let mut child = self.fork();
            child.cancel.push(stop.clone());
            child.frames = vec![Frame { self_val: self.self_val(), scope: new_scope(Some(self.scope().clone())) }];
            let sender = sender.clone();
            self.spawn_task(child, move |task| {
                let _ = sender.send((i, task.eval(branch)));
            });
        }
        drop(sender);
        let mut first_error: Option<(usize, Ctrl<'p>)> = None;
        for (i, result) in receiver {
            match result {
                Ok(v) => {
                    stop.store(true, AtomicOrdering::Relaxed);
                    return Ok(v);
                }
                Err(e) if e.error_type() == Some("Cancelled") => {}
                Err(e) => {
                    if first_error.as_ref().is_none_or(|(j, _)| i < *j) {
                        first_error = Some((i, e));
                    }
                }
            }
        }
        match first_error {
            Some((_, e)) => Err(e),
            None => self.check_cancel().map(|()| Value::Nil),
        }
    }
}
