//! Model calls (`grenat_calls`): what each one cost, and on whose behalf —
//! an agent, a workflow, a job — summed up by the console.

use grenat_db::{Cell, Connection};

use crate::Result;
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
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
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
}

pub fn ensure(db: &mut dyn Connection) -> Result<()> {
    let key = db.dialect().primary_key();
    db.batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TABLE} (id {key}, at FLOAT NOT NULL, model TEXT NOT NULL, agent TEXT, \
         workflow TEXT, job_id INTEGER, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, \
         cost_usd FLOAT NOT NULL)"
    ))
}

pub fn record(db: &mut dyn Connection, call: &Call) -> Result<()> {
    ensure(db)?;
    let sql = format!(
        "INSERT INTO {TABLE} (at, model, agent, workflow, job_id, input_tokens, output_tokens, cost_usd) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
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
    ];
    db.execute(&sql, &params).map(drop)
}

/// The calls made since `since`, oldest first.
pub fn since(db: &mut dyn Connection, since: f64) -> Result<Vec<Call>> {
    ensure(db)?;
    let sql = format!(
        "SELECT at, model, agent, workflow, job_id, input_tokens, output_tokens, cost_usd FROM {TABLE} \
         WHERE at >= ? ORDER BY at, id"
    );
    let rows = db.query(&sql, &[Cell::Float(since)])?;
    Ok(rows
        .iter()
        .map(|r| Call {
            at: float(r, 0),
            model: text(r, 1),
            agent: opt_text(r, 2),
            workflow: opt_text(r, 3),
            job_id: opt_int(r, 4),
            input_tokens: int(r, 5),
            output_tokens: int(r, 6),
            cost_usd: float(r, 7),
        })
        .collect())
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
