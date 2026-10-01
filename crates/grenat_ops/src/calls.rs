//! Model calls (`grenat_calls`): what each one cost, and on whose behalf —
//! an agent, a workflow, a job — and how much of its input the prompt cache
//! served, summed up by the console.

use grenat_db::{Cell, Connection};

use crate::Result;
use crate::columns;
use crate::row::{float, int, nullable, opt_int, opt_text, text};

pub const TABLE: &str = "grenat_calls";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Call {
    pub at: f64,
    /// The model that answered.
    pub model: String,
    /// The agent whose handler made the call.
    pub agent: Option<String>,
    /// The workflow running.
    pub workflow: Option<String>,
    /// The job running.
    pub job_id: Option<i64>,
    /// Every input token, cached or not.
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    /// Of the input, the tokens read from the prompt cache.
    pub cached_tokens: i64,
    /// Of the input, the tokens written to the prompt cache.
    pub cache_write_tokens: i64,
}

/// What costs are summed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Agent,
    Workflow,
    Model,
    /// The UTC day, `YYYY-MM-DD`.
    Day,
}

/// The calls of one agent (workflow, model, day), summed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Total {
    /// The agent's name (…), or `None` for calls made outside any.
    pub key: Option<String>,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub cached_tokens: i64,
    pub cache_write_tokens: i64,
}

impl Total {
    /// The share of the input the cache served, from 0 to 1.
    pub fn cached_share(&self) -> f64 {
        share(self.cached_tokens, self.input_tokens)
    }
}

/// The share of `calls`' input the cache served, from 0 to 1.
pub fn cached_share(calls: &[Call]) -> f64 {
    share(calls.iter().map(|c| c.cached_tokens).sum(), calls.iter().map(|c| c.input_tokens).sum())
}

fn share(cached: i64, input: i64) -> f64 {
    if input > 0 { cached as f64 / input as f64 } else { 0.0 }
}

/// Columns added since the table's first release.
const ADDED: [(&str, &str); 2] =
    [("cached_tokens", "INTEGER NOT NULL DEFAULT 0"), ("cache_write_tokens", "INTEGER NOT NULL DEFAULT 0")];

pub fn ensure(db: &mut dyn Connection) -> Result<()> {
    let key = db.dialect().primary_key();
    db.batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, at FLOAT NOT NULL, model TEXT NOT NULL, agent TEXT, \
         workflow TEXT, job_id INTEGER, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, \
         cost_usd FLOAT NOT NULL, cached_tokens INTEGER NOT NULL DEFAULT 0, \
         cache_write_tokens INTEGER NOT NULL DEFAULT 0)"
    ))?;
    columns::add_missing(db, TABLE, &ADDED)
}

pub fn record(db: &mut dyn Connection, call: &Call) -> Result<()> {
    ensure(db)?;
    let sql = format!(
        "INSERT INTO {TABLE} (at, model, agent, workflow, job_id, input_tokens, output_tokens, cost_usd, \
         cached_tokens, cache_write_tokens) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    );
    let params = [
        Cell::Float(call.at),
        Cell::Text(call.model.clone()),
        nullable(call.agent.as_deref()),
        nullable(call.workflow.as_deref()),
        call.job_id.map_or(Cell::Null, Cell::Int),
        Cell::Int(call.input_tokens),
        Cell::Int(call.output_tokens),
        Cell::Float(call.cost_usd),
        Cell::Int(call.cached_tokens),
        Cell::Int(call.cache_write_tokens),
    ];
    db.execute(&sql, &params).map(drop)
}

const COLUMNS: &str =
    "at, model, agent, workflow, job_id, input_tokens, output_tokens, cost_usd, cached_tokens, cache_write_tokens";

/// The calls made since `since`, oldest first.
pub fn since(db: &mut dyn Connection, since: f64) -> Result<Vec<Call>> {
    ensure(db)?;
    let rows =
        db.query(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE at >= ? ORDER BY at, id"), &[Cell::Float(since)])?;
    Ok(rows.iter().map(call).collect())
}

/// The calls job `job` made, oldest first.
pub fn of_job(db: &mut dyn Connection, job: i64) -> Result<Vec<Call>> {
    ensure(db)?;
    let rows =
        db.query(&format!("SELECT {COLUMNS} FROM {TABLE} WHERE job_id = ? ORDER BY at, id"), &[Cell::Int(job)])?;
    Ok(rows.iter().map(call).collect())
}

fn call(r: &grenat_db::Row) -> Call {
    Call {
        at: float(r, 0),
        model: text(r, 1),
        agent: opt_text(r, 2),
        workflow: opt_text(r, 3),
        job_id: opt_int(r, 4),
        input_tokens: int(r, 5),
        output_tokens: int(r, 6),
        cost_usd: float(r, 7),
        cached_tokens: int(r, 8),
        cache_write_tokens: int(r, 9),
    }
}

/// `calls` summed `by` agent (workflow, model, day), costliest first — by
/// day, in date order.
pub fn totals(calls: &[Call], by: By) -> Vec<Total> {
    let mut totals: Vec<Total> = Vec::new();
    for call in calls {
        let key = match by {
            By::Agent => call.agent.clone(),
            By::Workflow => call.workflow.clone(),
            By::Model => Some(call.model.clone()),
            By::Day => Some(day(call.at)),
        };
        let total = match totals.iter().position(|t| t.key == key) {
            Some(i) => &mut totals[i],
            None => {
                totals.push(Total { key, ..Total::default() });
                totals.last_mut().expect("just pushed")
            }
        };
        total.calls += 1;
        total.input_tokens += call.input_tokens;
        total.output_tokens += call.output_tokens;
        total.cost_usd += call.cost_usd;
        total.cached_tokens += call.cached_tokens;
        total.cache_write_tokens += call.cache_write_tokens;
    }
    match by {
        By::Day => totals.sort_by(|a, b| a.key.cmp(&b.key)),
        _ => totals.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd)),
    }
    totals
}

/// `YYYY-MM-DD` (UTC) of `t` seconds since the epoch.
pub fn day(t: f64) -> String {
    let days = (t / 86_400.0).floor() as i64;
    // Howard Hinnant's civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
