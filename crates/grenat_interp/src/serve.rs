//! `grenat serve`: a program woken by its triggers — schedules run on
//! their own task, each webhook request on a task of its own.

use std::time::Duration;

use grenat_ast::Program;

use crate::builtins::{Every, RawRequest};
use crate::value::Locked;
use crate::{Interp, Options, RuntimeError, on_interpreter_thread, spawner};

/// Runs the script (which declares the triggers), then serves them until
/// the process ends. `listening` is told the webhook server's address.
pub fn serve(
    program: &Program,
    options: Options,
    address: &str,
    listening: impl FnOnce(&str) + Send,
) -> Result<(), RuntimeError> {
    on_interpreter_thread(|green| {
        let mut interp = Interp::new(program, options, spawner(green))?;
        if let Err(ctrl) = interp.run_script() {
            return Err(interp.runtime_error(ctrl));
        }
        let schedules = interp.schedules.borrow().len();
        let webhooks = !interp.webhooks.borrow().is_empty();
        if schedules == 0 && !webhooks {
            return Err(RuntimeError {
                ty: "ArgumentError".into(),
                message: "nothing to serve: declare `every … do … end` or `on_webhook \"/path\" do … end`".into(),
                span: None,
                trace: Vec::new(),
            });
        }
        for i in 0..schedules {
            interp.spawn_task(interp.fork(), move |task| task.run_schedule(i));
        }
        if webhooks {
            let server = grenat_serve::Server::bind(address)
                .map_err(|message| RuntimeError { ty: "IoError".into(), message, span: None, trace: Vec::new() })?;
            listening(&server.address());
            loop {
                let Ok(incoming) = grenat_green::blocking(|| server.next()) else { continue };
                interp.spawn_task(interp.fork(), move |task| {
                    let raw = RawRequest {
                        method: incoming.method.clone(),
                        path: incoming.path.clone(),
                        query: incoming.query.clone(),
                        headers: incoming.headers.clone(),
                        body: incoming.body.clone(),
                    };
                    let (status, body) = match task.handle_webhook(raw) {
                        Ok(answer) => answer,
                        Err(ctrl) => {
                            let error = task.runtime_error(ctrl);
                            task.write_err(&format!("[webhook] {}: {}: {}\n", incoming.path, error.ty, error.message));
                            (500, "error".into())
                        }
                    };
                    let json = body.starts_with('{') || body.starts_with('[');
                    incoming.respond(status, if json { "application/json" } else { "text/plain; charset=utf-8" }, body);
                });
            }
        }
        interp.wait_for_tasks();
        Ok(())
    })
}

impl Interp<'_> {
    /// Runs schedule `i` forever; a failed run is reported, not fatal.
    fn run_schedule(&mut self, i: usize) {
        loop {
            let (wait, block, label) = {
                let schedules = self.schedules.borrow();
                let schedule = &schedules[i];
                let wait = match &schedule.every {
                    Every::Seconds(s) => Duration::from_secs_f64(*s),
                    Every::Cron(cron) => {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                        let next = cron.next(now.as_secs() as i64).unwrap_or(i64::MAX / 2);
                        Duration::from_secs_f64((next as f64 - now.as_secs_f64()).max(0.0))
                    }
                };
                (wait, schedule.block.clone(), schedule.label.clone())
            };
            grenat_green::sleep(wait);
            if let Err(ctrl) = self.call_block(&block, Vec::new()) {
                let error = self.runtime_error(ctrl);
                self.write_err(&format!("[{label}] failed: {}: {}\n", error.ty, error.message));
            }
        }
    }
}
