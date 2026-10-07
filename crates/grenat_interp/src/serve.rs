//! `grenat serve`: a program woken by its triggers — schedules and
//! mailboxes (see `crate::mailboxes`) run on their own task, each webhook
//! request on a task of its own; a streamed
//! response is written by its block, on that task, as it runs.

use std::time::Duration;

use grenat_ast::Program;

use std::sync::{Arc, Mutex};

use crate::builtins::{EventSink, Every, HttpAnswer, RawRequest};
use crate::value::{Locked, Value};
use crate::{Interp, Options, RuntimeError, on_interpreter_thread, spawner};

/// The client's stream, kept to be ended once the block has run.
struct ClientStream(Arc<Mutex<Option<grenat_serve::EventStream>>>);

impl EventSink for ClientStream {
    fn send(&mut self, name: Option<&str>, data: &str) -> std::io::Result<()> {
        match self.0.borrow_mut().as_mut() {
            Some(events) => events.send(name, data),
            None => Err(std::io::ErrorKind::NotConnected.into()),
        }
    }
}

/// Job workers of an application with a database.
const WORKERS: usize = 2;

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
        let mailboxes = interp.mailboxes.borrow().len();
        let database = interp.app_db.borrow().is_some();
        let webhooks = !interp.webhooks.borrow().is_empty()
            || !interp.routes.borrow().is_empty()
            || !interp.exposures.borrow().is_empty();
        if schedules == 0 && mailboxes == 0 && !webhooks && !database {
            return Err(RuntimeError {
                ty: "ArgumentError".into(),
                message: "nothing to serve: declare routes (`get \"/\" do … end`), `expose`, `on_webhook`, `on_email` or `every`"
                    .into(),
                span: None,
                trace: Vec::new(),
            });
        }
        for i in 0..schedules {
            interp.spawn_task(interp.fork(), move |task| task.run_schedule(i));
        }
        for i in 0..mailboxes {
            interp.spawn_task(interp.fork(), move |task| task.watch_mailbox(i));
        }
        // with a database, workers run the queued jobs
        if database {
            for _ in 0..WORKERS {
                interp.spawn_task(interp.fork(), |task| task.work());
            }
        }
        if webhooks {
            let server = grenat_serve::Server::bind(address).map_err(|message| RuntimeError {
                ty: "IoError".into(),
                message,
                span: None,
                trace: Vec::new(),
            })?;
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
                    let mut answer = match task.handle_request(raw) {
                        Ok(answer) => answer,
                        Err(ctrl) => {
                            let error = task.runtime_error(ctrl);
                            task.write_err(&format!(
                                "[{} {}] {}: {}\n",
                                incoming.method, incoming.path, error.ty, error.message
                            ));
                            task.record_event("request", &format!("{} {}", incoming.method, incoming.path), &error);
                            HttpAnswer::text(500, "error")
                        }
                    };
                    if task.log {
                        task.write_err(&format!("[web] {} {} → {}\n", incoming.method, incoming.path, answer.status));
                    }
                    let label = format!("{} {}", incoming.method, incoming.path);
                    match answer.stream.take() {
                        Some(block) => {
                            // a client gone before the head: nothing to stream to
                            if let Ok(events) = incoming.respond_events(answer.status, &answer.headers) {
                                task.serve_events(&block, events, &label);
                            }
                        }
                        // written off the green workers: a client that does not read holds no one
                        None => grenat_green::blocking(move || {
                            incoming.respond(answer.status, &answer.content_type, &answer.headers, answer.body)
                        }),
                    }
                });
            }
        }
        interp.wait_for_tasks();
        Ok(())
    })
}

impl<'p> Interp<'p> {
    /// Streams a response: its block writes events to the client as they
    /// come. A failure once the head is sent ends the stream with an
    /// `error` event (its details stay in the log); a client gone is no
    /// failure.
    fn serve_events(&mut self, block: &Value<'p>, events: grenat_serve::EventStream, label: &str) {
        let sink = Arc::new(Mutex::new(Some(events)));
        let result = self.run_events(block, Box::new(ClientStream(sink.clone())));
        let mut events = sink.borrow_mut().take().expect("the stream, until now");
        match result.map_err(|ctrl| self.runtime_error(ctrl)) {
            Ok(()) => {}
            Err(error) if &*error.ty == "StreamClosed" => {
                if self.log {
                    self.write_err(&format!("[web] {label}: the client closed the stream\n"));
                }
                return;
            }
            Err(error) => {
                self.write_err(&format!("[{label}] {}: {}\n", error.ty, error.message));
                self.record_event("request", label, &error);
                let _ = events.send(Some("error"), "error");
            }
        }
        let _ = events.finish();
    }

    /// Runs schedule `i` forever; a failed run is reported, not fatal.
    fn run_schedule(&mut self, i: usize) {
        loop {
            let (wait, block, label) = {
                let schedules = self.schedules.borrow();
                let schedule = &schedules[i];
                let wait = match &schedule.every {
                    Every::Seconds(s) => Duration::from_secs_f64(*s),
                    Every::Cron(cron) => {
                        let now =
                            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
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
                self.record_event("schedule", &label, &error);
            }
        }
    }
}
