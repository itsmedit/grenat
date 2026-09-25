//! The files generators write: Grenat code, readable as it is, whose
//! `__snake__`, `__Camel__` and `__plural__` markers take the names given.

use crate::fields::Field;
use crate::names::Names;

/// A template with the names filled in.
pub(crate) fn fill(template: &str, names: &Names) -> String {
    template.replace("__snake__", &names.snake).replace("__Camel__", &names.camel).replace("__plural__", &names.plural)
}

// ── A new application ───────────────────────────────────────

pub(crate) const CONFIG: &str = "\
# The database: SQLite by default, PostgreSQL with `DATABASE_URL=postgres://…`.
# Tests get a new in-memory database each, with every migration applied.
database Env.get(\"DATABASE_URL\") || \"sqlite://db/development.db\"

model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
model :smart, provider: :anthropic, name: \"claude-opus-5\"
";

/// `grenat generate` adds its `require`s after the last one.
pub(crate) const APP: &str = "\
# The application: its parts, then what it serves (`grenat serve`).
require \"./config\"

get \"/health\" do |req|
  \"ok\"
end
";

pub(crate) const APP_TEST: &str = "\
require \"../src/app\"

test \"the application is up\" do
  assert_equal 200, request(:get, \"/health\")[\"status\"]
end
";

pub(crate) const README: &str = "\
# __snake__

A Grenat application.

```sh
grenat migrate            # applies the migrations to the database
grenat serve              # routes, exposed agents, triggers and job workers
grenat test               # offline: mocked models, a database per test
grenat eval evals/…       # quality, with the real models
```

Its parts, each with its tests (`grenat generate agent|workflow|record|tool|eval <name>`):

- `src/config.grn` — the database and the models;
- `src/app.grn` — requires the parts, then declares what is served;
- `src/agents/`, `src/workflows/`, `src/records/` (and `src/migrations/`), `src/tools/`;
- `tests/`, `evals/`.
";

// ── Parts ───────────────────────────────────────────────────

pub(crate) const AGENT: &str = "\
require \"../config\"

## __Camel__: say what this agent is for.
agent __Camel__
  model :fast
  instructions \"You are __Camel__. Answer briefly.\"
  max_turns 10

  ## Answers a request.
  on __Camel__Request(text: String) -> ~String
    run text
  end
end
";

pub(crate) const AGENT_TEST: &str = "\
require \"../../src/app\"

test \"__Camel__ answers a request\" do
  mock :fast, replies: [\"Done.\"]
  __snake__ = spawn __Camel__
  assert_equal \"Done.\", __snake__.ask(__Camel__Request(text: \"Hello\")).trust!
end
";

pub(crate) const WORKFLOW: &str = "\
require \"../config\"

prompt __snake___draft(id: Int) -> ~String using :fast
  user \"Write a draft for #{id}.\"
end

## __Camel__: a durable workflow. Each step is journaled: a run that stops
## resumes where it stopped. Queue it with `enqueue(:__snake__, id)`; in a job,
## the approval waits for `Approvals.approve`.
workflow __snake__(id: Int) -> ~String uses llm, human
  draft = step(:draft) { __snake___draft(id) }
  step(:review) { approve! \"Go on with the draft for #{id}?\" }
  draft
end
";

pub(crate) const WORKFLOW_TEST: &str = "\
require \"../../src/app\"

test \"__snake__ drafts, then asks a human\" do
  mock :fast, replies: [\"A draft\"]
  with_human(approve_all) do
    assert_equal \"A draft\", __snake__(1).trust!
  end
end

test \"__snake__ stops when the human says no\" do
  mock :fast, replies: [\"A draft\"]
  with_human(deny_all) do
    assert_raises(ApprovalDenied) { __snake__(1) }
  end
end
";

pub(crate) const TOOL: &str = "\
require \"../config\"

## __Camel__: say what this tool does — a model reads it to decide when to call it.
tool __snake__(query: String) -> String
  \"No result for #{query}\"
end
";

pub(crate) const TOOL_TEST: &str = "\
require \"../../src/app\"

test \"__snake__ answers\" do
  assert_equal \"No result for rust\", __snake__(\"rust\")
end
";

pub(crate) const EVAL: &str = "\
require \"../src/app\"

# `grenat eval evals/__snake___eval.grn` asks the real models: each row of
# `__snake__.jsonl` is scored from 0 to 1 by a judge.
eval \"__snake__\", dataset: \"__snake__.jsonl\", threshold: 0.8 do |row|
  answer = spawn(__Camel__).ask(__Camel__Request(text: row.input))
  judge(:smart, \"Does the answer match the expected one?\", answer, row.expected)
end
";

pub(crate) const DATASET: &str = "\
{\"input\": \"Hello\", \"expected\": \"A short, polite greeting\"}
";

// ── Records ─────────────────────────────────────────────────

pub(crate) fn record(names: &Names, fields: &[Field]) -> String {
    let declarations: String = fields.iter().map(|f| format!("  {}\n", f.declaration())).collect();
    fill(&format!("require \"../config\"\n\nstruct __Camel__\n  table :__plural__\n  id: Int?\n{declarations}end\n"), names)
}

pub(crate) fn migration(names: &Names, fields: &[Field], version: &str) -> String {
    let columns: Vec<String> = fields.iter().map(Field::column).collect();
    fill(
        &format!(
            "require \"../config\"\n\nmigration \"{version}_create___plural__\" do |db|\n  db.migrate(\"CREATE TABLE __plural__ (id #{{db.primary_key}}, {})\")\nend\n",
            columns.join(", ")
        ),
        names,
    )
}

pub(crate) fn record_test(names: &Names, fields: &[Field]) -> String {
    let values: Vec<String> = fields.iter().map(|f| format!("{}: {}", f.name, f.sample())).collect();
    let first = &fields[0].name;
    fill(
        &format!(
            "require \"../../src/app\"\n\ntest \"a __Camel__ is saved, then found\" do\n  saved = __Camel__.create({})\n  assert_equal 1, __Camel__.count\n  assert_equal saved.{first}, __Camel__.find(saved.id)&.{first}\nend\n",
            values.join(", ")
        ),
        names,
    )
}
