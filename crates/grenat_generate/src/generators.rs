//! `grenat generate <kind> <name>`: a part of an application and its tests,
//! required from `src/app.grn`.

use std::path::Path;

use crate::app::{APP_FILE, with_require};
use crate::fields::Field;
use crate::names::Names;
use crate::templates::{self, fill};
use crate::write::{Change, Writer};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Agent,
    Workflow,
    Record,
    Tool,
    Eval,
}

impl Kind {
    pub const ALL: &str = "agent|workflow|record|tool|eval";

    pub fn parse(kind: &str) -> Option<Kind> {
        Some(match kind {
            "agent" => Kind::Agent,
            "workflow" => Kind::Workflow,
            "record" => Kind::Record,
            "tool" => Kind::Tool,
            "eval" => Kind::Eval,
            _ => return None,
        })
    }
}

/// A file to write: its path in the application, its text.
type File = (String, String);

/// Generates `kind` named `name` in the application at `root`; `fields` are a
/// record's (`subject:String`), `now` (seconds since the epoch) dates a
/// migration.
pub fn generate(root: &Path, kind: Kind, name: &str, fields: &[String], now: i64) -> Result<Vec<Change>, String> {
    let app = std::fs::read_to_string(root.join(APP_FILE))
        .map_err(|_| format!("{} is not an application: it has no {APP_FILE} (`grenat new --app <name>`)", root.display()))?;
    let names = Names::new(name)?;
    if kind != Kind::Record && !fields.is_empty() {
        return Err("only a record takes fields".into());
    }
    let (files, requires) = match kind {
        Kind::Agent => part("agents", &names, templates::AGENT, templates::AGENT_TEST),
        Kind::Workflow => part("workflows", &names, templates::WORKFLOW, templates::WORKFLOW_TEST),
        Kind::Tool => part("tools", &names, templates::TOOL, templates::TOOL_TEST),
        Kind::Record => record(&names, fields, now)?,
        Kind::Eval => {
            if !root.join(format!("src/agents/{}.grn", names.snake)).exists() {
                return Err(format!("an eval asks an agent: there is no src/agents/{}.grn (`grenat generate agent {}`)", names.snake, names.snake));
            }
            let files = vec![
                (format!("evals/{}_eval.grn", names.snake), fill(templates::EVAL, &names)),
                (format!("evals/{}.jsonl", names.snake), templates::DATASET.to_string()),
            ];
            (files, Vec::new())
        }
    };
    let mut writer = Writer::new(root);
    // all or nothing: every file is new
    for (path, _) in &files {
        writer.check_new(path)?;
    }
    for (path, text) in &files {
        writer.create(path, text)?;
    }
    if !requires.is_empty() {
        let app = requires.iter().fold(app, |app, target| with_require(&app, target));
        writer.update(APP_FILE, &app)?;
    }
    Ok(writer.changes)
}

/// A part in `src/<dir>/` with its test in `tests/<dir>/`.
fn part(dir: &str, names: &Names, code: &str, test: &str) -> (Vec<File>, Vec<String>) {
    let snake = &names.snake;
    let files = vec![
        (format!("src/{dir}/{snake}.grn"), fill(code, names)),
        (format!("tests/{dir}/{snake}_test.grn"), fill(test, names)),
    ];
    (files, vec![format!("./{dir}/{snake}")])
}

/// A record, the migration creating its table, and its test.
fn record(names: &Names, fields: &[String], now: i64) -> Result<(Vec<File>, Vec<String>), String> {
    let fields = fields.iter().map(|f| Field::parse(f)).collect::<Result<Vec<_>, _>>()?;
    if fields.is_empty() {
        return Err(format!("a record has fields: `grenat generate record {} subject:String`", names.snake));
    }
    let version = version(now);
    let migration = format!("{version}_create_{}", names.plural);
    let files = vec![
        (format!("src/records/{}.grn", names.snake), templates::record(names, &fields)),
        (format!("src/migrations/{migration}.grn"), templates::migration(names, &fields, &version)),
        (format!("tests/records/{}_test.grn", names.snake), templates::record_test(names, &fields)),
    ];
    Ok((files, vec![format!("./records/{}", names.snake), format!("./migrations/{migration}")]))
}

/// `YYYYMMDDHHMMSS`, UTC: migrations sort in the order they were made.
fn version(now: i64) -> String {
    let (year, month, day, hour, minute, _) = grenat_serve::calendar::civil(now);
    format!("{year:04}{month:02}{day:02}{hour:02}{minute:02}{:02}", now.rem_euclid(60))
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert_eq!(super::version(0), "19700101000000");
        assert_eq!(super::version(1_790_000_000), "20260921141320");
    }
}
