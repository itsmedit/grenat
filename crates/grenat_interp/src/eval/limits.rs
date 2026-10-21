//! Rate limits: `limit net("api.hubspot.com"), 10.per_second` declares how
//! often the process may call a host, `concurrent: 4` how many calls may be
//! in flight at once. One limiter per host, shared by every task and job of
//! the process (see `grenat_limits`).
//!
//! ```ruby
//! limit net("api.hubspot.com"), 10.per_second
//! limit net("api.github.com"), 5000.per_hour, concurrent: 4
//! limit net("api.anthropic.com"), 50.per_minute     # model calls too
//! ```
//!
//! A call over the limit waits for its turn instead of failing: a green
//! sleep, in slices, so that a cancelled task (`race`, `parallel_map`)
//! stops waiting at once and a `within budget(time: …)` block is stopped
//! when its time is up (waiting is time spent). The calls held are those
//! of `Http`, `Slack.post` and the Slack approvals (`slack.com`),
//! `Audio.url`, a `Mailer`'s `send` (its SMTP host), and every model call —
//! prompts, streams, embeddings, transcriptions — by its provider's host
//! (a mock's too: a test sees the limit the server would). A `batch_map`
//! batch is one request, not held.
//!
//! A `429` answered with `Retry-After` to a call of `Http` pauses the
//! limiter of the host that answered — a redirect's target, not the host
//! first asked — until then (an hour at most): no task calls it before.
//! A limit declared in a test ends with it; the others start each test
//! with a full bucket.

use std::time::Duration;

use grenat_limits::{Limiter, Permit, Rate};
use grenat_llm::ModelConfig;

use crate::prelude::*;

/// The record `10.per_second` makes.
pub(crate) const RATE: &str = "Rate";

/// The longest a slice of waiting lasts: cancellation is seen this soon.
const NAP: Duration = Duration::from_millis(20);

/// The longest a server's `Retry-After` pauses a host.
const MAX_PAUSE: Duration = Duration::from_secs(3600);

const USAGE: &str = "`limit net(\"api.github.com\"), 5000.per_hour, concurrent: 4`";

/// A host's limit, as declared.
pub(crate) struct DeclaredLimit {
    host: String,
    limiter: Arc<Limiter>,
    /// Declared by a running test: gone when it ends.
    in_test: bool,
}

/// `net("host")` as the first argument of `limit`: the host's expression.
pub(crate) fn net_host(args: &[grenat_ast::Arg]) -> Option<&Expr> {
    let Some(grenat_ast::Arg::Pos(first)) = args.first() else {
        return None;
    };
    match &first.kind {
        ExprKind::Call { recv: None, name, args, block: None, .. } if name.name == "net" => match args.as_slice() {
            [grenat_ast::Arg::Pos(host)] => Some(host),
            _ => None,
        },
        _ => None,
    }
}

/// `count.per_second` (and `per_minute`, `per_hour`, `per_day`, `per(d)`).
pub(crate) fn rate<'p>(count: i64, seconds: f64) -> R<'p> {
    let rate = Rate::new(count, seconds).or_else(|e| raise("ArgumentError", e))?;
    Ok(Value::record(
        RATE,
        vec![("count".into(), Value::Int(i64::from(rate.count))), ("period".into(), Value::Duration(seconds))],
    ))
}

/// The rate a `Rate` record holds.
fn rate_of(fields: &Fields<'_>) -> Option<Rate> {
    match (field(fields, "count"), field(fields, "period")) {
        (Some(Value::Int(count)), Some(Value::Duration(seconds))) => Rate::new(*count, *seconds).ok(),
        _ => None,
    }
}

/// A host as `uses net("…")` names it: no scheme, port, path, user or space.
fn valid_host(host: &str) -> bool {
    !host.is_empty() && !host.contains(['/', '@', '?', '#', ':']) && !host.chars().any(char::is_whitespace)
}

impl<'p> Interp<'p> {
    /// `limit net("api.hubspot.com"), 10.per_second, concurrent: 4`.
    pub(crate) fn declare_limit(&mut self, host: &'p Expr, rest: &'p [grenat_ast::Arg]) -> R<'p> {
        let host = self.eval(host)?;
        let args = self.eval_args(rest)?;
        if host.contains_taint() || args.pos.iter().chain(args.named.iter().map(|(_, v)| v)).any(Value::contains_taint)
        {
            return raise("TaintError", "an untrusted value reaches `limit` without validation");
        }
        let host = match &host {
            Value::Str(h) if valid_host(h) => h.to_ascii_lowercase(),
            Value::Str(h) => {
                return raise("ArgumentError", format!("`limit` expects a host (`api.github.com`), got {h:?}"));
            }
            Value::Secret(_) => return raise("SecretError", "a secret is never a host: `limit net(\"api.x.com\"), …`"),
            other => return raise("TypeError", format!("`limit` expects a host, got {}: {USAGE}", other.inspect())),
        };
        let rate = match args.pos.as_slice() {
            [] => None,
            [Value::Record(r)] if &*r.ty == RATE => rate_of(&r.fields),
            [other] => {
                let message = format!("`limit` expects a rate (`10.per_second`), got {}: {USAGE}", other.inspect());
                return raise("TypeError", message);
            }
            _ => return raise("ArgumentError", format!("`limit` takes a host, then a rate: {USAGE}")),
        };
        let mut concurrent = None;
        for (option, value) in &args.named {
            match (option.as_str(), value) {
                ("concurrent", Value::Int(n)) if *n >= 1 => concurrent = Some(*n as usize),
                (option, v) => {
                    let message = format!(
                        "invalid `limit` option `{option}: {}` (it takes `concurrent:` a positive count)",
                        v.inspect()
                    );
                    return raise("ArgumentError", message);
                }
            }
        }
        if rate.is_none() && concurrent.is_none() {
            return raise("ArgumentError", format!("`limit` expects a rate, `concurrent:`, or both: {USAGE}"));
        }
        let mut limits = self.limits.borrow_mut();
        if limits.iter().any(|l| l.host == host) {
            return raise("ArgumentError", format!("`net(\"{host}\")` is already limited: declare its limit once"));
        }
        if self.log {
            let rate = rate.map(|r| r.to_string());
            let at_once = concurrent.map(|n| format!("{n} at once"));
            let what: Vec<String> = rate.into_iter().chain(at_once).collect();
            self.write_err(&format!("[limit] net(\"{host}\"): {}\n", what.join(", ")));
        }
        let limiter = Arc::new(Limiter::new(rate, concurrent));
        limits.push(DeclaredLimit { host, limiter, in_test: self.in_test() });
        Ok(Value::Nil)
    }

    /// The limiter of `host`, if one was declared.
    fn limiter_for(&self, host: &str) -> Option<Arc<Limiter>> {
        let limits = self.limits.borrow();
        limits.iter().find(|l| l.host.eq_ignore_ascii_case(host)).map(|l| l.limiter.clone())
    }

    /// Waits for `host`'s turn: the permit to call it (`None` without a
    /// limit), its slot given back when dropped.
    pub(crate) fn throttle(&self, host: &str) -> Result<Option<Permit>, Ctrl<'p>> {
        let Some(limiter) = self.limiter_for(host) else {
            return Ok(None);
        };
        let permit = limiter.acquire(Instant::now, |left| self.nap(left))?;
        if self.log && !permit.waited.is_zero() {
            self.write_err(&format!("[limit] net(\"{host}\"): waited {:.2}s\n", permit.waited.as_secs_f64()));
        }
        Ok(Some(permit))
    }

    /// The permit for a call to `model`, by its provider's host.
    pub(crate) fn throttle_model(&self, model: &ModelConfig) -> Result<Option<Permit>, Ctrl<'p>> {
        if self.limits.borrow().is_empty() {
            return Ok(None);
        }
        match crate::llm::model_host(model) {
            Some(host) => self.throttle(&host),
            None => Ok(None),
        }
    }

    /// No call to `host` for `wait` (a server's `Retry-After`; an hour at most).
    pub(crate) fn pause_host(&self, host: &str, wait: Duration) {
        if let Some(limiter) = self.limiter_for(host) {
            limiter.pause_until(Instant::now() + wait.min(MAX_PAUSE));
        }
    }

    /// Waits `duration`: a green sleep, cancellable, within the budgets.
    pub(crate) fn wait_out(&self, duration: Duration) -> Result<(), Ctrl<'p>> {
        let deadline = Instant::now() + duration;
        while let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
            self.nap(left)?;
        }
        Ok(())
    }

    /// One slice of a wait: then a cancelled task, or a budget whose time
    /// is up, stops waiting.
    fn nap(&self, left: Duration) -> Result<(), Ctrl<'p>> {
        grenat_green::sleep(left.min(NAP));
        self.check_cancel()?;
        self.check_budgets()
    }

    /// After a test: its own limits gone, the others full again.
    pub(crate) fn end_test_limits(&self) {
        let mut limits = self.limits.borrow_mut();
        limits.retain(|l| !l.in_test);
        for limit in limits.iter() {
            limit.limiter.refill();
        }
    }
}
