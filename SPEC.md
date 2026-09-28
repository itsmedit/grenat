# Grenat — specification v0.1 (draft)

> *Ruby's syntax, Rust's speed, agents as first-class citizens.*

Name: **Grenat** (French for "garnet", a red gemstone and a cousin of the ruby). Extension: `.grn`. CLI: `grenat`.

---

## 1. Philosophy

1. **It reads like Ruby**: `def … end`, `do |x|` blocks, `@ivars`, symbols, `"#{}"` interpolation, `unless`, trailing `if`, implicit return.
2. **It runs like Rust**: static typing, **inferred** (you almost never write types inside function bodies), native compilation (Cranelift for development, LLVM for release), no GC: **Perceus reference counting** (as in Koka and Roc), with in-place reuse.
3. **Agents are actors**: an agent is an isolated lightweight process with a mailbox, supervised, as in Erlang.
4. **The LLM is an effect**: the compiler knows which functions call an LLM, touch the network or the disk, or ask a human for approval.
5. **Everything that comes out of an LLM is suspect**: it is *tainted* data, `~T`, that cannot reach a dangerous tool without validation. Prompt injection is defended against **at compile time**.
6. **No async coloring**: no `async`/`await`. Everything is concurrent by default (M:N green threads), as in Go or Erlang.

### What we drop from Ruby (the price of speed)

| Ruby | Grenat |
|---|---|
| Dynamic typing | Hindley-Milner inference + annotations at the boundaries (`def`, `struct`) |
| `method_missing`, `send`, `eval`, `instance_eval` | ❌ replaced by compile-time **macros** (§2, *Macros*) |
| Monkey-patching, open classes | ❌ extension only through `module` + `include` (static traits) |
| `nil` everywhere | Explicit optional types `String?`; `&.` and `||` are kept |
| Exceptions | Errors are values (`Result`), with `raise`/`rescue` and `?` as sugar |
| GC | Deterministic reference counting, zero pauses |

Crystal already showed that Ruby syntax with static types and LLVM delivers compiled-language performance. Grenat follows that path and adds an agentic runtime and an effect system.

---

## 2. The basics

```ruby
# Inference: no types inside the body
def fib(n: Int) -> Int
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end

names = ["Ada", "Linus", "Matz"]            # inferred Array(String)
names.map { |n| n.upcase }.each { |n| puts n }

# Optionals
def find_user(id: Int) -> User?
  users.find { |u| u.id == id }
end

email = find_user(42)&.email || "unknown"
```

### Structs (values), classes (references), modules (traits)

```ruby
struct Point
  x: Float
  y: Float

  def norm = Math.sqrt(x * x + y * y)      # one-line method
end

class Counter                              # reference, RC-counted
  @count: Int = 0
  def incr! = @count += 1
end

module Describable
  abstract def describe -> String          # abstract method
  def shout = describe.upcase              # default method
end

struct Invoice
  include Describable
  amount: Money
  def describe = "Invoice for #{amount}"
end
```

### Algebraic enums and pattern matching

```ruby
enum Shape
  Circle(radius: Float)
  Rect(w: Float, h: Float)
end

def area(s: Shape) -> Float
  case s
  in Circle(r)  then 3.14159 * r * r
  in Rect(w, h) then w * h
  end                                      # exhaustiveness checked by the compiler
end
```

### Errors

```ruby
def load_config(path: Path) -> Result(Config, IoError) uses fs.read
  text = File.read(path)?                  # ? propagates the error
  Config.parse(text)?
end

begin
  cfg = load_config("app.toml")?
rescue IoError => e
  warn "missing config: #{e.message}"
  cfg = Config.default
end
```

### Files and packages

A program is a file and every file it `require`s. Requires are static — a literal path, at the top level — and resolved before anything runs; each file is loaded once, after the files it requires, and all of them share one namespace, as in Ruby.

```ruby
require "./helpers"        # helpers.grn, next to this file
require "../shared/text"   # ../shared/text.grn
require "http"             # the dependency `http`: its src/lib.grn
require "http/client"      # its src/client.grn
```

A package is a directory with a `grenat.toml` (`grenat new <name>` creates one, with `src/main.grn`, `src/lib.grn` and `tests/`):

```toml
[package]
name = "support"
version = "0.1.0"
# main = "src/main.grn"    what `grenat run` runs (the default)
# lib  = "src/lib.grn"     what `require "support"` loads (the default)

[dependencies]
utils = { path = "../utils" }
http = { git = "https://github.com/grenat-lang/http", tag = "v0.2.0" }   # or `branch`, `rev`
```

Git dependencies are fetched into the root package's `.grenat/deps/` and their exact commit recorded in `grenat.lock`, so that the program builds the same everywhere; `grenat update` moves them to the latest commit of their branch or tag. In a package, `grenat run`, `test`, `check` and `build` need no file: they take the package's program, or every file of `src/` and `tests/`.

### Macros

What Ruby does at run time with `attr_accessor`, `define_method` or `method_missing`, Grenat does at compile time. A macro is a template of declarations, invoked as a statement at the top level or in the body of a type, and replaced by its expansion before the program is checked — the generated code is typed and checked like the rest.

```ruby
## A reader and a writer for an instance variable.
macro property(name, type)
  def {{name}} -> {{type}} = @{{name}}

  def set_{{name}}(value: {{type}})
    @{{name}} = value
  end
end

macro statuses(*names)
  {% for s in names %}
  def {{s}}?(status: String) -> Bool = status == "{{s}}"
  {% end %}
end

class Invoice
  @customer: String = ""
  property :customer, String
end

statuses :draft, :sent, :paid
```

In a template, `{{name}}` is an argument — a symbol gives its name (`:customer` → `customer`), anything else the text written (`2 * 21`, `"EUR"`, `String`) — and `{% for x in list %}` … `{% end %}` repeats over a `*variadic` parameter (`{{list}}` alone joins it with `, `). Arguments can be named (`property name: :customer, type: String`). An expansion may invoke other macros, including in the types it generates; nesting stops after 32 levels. Macros are not hygienic: the generated code is what the template says. Errors in expanded code — a template that does not parse, a type error — are reported at the invocation.

---

## 3. Effects and capabilities

Every function has a set of **effects**. They are **inferred** inside a module and **declared** on public functions, tools and `main`.

| Effect | Meaning |
|---|---|
| `llm` | calls a model (spends budget) |
| `net` / `net("api.github.com")` | network, optionally restricted to a host |
| `fs.read(path)` / `fs.write(path)` | disk, restricted to a path prefix |
| `shell` | external process, **always run in a WASM sandbox** |
| `human` | waits for a human answer (approval, input) |
| `time`, `random` | non-determinism (matters for durable workflows) |

```ruby
def main uses llm, net, fs.read("./docs"), human
  # main is the capability root: no function can do
  # more than what main grants it.
end
```

The compiler **rejects** a call whose effect is not covered by the caller. At run time, path and host restrictions are checked a second time (defense in depth).

---

## 4. Models and typed prompts

```ruby
model :fast,  provider: :anthropic, name: "claude-haiku-4-5",  temperature: 0.2
model :smart, provider: :anthropic, name: "claude-opus-5"
model :local, provider: :ollama,    name: "llama3.3"
```

A **`prompt` function** is an ordinary function whose implementation is delegated to an LLM. Its return type becomes a **JSON Schema generated at compile time**. The output is parsed, validated, and **automatically retried** when it is malformed.

```ruby
enum Sentiment
  Positive
  Neutral
  Negative
end

struct Summary
  title: String              ## Short title, 8 words at most
  bullets: Array(String)     ## 3 to 5 key points
  sentiment: Sentiment
end

## Summarizes a news article.
prompt summarize(article: String) -> ~Summary using :fast
  system "You are a concise, factual analyst."
  user <<~P
    Summarize this article:
    #{article}
  P
end
```

`##` comments are sent to the model: they become the descriptions of the schema and of the tools. The code's documentation is also the prompt.

### The tainted type `~T`

`~Summary` means: *the structure is valid, but the content comes from an LLM*.

- You can **read** a `~T` freely (print it, log it, pass it to another prompt).
- You **cannot** pass it to a function carrying the `shell`, `fs.write`, `net` or `human` effect. The compiler forbids it.
- To "clean" it, you must do so explicitly:

```ruby
s = summarize(article)

s.check { |x| x.bullets.size.between?(3, 5) }   # -> Result(Summary, CheckError)
s.approve(by: :human)                            # -> Summary (human effect)
s.trust!                                         # -> Summary (grep-able, flagged by the linter)
```

---

## 5. Tools

```ruby
## Reads a text file from the working directory.
tool read_file(path: Path) -> String uses fs.read("./workspace")
  File.read(path)
end

## Runs a command in a sandbox (no network).
tool run(cmd: String) -> Output uses shell
  Sandbox.exec(cmd, timeout: 30.s, net: false)
end

## Sends an email. Requires human approval.
tool send_email(to: Email, subject: String, body: String) -> Unit uses net("smtp.mail.com"), human
  approve! "Send \"#{subject}\" to #{to}?"
  Smtp.send(to:, subject:, body:)
end
```

The arguments an LLM passes to a tool arrive **tainted**. A `tool` is the only place where Grenat accepts converting them to `T`, after schema validation and a capability check. It is the trust boundary.

---

## 6. Agents (actors)

```ruby
agent Researcher
  model :smart
  tools read_url, search_web, save_note
  budget tokens: 200_000, usd: 2.00, time: 10.min
  max_turns 30

  instructions <<~I
    You are a rigorous researcher. Always cite your sources.
  I

  @notes: Array(Note) = []                 # private state, never shared

  on Research(topic: String) -> ~Report
    run "Thorough investigation of: #{topic}"
  end

  on AddNote(note: Note)
    @notes << note
  end
end
```

- `run` is the **built-in agent loop** (LLM → tools → LLM …). It stops when the model produces the handler's return type (here `Report`), or when the budget or `max_turns` runs out.
- `@…` state is **isolated**: no other agent can reach it. Messages are **moved** or **frozen** (`Sendable`), so there are no data races by construction.

```ruby
r = spawn Researcher

report = r.ask(Research(topic: "nuclear fusion 2026"))    # waits for the answer
r.tell(AddNote(note: Note.new("to double-check")))          # fire and forget

# Concurrency without async/await
reports = topics.parallel_map(limit: 5) { |t| r.ask(Research(topic: t)) }

winner = race do
  a.ask(Solve(problem))
  b.ask(Solve(problem))
end                                        # the first one wins, the other is cancelled
```

### Nested budgets

```ruby
within budget(usd: 1.00, time: 2.min) do
  reports = topics.parallel_map { |t| r.ask(Research(topic: t)) }
rescue BudgetExceeded => e
  warn "stopped at #{e.spent}"
end
```

An inner budget can never exceed its enclosing budget. The runtime **accounts for every token** and cuts in-flight calls as soon as the limit is reached.

### Supervision

```ruby
supervisor SupportTeam, strategy: :one_for_one, max_restarts: 3, within: 1.min
  child Triage
  child Researcher, count: 4               # pool of 4, load-balanced
  child Writer
end
```

---

## 7. Durable workflows

A `workflow` survives crashes, redeployments and human waits lasting several days. Every `step` is **journaled** (an append-only file today; SQLite and Postgres later). On restart, completed steps are **replayed from the journal**: no LLM call is ever billed twice.

```ruby
workflow publish_article(topic: String) -> Url
  report   = step(:research) { researcher.ask(Research(topic:)) }
  draft    = step(:draft)    { writer.ask(Draft(report:)) }
  approved = step(:review)   { draft.approve(by: :human, timeout: 3.days) }
  step(:publish) { Blog.publish(approved) }
end
```

**Rule enforced by the compiler**: inside a `workflow`, every non-deterministic effect (`llm`, `net`, `time`, `random`, `human`) must be **inside a `step`**. Code outside steps can therefore be replayed identically.

---

## 8. Tests and evals

```ruby
test "summarize respects the format" do
  cassette "summaries/article_1" do        # records, then replays (VCR-style)
    s = summarize(fixture("article_1.txt")).trust!
    assert s.bullets.size.between?(3, 5)
  end
end

test "the agent refuses to send without approval" do
  mock :smart, replies: [call(:send_email, to: "x@y.z", subject: "hi", body: "…")]
  with_human(deny_all) do
    assert_raises ApprovalDenied { spawn(Mailer).ask(Handle(ticket)) }
  end
end

eval "summary quality", dataset: "evals/articles.jsonl", threshold: 0.85 do |row|
  s = summarize(row.input)
  judge(:smart, "Is the summary faithful?", s, row.input)   # LLM as a judge
end
```

`grenat test` is deterministic (cassettes and mocks). `grenat eval` calls the real models and produces a score and cost report.

---

## 9. Compiler architecture (Rust)

```
grenat/
├── crates/
│   ├── grenat_lexer      # hand-written (modes: interpolation, heredocs) — tokens + comments
│   ├── grenat_ast        # typed syntax tree, spans
│   ├── grenat_parser     # recursive descent + Pratt, error recovery → AST
│   ├── grenat_hir        # name resolution, desugaring (blocks, &., ?, on/tool/prompt)
│   ├── grenat_types      # gradual checking: names, types, effects, ~T taint
│   ├── grenat_mir        # SSA IR, Perceus RC insertion, monomorphization
│   ├── grenat_codegen    # Cranelift JIT today (numbers, strings, arrays, structs); AOT and LLVM later
│   ├── grenat_runtime    # reference-counted objects called by native code (today);
│   │                     # later a staticlib linked into every binary:
│   │                     #   M:N work-stealing scheduler, actors, supervision,
│   │                     #   LLM clients (Anthropic, OpenAI, Ollama), budgets,
│   │                     #   durable journal (SQLite), wasmtime sandbox
│   ├── grenat_interp     # HIR interpreter (phase 1, to validate the semantics)
│   ├── grenat_driver     # load, check and run a program: shared by the CLI and built executables
│   ├── grenat_host       # static library linked into executables (runtime + interpreter + main)
│   ├── grenat_standalone # static library linked into `--native` executables (runtime + main)
│   ├── grenat_report     # diagnostic rendering, shared by the CLI and executables
│   └── grenat_cli        # grenat run | build | test | eval | fmt
└── std/                  # standard library written in Grenat
```

Error messages: every diagnostic has a stable code, the offending line and, for taint, **the place where the LLM produced the value**. Actual output of `grenat check` when the agent's unvalidated answer is sent in `support_desk.grn`:

```
error[E0412]: an untrusted value reaches `send_reply` (effect `net`) without validation
   --> support_desk.grn:163:29
    |
163 |     send_reply(ticket.from, answer.body)
    |                             ^^^^^^^^^^^
note: untrusted from here (a model's answer or a network response)
   --> support_desk.grn:132:5
    |
132 |     run <<~T
    |     ^^^^^^^^
  = help: validate it with `.check { … }`, `.approve(by: :human)` or `.trust!`
```

| Code | Family |
|---|---|
| E0100 | unknown name (variable, function, type, constant, model, tool), with a suggestion |
| E0200 | type, arity, named argument, unknown field or method |
| E0300 | effect used but not declared; `main` and `tool`s must declare theirs |
| E0412 | tainted `~T` value reaching a dangerous effect without validation |
| E0413 | `prompt`, or handler using `run`, whose return type is not tainted |
| E0500 | invalid declaration (unknown effect, non-serializable LLM output, `run` outside an agent…) |

---

## 10. Distribution

Goal: `brew install grenat` on macOS, `yay -S grenat` on Arch / Omarchy, with no runtime dependency other than the system linker.

| Channel | Command | When |
|---|---|---|
| Homebrew tap (`itsmedit/homebrew-grenat`) ✅ | `brew install itsmedit/grenat/grenat` | from v0.1 |
| `.deb` and `.rpm` of each release ✅ | `sudo apt install ./grenat_<v>_amd64.deb`, `sudo dnf install <url>.rpm` | from v0.1.1 |
| Docker image (GHCR) ✅ | `docker run ghcr.io/itsmedit/grenat` | from v0.1.1 |
| homebrew-core | `brew install grenat` | once the project is "notable" (~75 stars), stable release, built from source |
| AUR `grenat` (source) and `grenat-bin` (prebuilt) | `yay -S grenat` | from v0.1 |
| Arch `extra` repository | `pacman -S grenat` | once an Arch packager adopts it |
| Shell installer ✅ | `curl -sSL https://github.com/itsmedit/grenat/releases/latest/download/install.sh \| sh` | from v0.1.1 |

**Automation** (`.github/workflows/release.yml`), on every `v*` tag: the macOS arm64/x86_64 and Linux x86_64/aarch64 binaries — the Linux ones built on Debian 11 (glibc 2.31), so that they run on Ubuntu 20.04+, Debian 11+, Amazon Linux 2023, RHEL 9 and Fedora — each packaged with the runtime libraries and checked as installed (`scripts/release-package.sh`); `.deb` and `.rpm` packages (`packaging/nfpm.yaml`); a GitHub Release with them, their checksums and the install script (`scripts/install.sh`); then the Docker image, built from that release (`packaging/Dockerfile`), pushed to `ghcr.io/itsmedit/grenat`. The tap's formula is written from the release by `scripts/homebrew-formula.sh`. The `grenat-bin` `PKGBUILD` will point to the same artifacts.

**Resulting design constraints**:

| Constraint | Decision |
|---|---|
| Grenat is a compiler that links a runtime | `libgrenat_host.a` (the runtime and the interpreter, linked into every executable `grenat build` produces) and `std/` are looked up **relative to the executable** (`<prefix>/bin/grenat` → `<prefix>/lib/grenat/`, `<prefix>/share/grenat/std/`), overridable with `GRENAT_HOME`. Works under `/opt/homebrew`, `/usr` and `~/.cargo`; in a Cargo build, next to `target/*/grenat` |
| Linker | the system `cc` (Xcode CLT on macOS, `gcc` on Arch), the only runtime dependency. Verified on macOS arm64 and on Linux (Debian, `scripts/test-linux.sh`): arm64 (the whole suite passes) and x86_64 under emulation (the JIT, both kinds of executables; only timing assertions suffer from the emulated CPU). On Linux the debug information of Rust's standard library is stripped at link time (`--strip-debug`), or it would make a native program 2.5 MB instead of 0.5 MB |
| No system dependencies | `rustls` (no OpenSSL), bundled SQLite (`rusqlite`, `bundled` feature) |
| LLVM is heavy (~100 MB) | **Cranelift by default**, embedded and pure Rust. LLVM as an optional feature |
| License | MIT OR Apache-2.0 from the first commit |

Installed layout:

```
<prefix>/bin/grenat
<prefix>/lib/grenat/libgrenat_host.a
<prefix>/share/grenat/std/…
```

---

## 11. Roadmap

| Phase | Content | Outcome |
|---|---|---|
| **0** ✅ | Lexer + parser + AST + `grenat check/parse/tokens` | every example and every code block of this spec parses |
| **0.5** ✅ | `grenat fmt` (comment-preserving) | canonical formatter |
| **1** ✅ | Interpreter, `prompt`, `tool`, agents, budgets, taint, Anthropic client, `grenat run/test` | the first agent runs |
| **2** ✅ | Names, types, effects and `~T` taint checked **before execution**; capabilities enforced at run time | security errors before execution |
| **3** ✅ | Concurrent actor agents, real `parallel_map`/`race`, cancellation, deadlock detection, supervision | multi-agent |
| **4** ✅ | Cranelift codegen: 4a JIT for numeric functions, 4b strings/arrays/structs with Perceus RC, 4c `grenat build`, 4d M:N green threads, 4e programs without the interpreter | fast native binaries |
| **5** ✅ | Durable workflows (`step` journal), cassettes, `mock`, `eval` | production-ready |
| **6** ✅ | LSP, LLVM release builds, macros, package manager (and programs of several files) | ecosystem |
| **7** ✅ | What real agents need, measured by ten use cases (`examples/usecases`): an I/O library with effects (`Http` client and server, `Db`, email), MCP client, multimodal prompts and the Batch API, conversations and long-term memory, a sandbox for `shell` and per-tool timeouts, triggers (`every`, webhooks) | agents in production |
| **8** ✅ | Agent applications, in the language and its toolchain (no framework on top): an HTTP server with routes, records and migrations on `Db`, jobs and triggers, a persisted approval queue (a workflow waits days for a human), agents served over HTTP and as MCP servers; `grenat new --app`, `grenat generate agent\|workflow\|record\|tool\|eval`, `grenat serve` | applications of agents |
| **9** ✅ | `grenat console`: the operations console of an application, derived from the program and its runtime — approvals inbox, runs and their journals (replay, resume), costs per agent, evals over time, taint and capability refusals, MCP servers. It observes and operates; code stays the source of truth | agents operated from a browser |
| **10** ✅ | Configuration without surprise: encrypted credentials per environment (`grenat credentials edit`), `Secret` values that never reach a model, and `config/models.yml` — the models of ten providers (Anthropic, OpenAI, Gemini, Mistral, xAI, OpenRouter, Groq, DeepSeek, Together, Ollama), each reached by the right connector with its key found in the credentials or the environment | an application configured, not coded |

### Phases 8 and 9: applications, in Grenat itself

What a web framework would add on top of a language, Grenat takes in, because the runtime already holds what an agent application is made of: budgets, approvals, workflow journals, evals, capabilities. Three layers, kept apart:

1. **The language and its runtime** stay small and stable; nothing below changes their semantics.
2. **The standard library** grows the batteries, each an effect: `Web` (an HTTP server: routes, requests, responses), `Record` (typed records and migrations over `Db`), `Jobs` (queued and scheduled work, webhooks), an approval queue in the database instead of the terminal.
3. **The toolchain** carries the conventions — `grenat new --app` (a layout: `agents/`, `workflows/`, `records/`, `web/`, `tests/`, `evals/`), `grenat generate` (code and its tests, never hidden configuration), `grenat serve` (the server, the triggers and the workers of an application), `grenat console` — in crates of their own, in the one `grenat` binary.

A program that wants none of it pays nothing: conventions live in generators, not in the language. Interoperability comes first: an application serves its agents over HTTP and as MCP servers, so that programs in other languages use them, and it uses agents written elsewhere through `Http` and `mcp`.

### Phase 8 status: routes (`Web`)

```ruby
get "/tickets/:id" do |req|
  id = req.params["id"].check { |i| i.to_i > 0 }?.to_i
  json(find_ticket(id))
end

post "/tickets" do |req|
  status 201, json(create_ticket(req.json.trust!["subject"]))
end

get "/hello" do |req|
  html "<h1>Hello #{Html.escape(req.params["name"])}</h1>"
end
```

`get`, `post`, `put`, `patch` and `delete` declare routes (`:name` segments are parameters), served by `grenat serve` with the webhooks and schedules. A handler receives a `Request` — `method`, `path`, `params` (the path's and the query's, decoded), `query`, `headers`, `body`, `json` — and its value is the response: a string (text), a hash or record (JSON), `json(v)`, `html(page)`, `status(code, response)`, `redirect(url)`, an integer (a status), `nil` (204). An unknown path is a 404, a known one with another method a 405.

- **Taint.** Everything a request carries but its method and path is untrusted. A page is a sink: `html` refuses an untrusted value that was not escaped — `Html.escape` is the check that makes it safe — so a page cannot carry a script someone slipped in; `redirect` refuses an untrusted URL (no open redirects). Statically (E0412) and at run time.
- **Tests.** `request :get, "/tickets/42"` (or `json:`, `body:`, `headers:`) goes through the routes without a server and returns `{"status" => …, "body" => …, "content_type" => …, "headers" => …}`.

### Phase 8 status: records and migrations

```ruby
database Env.fetch("DATABASE_URL")

struct Ticket
  table :tickets
  id: Int?
  subject: String
  status: String = "open"
end

migration "001_create_tickets" do |db|
  db.migrate("CREATE TABLE tickets (id #{db.primary_key}, subject TEXT NOT NULL, status TEXT NOT NULL)")
end

t = Ticket.create(subject: "Bug")
Ticket.find(t.id)                     # Ticket? — also Ticket.where(status: "open"), Ticket.all, Ticket.count
t.with(status: "closed").save
t.delete
```

`database` declares the application's database; a struct with `table :name` is a **record**: its fields are the table's columns, `id` its primary key, given by the database (`INSERT … RETURNING`, on SQLite and PostgreSQL alike). `Ticket.all`, `where` (equalities), `find` and `count` read; `create`, `save` (an insert without an id, an update with it) and `delete` write — typed for the checker (`find` is a `Ticket?`, `where` an `Array(Ticket)`). Identifiers are quoted, values always parameters. `migration "name" do |db| … end` declares migrations; `grenat migrate` applies those the database has not seen, in order, each in a transaction, and records them (`grenat_migrations`).

- **Effects and taint.** Reads are `db.read`, writes `db.write`; a record written with an untrusted value is refused (E0412, `TaintError`) — check it first — while an untrusted value may filter a read.
- **Tests.** In `grenat test`, each test gets a new in-memory SQLite database with every migration applied: no test sees another's data, none touches the real database. Migrations are therefore written in SQL that SQLite and PostgreSQL both accept; where they differ, `db.primary_key` is the column type of an id the database gives (`INTEGER PRIMARY KEY`, `BIGSERIAL PRIMARY KEY`), and `db.dialect` is `:sqlite` or `:postgres`.
- **Limits.** Fields are integers, floats, strings and booleans (optional or not); no associations yet.

### Phase 8 status: jobs

```ruby
post "/tickets" do |req|
  ticket = Ticket.create(subject: req.json["subject"].check { |s| s.size < 200 }?)
  enqueue(:triage, ticket.id)            # or enqueue(:digest, in: 1.hour)
  status 202, json(ticket)
end

workflow triage(id: Int) uses llm, db    # a workflow: durable, its steps journaled
  step(:classify) { classify(Ticket.find(id).subject) }
end
```

`enqueue(:function, args…)` queues a call in the application's database (`grenat_jobs`: the arguments in the exact encoding of workflow journals); `grenat serve` runs workers that claim ready jobs with a conditional update — two workers, or two servers, never run the same job — and mark them done, or retry them 1, 2, 4… minutes later, up to three runs, then mark them failed with their error. A job's arguments are written: nothing untrusted goes in them (E0412). In `grenat test`, jobs wait in the test's database: `Jobs.enqueued` lists them, `Jobs.perform` runs them (and those they queue), `Jobs.failed` gives the ones given up.

### Phase 8 status: approvals that wait

```ruby
workflow publish(id: Int) uses llm, db, human
  draft = step(:draft) { write_post(id) }
  step(:review) { approve! "Publish “#{draft.title}”?" }    # the job waits, for days if need be
  step(:publish) { Blog.publish(draft) }
end

get "/approvals" do |req|
  json(Approvals.pending)
end

post "/approvals/:id" do |req|
  Approvals.approve(req.params["id"].check { |i| i.to_i > 0 }?.to_i)
  204
end
```

In a job, a question to a human (`approve!`, `.approve(by: :human)`) no longer waits at a terminal: it is stored (`grenat_approvals`), and the job waits — status `waiting`, neither failed nor retried — until someone decides with `Approvals.approve(id)` or `Approvals.deny(id)` (a `human` effect), from a route today, from `grenat console` in phase 9. The decision queues the job again: it runs from the start, a workflow replays its journaled steps without running them — the model is not called again — and finds the answer where it stopped. A question is known by its job and its rank among the questions of a run, so the same question gets the same answer on every run; a denial fails the job at once (`ApprovalDenied`), without retries. `Approvals.pending` lists what waits. Outside jobs, and in tests under `with_human`, questions are answered as before.

### Phase 8 status: agents served to other programs (`expose`)

```ruby
## Finds a ticket by its number.
tool find_ticket(id: Int) -> String uses db.read
  Ticket.find(id)&.subject || "no such ticket"
end

agent Triage
  model :fast
  ## Classifies a ticket.
  on Classify(subject: String) -> ~String
    run "Classify: #{subject}"
  end
end

expose "/mcp", tools: [:find_ticket], agents: [Triage], token: Env.fetch("API_TOKEN"), name: "support"
```

`expose` serves tools and agents to programs in any language, next to the routes of `grenat serve`:

- `POST /mcp` is an MCP server (streamable HTTP, stateless: a JSON response per message) — `initialize`, `ping`, `tools/list`, `tools/call` — so Claude, an IDE or another Grenat program (`mcp :support, url: …`) uses them as tools;
- `POST /mcp/find_ticket` with `{"id": 42}` answers `{"result": …}` (422 with `{"error": …}` when the tool raises, 400 for arguments that are not a JSON object, 404 for an unknown tool);
- `GET /mcp` lists the tools and their input schemas.

A tool keeps its name, its `##` description and the schema of its parameters; it is announced read-only when its effects change nothing (`llm`, `db.read`, `fs.read`, `env`, `time`). Each handler of an exposed agent is a tool too (`triage_classify`), asked of one instance of the agent — one message at a time, as always. What a tool raises is a tool error for the caller, not a failure of the server.

- **Who may call.** An exposure spends money: it requires `Authorization: Bearer <token>` (compared in constant time; 401 otherwise), unless it says `public: true` — one of the two must be written.
- **Taint.** Arguments are checked against the schema. A tool is the trust boundary, as when a model calls it; an agent's message arrives untrusted, as a model's answer would, so a handler cannot put it in a page or a command unchecked (checked at run time).
- **Tests.** `request :post, "/mcp", json: {…}, headers: {…}` speaks to an exposure without a server.

### Phase 12 status: native facets (Rust)

A facet can ship Rust code that Grenat programs call as ordinary functions, as a Ruby gem ships C: a crate (a `cdylib`) depending on `grenat_ext`, the SDK, named by `[native]` in the facet's `grenat.toml` (`path = "native"`, the default).

```rust
use grenat_ext::{GrenatType, export};
use serde::{Deserialize, Serialize};

/// A cell of a sheet.
#[derive(Serialize, Deserialize, GrenatType)]
pub struct Cell { pub row: i64, pub text: String }

/// Reads a sheet: a line per row, cells separated by commas.
#[export(effects = "fs.read", error = "SheetError")]
pub fn read_sheet(path: String) -> Result<Vec<Vec<String>>, String> { … }

/// Adds two integers.
#[export(pure)]
pub fn add(a: i64, b: i64) -> i64 { a + b }
```

```ruby
# the application's Facetfile: its Rust code runs outside Grenat's sandbox, so it is trusted explicitly
facet "sheets", "~> 0.1", native: true
```

```ruby
# .grenat/native/native.grn in the installed facet, written by `setter install` from the library's manifest
## A cell of a sheet.
struct Cell
  row: Int
  text: String
end

## Reads a sheet: a line per row, cells separated by commas.
native def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read

## Adds two integers.
native def add(a: Int, b: Int) -> Int pure

# the application calls them as any function
def main uses fs.read
  rows = read_sheet("sales.csv").trust!
  puts add(rows.size, 1)
end
```

- **The facet's side.** `#[grenat_ext::export]` on a plain Rust function — `#[export(effects = "fs.read, net")]`, `#[export(pure)]`, `#[export(error = "SheetError")]` — whose parameters and result are `String`, integers (`Int`), `f64` (`Float`), `bool`, `Vec<T>` (`Array(T)`), `Option<T>` (`T?`), `HashMap<String, T>` (`Hash(String, T)`) or structs deriving serde's `Serialize`/`Deserialize` and `GrenatType` (their fields and `///` comments declared too). A function returning `Result<T, E: Display>` raises its `Err` as a Grenat error (`NativeError`, or the type `error:` names). The macro refuses at compile time what cannot cross: references, generics, `async`, methods, a `pure` function with effects.
- **A stable ABI.** No Rust type crosses the boundary, whose layout changes with the compiler: each function gets a C entry point `grenat_ext_v1_<name>` that takes its arguments as a JSON array (bytes the caller owns) and writes its result — or an error — as JSON into a buffer the library owns and frees (`grenat_ext_free`). A panic is caught in the library and never unwinds into Grenat. `grenat_ext_abi_version` is asked before anything else: a library built for another version of the ABI is refused, with the advice to rebuild it. `grenat_ext_manifest` describes the library — the ABI version, each function (name, documentation, parameters and result as Grenat types, effects, purity, error type) and the structs they use.
- **Installing.** `setter install` builds each trusted native facet (`cargo build --release`: a Rust toolchain is needed), or takes the library the facet ships for this platform (`<crate>/prebuilt/<arch>-<os>/lib<facet>.so|.dylib`: prebuilt binaries drop in there), copies it into the installed facet (`.grenat/native/<arch>-<os>/`), loads it once to read its manifest (`manifest.json`), and writes the declarations it stands for (`native.grn`, checked to parse). A program that requires the facet loads them first: the checker, the language server and the interpreter see ordinary declarations. A native facet's own tests see its functions once `setter install` has run in its directory.
- **`native def`.** A function implemented natively, declared without a body, at the top level: `native def name(params) -> T uses effects`, or `… -> T pure`. The checker takes the declaration at its word: parameters and result must be types that cross (E0500); the result of a function that is not `pure` comes from outside Grenat and is declared untrusted, `~T`, and a `pure` one's is not (E0413); a `pure` function has no effects (E0500); a wrong argument is an E0200, and the declared effects are the callers', which must cover them (E0300).
- **Trust.** Native code escapes Grenat's sandbox — no capability checked inside it, no taint tracked — so the application says which facets may ship some, in its own `Facetfile`: `native: true`. Without it, `setter install` refuses before building anything (building runs the crate's code) and loading a program that requires the facet refuses too, each saying why and how to trust it. A facet that another facet requires is trusted by the application the same way. A package's own native part is its author's, trusted.
- **Calls.** The library is loaded once, at the first call of one of its functions, and never unloaded. The arguments are encoded as JSON, the result decoded into the declared type (another type is a `NativeError`). The function's effects are checked against the capabilities of its callers before it runs, by name — a restriction such as `fs.read("./docs")` cannot bound what Rust code does. Its result is untrusted, unless it is `pure`, whose result is as trusted as its arguments; an untrusted argument cannot reach a native function with a dangerous effect (E0412, `TaintError`). **No secret is handed to native code** (E0414, `SecretError`), with no way around it for now: a secret serves where Grenat's own connectors reveal it. An `Err` raises the facet's error type; a panic, a `NativeError` (`` `explode` panicked: on fire (at src/lib.rs:98) ``); `--log` shows each call (`[native] sheets: read_sheet`).
- **Limits.** `grenat build` refuses a program that calls native code, and says so: the facets' libraries are not linked into executables yet. Enums, callbacks into Grenat and asynchronous functions do not cross.

### Phase 12 status: bridge facets (Ruby, Python)

A facet can also ship functions written in Ruby or Python. **No Ruby or Python is embedded in Grenat**: the facet ships a server, a separate process that Grenat starts and speaks JSON-RPC 2.0 to on its standard input and output. The facet names it in its `grenat.toml`:

```toml
[bridge]
command = ["ruby", "bridge/server.rb"]   # or ["python3", "bridge/server.py"]: run in the facet's directory
env = ["TEXTS_API_URL"]                  # variables passed on from Grenat's environment, if set
timeout = 30                             # seconds per call (the default)
```

The server uses a helper library Grenat ships (`bridges/` in the repository), standard library only — no gem, no pip package: Grenat writes it into the installed facet and puts it on the server's load path (`RUBYLIB`, `PYTHONPATH`).

```rb
# bridge/server.rb
require "grenat/bridge"

Grenat::Bridge.struct(:Word, fields: {text: :string, position: :int}, doc: "A word of a text.")

Grenat::Bridge.export(:words, params: {text: :string}, returns: ["Word"], pure: true, doc: "The words of a text.") do |text:|
  text.split.each_with_index.map { |word, i| {text: word, position: i} }
end

Grenat::Bridge.export(:read_text, params: {path: :string}, returns: :string, effects: ["fs.read"], error: "TextError") do |path:|
  File.read(path)
end

Grenat::Bridge.run
```

```python
# bridge/server.py
from typing import List, Optional
from grenat_bridge import BridgeError, export, run

@export(pure=True)
def mean(values: List[float]) -> Optional[float]:
    """The mean of the values, if there are some."""
    return sum(values) / len(values) if values else None

@export(error="MathError")
def ratio(a: float, b: float) -> float:
    return a / b

run()
```

```ruby
# the application's Facetfile: its code runs outside Grenat's sandbox, so it is trusted explicitly
facet "texts", "~> 0.1", bridge: true
```

```ruby
# .grenat/bridge/bridge.grn in the installed facet, written by `setter install` from the server's `describe`
## A word of a text.
struct Word
  text: String
  position: Int
end

## The words of a text.
native def words(text: String) -> Array(Word) pure

native def read_text(path: String) -> ~String uses fs.read

# the application calls them as any function
def main uses fs.read
  puts words("a b").size, read_text("notes.txt").trust!
end
```

- **The facet's side.** Ruby: `Grenat::Bridge.export(:name, params: {html: :string}, returns: [:string], effects: [], pure: true, doc: "…", error: "HtmlError") { |html:| … }`, the block taking the parameters as keywords, then `Grenat::Bridge.run`. Types are `:string`, `:int`, `:float`, `:bool`, `:nil`, `[t]` (`Array(T)`), `{string: t}` (`Hash(String, T)`), or written as Grenat writes them (`"Word"`, `"String?"`); `Grenat::Bridge.struct(:Word, fields: {…})` declares a struct, which crosses as a hash. Python: `@export` (or `@export(params=…, returns=…, effects=…, pure=…, doc=…, error=…)`), the types coming from the function's annotations — `str`, `int`, `float`, `bool`, `None`, `List[T]`, `Dict[str, T]`, `Optional[T]`, or a Grenat type as a string — and its docstring; `struct("Point", fields={…})`; then `run()`. What cannot cross is refused when the server starts: a name that is not a Grenat name, an unknown type, a hash without string keys, a `pure` function with effects.
- **The protocol.** One JSON-RPC 2.0 message per line. `describe` answers the manifest of a native library — its `abi` is the protocol's version (1), then the functions (name, documentation, parameters and result as Grenat types, effects, purity, error type) and the structs — so that a bridge's declarations are a native library's: `native def`, generated by the same code. `call` (`{"name": "words", "args": ["a b"]}`, the arguments in order) answers `{"result": …}`, or an error: `{"code": -32000, "message": "…", "data": {"type": "TextError"}}` for an exception (the function's `error:` type, or the one a `Grenat::Bridge::Error` / `BridgeError` names), the standard codes for a request that is not JSON (-32700), not a request (-32600), an unknown method (-32601), an unknown function or a wrong number of arguments (-32602). While the server runs, what the functions print goes to standard error: standard output carries the protocol only.
- **Installing.** `setter install` writes the helper libraries into each trusted bridge facet (`.grenat/bridge/lib/`), starts its server once, without the network, to `describe` it, and writes the manifest (`manifest.json`) and the declarations (`bridge.grn`, checked to parse); a server that cannot start or describe itself fails the install, with its last lines of standard error. A program that requires the facet loads the declarations first, as a native facet's; a bridge facet's own tests see its functions once `setter install` has run in its directory. A facet ships `[native]` or `[bridge]`, not both.
- **Processes.** One server per facet, started at the first call of one of its functions and kept alive for the next ones; standard input closed at the end of the program, it ends. **Calls are serialized**: one request at a time per server (concurrent tasks wait their turn), each with its own id. A call has the facet's `timeout`: a server that does not answer in time is killed and the call raises a `BridgeError` (`` `nap` still running after 30s: the process of facet `texts` was killed ``); the next call starts a new one. A server found dead is started again; one that dies while it handles a call is started again and the call sent anew once if the function is `pure` — a call with effects is never run twice — else the call raises a `BridgeError` with its exit status and last lines of standard error. `--log` shows each call (`[bridge] texts: words`) and every line the server writes on standard error (`[bridge] texts: …`).
- **Sandbox.** The server runs as `Shell.run`'s sandboxed programs do (the same code, `grenat_sandbox`): an argument vector in the facet's directory, a clean environment (`PATH`, `HOME`, `LANG`, the helpers' load path and the variables the facet declares in `env`), and **no network unless one of its functions declares a `net` effect** (`sandbox-exec` on macOS, a network namespace with `unshare` on Linux). Where the host refuses the sandbox (user namespaces disabled, a container's seccomp profile), the server runs without it and `--log` says so: like native code, a bridge is trusted code, and the sandbox is a second line of defense, not the first.
- **Trust and taint.** As native code: the application trusts each bridge facet in its `Facetfile` (`bridge: true`; `native: true` does not trust a bridge), else `setter install` refuses before starting anything and loading a program that requires it refuses too. Types and effects are checked from the declarations (E0200, E0300, E0500); a result is untrusted (`~T`) unless the function is `pure` (E0413); an untrusted argument cannot reach a function with a dangerous effect (E0412); **no secret is handed to a bridge** (E0414, `SecretError`). An exception raises the error type the server names; a result of another type than declared, a death or a timeout, a `BridgeError`.
- **Limits.** `grenat build` refuses a program that calls a bridge, as it refuses native code. Callbacks into Grenat, streaming and concurrent calls to one server are not supported.

### Phase 11 status: proxies for Http

`Http.get(url, proxy: "socks5://user:pass@127.0.0.1:1080")` sends a request through a proxy, with every `Http` method: `socks5://` (the host is resolved here), `socks5h://` (resolved by the proxy), `socks4://`, `socks4a://`, and HTTP proxies (`http://`, `https://`, which tunnel with `CONNECT`); credentials go in the URL. Without `proxy:` (or with `proxy: nil`), the environment's proxy is used as curl chooses it — `no_proxy` exempts hosts and their subdomains, then `https_proxy` for an https URL or `http_proxy` for an http one (never `HTTP_PROXY` in capitals, which a CGI request header can set), then `all_proxy` — and `proxy: false` goes direct whatever the environment says. An invalid proxy URL is an `ArgumentError` (an invalid one in the environment, an `HttpError` naming the variable, not its value); an unreachable proxy or a refused password is an `HttpError`. A proxy URL may be a secret (`proxy: Credentials.fetch(:proxy, :url)`): it is revealed to the transport only, and neither an error nor the `--log` line (`[http] GET … via [secret] → 200`) names it; a plain proxy URL is logged without its credentials.

### Phase 11 status: SSH and SFTP

```ruby
def deploy(server: SshSession, release: String) -> Bool uses ssh("api.acme.com"), fs.read("dist")
  archive = "/srv/releases/#{release}.tar.gz"
  server.upload("dist/app.tar.gz", archive)
  unpacked = server.run(["tar", "-xzf", archive, "-C", "/srv/app"])
  return false unless unpacked.ok?
  res = server.run(["systemctl", "restart", "shop"])
  warn res.stderr.trust! unless res.ok?
  res.ok?
end

def main uses ssh("api.acme.com"), fs.read("dist"), env
  server = Ssh.connect("deploy@api.acme.com", key: Credentials.fetch(:deploy, :ssh_key))
  puts deploy(server, "2026.09.26")
  logs = server.sftp.list("/var/log/shop").trust!.select { |e| !e.dir? && e.size > 0 }
  puts logs.map { |e| e.name }.join(", ")
  server.close
end

test "a deployment restarts the shop" do
  mock_ssh "deploy@api.acme.com", commands: {
    "tar -xzf *" => "",
    "systemctl restart shop" => {stderr: "", status: 0},
  }
  sftp = Ssh.connect("deploy@api.acme.com", key: Credentials.fetch(:deploy, :ssh_key)).sftp
  sftp.write("/srv/app/VERSION", "2026.09.26")
  assert_equal "2026.09.26", sftp.read("/srv/app/VERSION").trust!
end
```

`Ssh.connect("user@host", …)` opens an SSH connection — `key:` (the text of a private key, OpenSSH format; `passphrase:` if it is encrypted) or `password:`, and `port:` (22; or `user@host:2222`), `proxy:` (`socks5://[user:password@]host:port`), `timeout:` (30 s to connect) — and returns an `SshSession`: `user`, `host`, `port`. `server.run(["systemctl", "restart", "shop"])` runs a command and gives an `SshResult` — `status` (`-1` when a signal ended it), `signal`, `ok?`, `stdout`, `stderr`; a command that fails is an answer, not an error. `server.upload(local, remote)` and `server.download(remote, local)` copy a file; `server.sftp` is an `Sftp`: `list(dir)` (entries `SftpEntry`: `name`, `size`, `dir?`, `modified` in seconds since the epoch), `read`, `write(path, text)`, `upload`, `download`, `remove` (a file or an empty directory), `mkdir`, `rename`, `exists?`. `server.close` ends the connection. Connections are kept by the runtime (the records hold a number) and shared by the tasks that use them, one call at a time; every call blocks its own task only. The layer (`grenat_ssh`, pure Rust) knows nothing of the language.

- **Commands are argument vectors.** SSH carries one command line, which the server's shell splits again: each argument is quoted, so that `; rm -rf /`, `$(id)` or a quote arrive as one argument, never as shell syntax.
- **Capabilities.** Reaching a server is an `ssh` effect restricted by host: `uses ssh("api.acme.com")`. The checker takes the host of a literal target (E0300); the runtime checks it when connecting and at every call, on the connection's own host — a connection opened elsewhere is no way around a function's `uses`. A transfer also reads (`upload`: `fs.read`) or writes (`download`: `fs.write`) its local file.
- **Taint, both ways.** Nothing untrusted reaches a server — the target, a command's arguments, a path, a file's content (E0412, `TaintError`); what comes back is untrusted: `stdout`, `stderr`, `signal` (the server names it), `read`, `list`. `status`, `ok?` and `exists?` are not; nor is the message of an `SftpError`, which never repeats the server's own words.
- **Secrets.** The key, its passphrase, the password and the proxy URL may be secrets (`Credentials.fetch`): they are revealed to the connection only, and go nowhere else — not in a command, a path or a file's content (E0414, `SecretError`), and never in an error, a `--log` line (`[ssh] deploy@api.acme.com:22: connected (ssh-ed25519 SHA256:…) via [secret]`), a journal or the console.
- **Host keys.** The server's key is verified before any credential is sent: it must be recorded in `~/.ssh/known_hosts` (hashed entries and `@revoked` markers read; certificate authorities are not supported), or in the file `known_hosts:` names, or have the fingerprint `fingerprint: "SHA256:…"` gives. Nothing is accepted silently: an unknown key is a `HostKeyError` showing the fingerprint offered and how to trust it once checked out of band (`fingerprint:` or `known_hosts:`); a changed or revoked key is refused.
- **Errors.** `SshError` (the connection, a command that cannot start, the proxy, a closed connection — also one lost while a command ran, whose outcome is then unknown), `HostKeyError`, `SshAuthError` (credentials refused, a key that cannot be read), `SftpError` (each names the path and what was attempted), `TimeoutError`.
- **Tests.** `grenat test` reaches no server: `mock_ssh "deploy@api.acme.com", commands: {"systemctl restart shop" => "done"}, files: {"/srv/x" => "content"}` stands for one for the rest of the test (mocking it again replaces it, for the connections already open too) — a command's answer is its output, or `{stdout:, stderr:, status:}`, and a trailing `*` matches a prefix of the command line; the files are kept in memory, where `sftp` and transfers find them. A command or a server not mocked is an `SshError`. The interpreter's own tests also run against a real SSH and SFTP server in process (`grenat_ssh`'s `fake` feature).

### Phase 10 status: models from any provider (`config/models.yml`)

```yaml
fast:                        # the first model is the default one
  provider: anthropic
  name: claude-haiku-4-5
smart:
  provider: openai
  name: gpt-5
local:
  provider: ollama           # local: no key
  name: llama3.3
```

An application's models are configured, not coded: `config/models.yml` stands for `model :fast, provider: :anthropic, name: "…"` declarations — checked the same way, a mistake reported in that file — and code uses them by name (`using :fast`, `model :smart`). Declaring a model in code still works; declaring one twice is an error.

A provider's name is enough. Grenat knows how to reach each one — Anthropic through its Messages API, OpenAI through its Responses API (its reasoning models take tools only there), Gemini (Google's compatible endpoint), Mistral, xAI, OpenRouter, Groq, DeepSeek, Together and Ollama through Chat Completions — and where its key is: `<provider>.api_key` in the credentials, else its variable (`OPENAI_API_KEY`, `GEMINI_API_KEY`…); a missing key is said plainly. Options: `temperature`, `max_tokens`, `effort` (reasoning effort), `base_url` (a proxy, another machine), `price`.

- **Structured answers.** Where the provider follows a JSON Schema exactly (Anthropic, OpenAI, Gemini, Mistral, xAI, OpenRouter), answers and tool calls are strict; elsewhere (Groq, DeepSeek, Together, Ollama) the model answers in JSON mode with the schema in its instructions, and Grenat validates what comes back, as always.
- **Documents.** PDFs go to the providers that read them (Anthropic, OpenAI); others refuse them clearly. Images go everywhere.
- **Verified live** against OpenAI (September 2026, `gpt-5.4-mini` with `effort: low`, `gpt-4.1-mini` with `temperature`): strict structured answers, text, an agent calling a tool, an image and a PDF — through a key in the credentials and `config/models.yml` only. The other providers are verified against their documented formats.
- **Cost.** Grenat knows Anthropic's prices; for another model, `price: {input: 1.25, output: 10}` (dollars per million tokens) lets budgets and the console count it — without it, Grenat says once that budgets in dollars do not. `batch_map` is half price where the provider's batch API is used (Anthropic); elsewhere its calls run one by one, at full price.

### Phase 10 status: credentials and secrets

```ruby
def main uses net, env
  token = Credentials.fetch(:github, :token)        # a Secret
  Http.get("https://api.github.com/user", headers: {"Authorization" => "Bearer #{token}"})
  puts token                                        # [secret]
end
```

As with Rails: `grenat credentials edit` opens the application's secrets — YAML, encrypted with AES-256-GCM in `config/credentials.yml.enc` — in `$EDITOR`, and encrypts them again on save; the key is `config/master.key` (created `0600`, kept out of git) or `GRENAT_MASTER_KEY`. An environment may have its own (`--env production`: `config/credentials/production.yml.enc` and its own key), used when `GRENAT_ENV` names it. `grenat new --app` creates them.

`Credentials.fetch(:github, :token)` (`dig` gives `nil` when missing) is a `Secret`, not a `String`:

- it serves where it is meant to — HTTP URLs, headers, query and body, `Db.connect`, `database`, `Mail.connect`, `mcp` (URL, headers, command, environment), `Ssh.connect` (key, passphrase, password, proxy), `Shell` environments, `on_webhook` secrets, `expose` tokens — and `"Bearer #{token}"` or `"…" + token` are secrets too;
- anywhere else it reads `[secret]`: `puts`, `p`, logs, JSON, pages, the console; an HTTP error does not name a URL holding one;
- it never reaches a model: in `user`, `system`, `run`, `judge`, `Conversation#say`, a tool's answer (the model gets an error instead), or as a tool's parameter — refused by the checker (E0414) and at run time (`SecretError`);
- it is never journaled, queued as a job argument, nor written to a database;
- compared to a string (`req.headers["x-token"] == secret`), in constant time; it has no other method (`to_s` keeps it a secret). A function that takes one says so: `def headers(token: Secret)`.

In tests, `mock_credentials({"github" => {"token" => "t"}})` gives a test its credentials; without credentials (no file, or no key — a CI needs none), `Credentials.fetch(:github, :token)` is the stand-in secret `test-github-token`, so that declarations reading credentials (`on_webhook …, token: Credentials.fetch(…)`) load in tests too.

### Phase 9 status: `grenat console`

```sh
grenat serve                                  # the application: routes, jobs, schedules
grenat console                                # its console, on http://127.0.0.1:4000
GRENAT_CONSOLE_TOKEN=… grenat console --listen 0.0.0.0:4000   # elsewhere: a token
```

The operations console of an application, in a browser, open source like the rest. It observes and operates; the code stays the source of truth:

- **Overview** — approvals waiting, failed jobs, what was spent in 24 hours and 7 days, refusals, the last score of each eval, and what the program declares (agents, workflows, tools, routes, MCP servers).
- **Approvals** — the questions jobs wait on (`approve!` in a job), approved or denied here: the job is queued again and resumes where it stopped.
- **Jobs** — the queue by status; a job's arguments, last error, journal, approvals and model calls; a failed job retried (a workflow resumes from its journal).
- **Journals** — each run of a workflow, its steps and their values, completed or not.
- **Costs** — model calls by agent, workflow, model and day, over 24 hours, 7 or 30 days.
- **Evals** — each `grenat eval` kept: scores over time against their threshold, and what they cost.
- **Events** — what failed while serving (a request, a schedule, a job run, an exposed tool), and among them the refusals: untrusted data stopped at a sink, an effect not granted, a human's no, a budget spent.
- **MCP** — the servers the program uses (their tools, listed on demand; never their headers or environment) and what it serves (`expose`).

What the console shows, the runtime records in the application's database, when it has one (`grenat_ops`): every model call with the agent, workflow and job it was made for; what fails under `grenat serve`; each eval run. Recording never fails a program. A model call made in a `db.transaction` that is rolled back leaves the ledger with it.

- **Who may use it.** Listening on this machine (the default), the console answers only requests addressed to this machine, so that no web page reaches it by renaming a domain to 127.0.0.1. Anywhere else it needs a token (16 characters at least, from `GRENAT_CONSOLE_TOKEN` rather than the command line), then knows the browser by a session cookie (`HttpOnly`, `SameSite=Strict`); behind a proxy, serve it over HTTPS. Every form carries a secret of the process, a form from another origin is refused, pages run no script (a strict `Content-Security-Policy`) and cannot be framed.
- **Where it runs.** Next to the application — the same directory, the same `DATABASE_URL` — since workflow journals are files there. It runs the program's declarations (its database, MCP servers, routes, exposures), never its workers or schedules.

### Phase 8 status: generators

```sh
grenat new --app desk
cd desk
grenat generate agent triage                 # src/agents/triage.grn, tests/agents/triage_test.grn
grenat generate workflow onboard             # a durable workflow, with an approval
grenat generate record ticket subject:String priority:Int "score:Float?"
grenat generate tool lookup
grenat generate eval triage                  # evals/triage_eval.grn and its dataset
grenat migrate && grenat test && grenat serve
```

`grenat new --app` lays out an application: `src/config.grn` (the database — SQLite by default, `DATABASE_URL` otherwise — and the models), `src/app.grn` (which requires the parts, then declares what is served), `tests/`, `db/`. `grenat generate` (or `g`) adds a part and its tests, and requires it from `src/app.grn`, after the last `require`: an agent answering a request, a workflow whose second step waits for a human, a tool, an eval asking an agent and scored by a judge, or a record — its fields (`String`, `Int`, `Float`, `Bool`, optional with `?`), the migration creating its table (in `db/migrations/`, named after the time, in SQL that SQLite and PostgreSQL both accept), and a test that saves one and finds it. What is generated is code, read and changed like the rest — no hidden configuration — and it passes `grenat check`, `grenat test` and `grenat fmt --check` as it comes. A generator never overwrites a file: when one exists, nothing is written.

`grenat test` gives each test its own database and its own workflow journals, so that no test resumes a workflow another one ran.

### Phase 7 plan: the ten use cases

Ten realistic programs, one per kind of agent, are in `examples/usecases` (the first is `examples/support_desk.grn`): support, code review, research, data, documents, a scheduled digest, operations, a chat with memory, a team of agents, third-party tools. Each checks and passes its tests today, the model mocked, in 24 to 47 lines of logic (113 for the full support desk). Only two run for real: the others fake, between `STUBS` markers, the I/O the standard library lacks. Phase 7 is done when every stub is gone. In order:

1. ✅ **`Http` client** with effects (`net("host")` enforced at run time, JSON, headers, timeouts) — unblocks cases 2, 3, 6, 7, 10.
2. ✅ **`Db`** (SQLite and Postgres, parameterized queries, `db.read` / `db.write` effects) — case 4.
3. ✅ **Sandboxed `shell`** (a process with a timeout, no network unless declared) and per-tool timeouts — cases 2 and 7.
4. ✅ **MCP client** (`tools mcp(:server)`, capabilities granted per server, results tainted) — case 10.
5. ✅ **Multimodal prompts** (PDF, images) and the **Batch API** — case 5.
6. ✅ **Email** and **triggers** (`every`, cron schedules, webhooks) — cases 1, 2, 6.
7. ✅ **Facets**: libraries shared like Ruby's gems. A *facet* (a garnet's face) is a package; a program lists the facets it uses in its `Facetfile`, pinned in `Facetfile.lock`; the `setter` tool (who sets stones in a jewel) creates, adds, installs, updates and publishes them, with versions (`facet "http", "~> 0.3"`) resolved from git tags through an index repository. It replaces the `[dependencies]` of `grenat.toml`.
8. ✅ Smaller gaps met while writing them: conversations as a type (`Conversation`: history, compaction into a summary, `save`/`load`) for case 8; top-level constants (`LIMIT = 10`); `Html.text(html)` (the text a page shows: no tags, scripts, styles or comments, entities decoded, a line per block; untrusted if the page is); constants in a type (`API = "…"`: a class method without arguments, read as `GitHub::API`, or `API` in the type's own methods; `grenat fmt` keeps them as written); the ternary `c ? a : b` (an `if` with one expression each way: interpreted, checked and compiled as one; `grenat fmt` keeps it as written); the hash shorthand `{query:}` (Ruby 3.1); a `def self.x` calling the other `def self.` of its type without a receiver; `assert !done` (a glued negation as a command argument).

### Phase 7 status: the `Http` client

```ruby
def stars(repo: String) -> Int uses net("api.github.com"), env
  res = Http.get("https://api.github.com/repos/#{repo}",
                 headers: {"Authorization" => "Bearer #{Env.fetch("GITHUB_TOKEN")}"})
  raise "GitHub answered #{res.status}" unless res.ok?
  res.json["stargazers_count"].check { |n| n >= 0 }?
end
```

`Http.get`, `post`, `put`, `patch`, `delete` and `head` take a URL and `query:` (parameters, percent-encoded), `headers:`, `json:` (sent as JSON) or `body:`, and `timeout:` (30 s by default). They return an `HttpResponse`: `status`, `ok?` (2xx), `headers`, `body`, `json`. A status is an answer, not an error; `HttpError` is for no answer at all (connection, timeout).

- **Capabilities.** A request is a `net` effect restricted by host: `uses net("api.github.com")` allows that host only. The checker takes the host of a literal URL (E0300 otherwise); a URL built at run time is checked when the request is made (`CapabilityError`).
- **Taint, both ways.** Nothing untrusted goes out: a model's answer or a response, unchecked, in the URL, headers or body is E0412 (and a `TaintError` at run time). What comes back is untrusted, as a model's answer is: `body`, `headers` and `json` are tainted — a web page can carry a prompt injection as well as a model can. `status` and `ok?` are not. Parsing untrusted text (`Json.parse`) gives untrusted data.
- **Tests.** `grenat test` never reaches the network: `mock_http "GET https://api.github.com/repos/*", json: {…}` (or `status:`, `body:`, `headers:`) answers every matching request of the test — without a method, any method; a trailing `*` matches a prefix. An unstubbed request is an `HttpError`.

### Phase 7 status: databases (`Db`)

```ruby
struct Order
  id: Int
  total: Float
end

def big_orders(db: Database, min: Float) -> Array(Order) uses db.read
  db.query("SELECT id, total FROM orders WHERE total > ? ORDER BY total DESC", [min], as: Order)
end

def main uses db, env
  db = Db.connect(Env.fetch("DATABASE_URL"))   # sqlite://app.db, sqlite::memory:, postgres://…
  db.migrate("CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, total FLOAT)")
  db.transaction do
    db.execute("INSERT INTO orders (total) VALUES (?)", [42.0])
  end
  p big_orders(db, 10.0)
end
```

`Db.connect` opens SQLite (embedded: nothing to install) or PostgreSQL. `query` gives rows as hashes, or as records with `as: Order` (typed `Array(Order)` for the checker); `first` the first one or `nil`; `execute` the number of rows changed; `migrate` runs a script; `transaction` commits the block, or rolls it back if it raises. Placeholders are `?` on every database (numbered for PostgreSQL); a value is never SQL text. The layer (`grenat_db`) knows nothing of the language: cells in, cells out.

- **Effects.** Reads are `db.read`, writes `db.write` (`uses db` for both), checked statically and at run time.
- **Taint.** SQL text is never untrusted — a model writing a query must have it checked first (E0412, `TaintError`). An untrusted value may filter a read (it is a parameter), never be written unchecked.
- **Limits.** A connection is shared by the tasks that use it: statements of concurrent tasks may interleave inside a `transaction`; PostgreSQL columns of other types than booleans, integers, floats and text are cast in the query (`created_at::text`).

### Phase 7 status: programs (`Shell`) and tool timeouts

```ruby
def restart(service: String) -> Bool uses shell("kubectl")
  res = Shell.run(["kubectl", "rollout", "restart", "deploy/#{service}"], timeout: 120)
  warn res.stderr.trust! unless res.ok?
  res.ok?
end
```

`Shell.run` takes an **argument vector**, never a shell line: no argument is interpreted by a shell, so there is no shell injection to guard against. Options: `timeout:` (60 s by default; the program is killed after), `cwd:`, `env:` (added to a clean environment: only `PATH`, `HOME` and `LANG` are passed on), `network: false` (no network: `sandbox-exec` on macOS, `unshare` on Linux; where neither exists, the call fails rather than run with network). It returns a `ShellResult`: `status`, `ok?`, `stdout`, `stderr`.

- **Capabilities.** Running a program is a `shell` effect restricted by program: `uses shell("kubectl")` allows `kubectl` only, statically (the program of a literal vector) and at run time.
- **Taint.** No untrusted argument or environment value goes in (E0412, `TaintError`); what a program prints is untrusted.
- **Tests.** `grenat test` never starts a program: `mock_shell "kubectl rollout restart*", stdout: "…"` (or `status:`, `stderr:`) stands for it.
- **Tool timeouts.** `tool_timeout 20` in an agent: a tool call still running after that is cancelled at its next checkpoint and reported to the model as a `TimeoutError`, like any tool error. A blocking call (a request, a program) is bounded by its own `timeout:`.

### Phase 7 status: MCP servers

```ruby
mcp :linear, url: "https://mcp.linear.app/mcp", headers: {"Authorization" => "Bearer #{Env.fetch("LINEAR_TOKEN")}"}
mcp :files, command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "./docs"], approve: false

agent Pm
  model :smart
  tools mcp(:linear), mcp(:files, only: ["read_file"])
  on Plan(spec: String) -> ~String
    run "Create the issues of #{spec}"
  end
end
```

`mcp :name` declares a Model Context Protocol server, reached at a `url:` (streamable HTTP, `headers:`) or run as a `command:` (stdio, `env:`); it is connected when first used. `tools mcp(:linear)` hands its tools to an agent — their names, descriptions and schemas come from the server (`linear__create_issue`, sent non-strict: the schemas are the server's) — or only some (`only: [...]`). `Mcp.tools(:name)` lists them, `Mcp.call(:name, "tool", {…})` calls one directly. The client (`grenat_mcp`) speaks JSON-RPC 2.0 and knows nothing of the language.

- **Capabilities.** Using a server is an `mcp` effect restricted by server: `uses mcp("linear")`; an agent with a server's tools needs it wherever it is asked.
- **Approval.** A tool its server does not declare read-only may change things: the human approves each call first (`approve: false` on a server you trust); a denial is reported to the model as a tool error.
- **Taint.** Nothing untrusted goes to `Mcp.call`, and what it returns is untrusted.
- **Tests.** `mock_mcp :linear, tools: {"create_issue" => "created L-1"}` stands for a server; `call(:linear__create_issue, …)` in a model's mocked replies calls one of its tools.

### Phase 7 status: documents and images

```ruby
prompt extract(invoice: Attachment) -> ~Invoice using :fast
  user "Extract this invoice.", invoice
end

extract(Pdf.read("invoices/a.pdf"))       # Image.read("scan.png"), Pdf.url("https://…")
```

`Pdf.read` and `Image.read` (`.png`, `.jpg`, `.gif`, `.webp`) read a file (an `fs.read` effect) into an `Attachment`; `Pdf.url` and `Image.url` point to one the provider fetches. In a prompt, `user` takes attachments among its texts: a message becomes document and image blocks first, then its text; a message of text only is sent as before, so recorded cassettes stay valid.

### Phase 7 status: triggers (`every`, webhooks) and `grenat serve`

```ruby
every cron: "0 8 * * MON" do            # or: every 1.hour
  weekly(Time.today, ["rust-lang/rust"])
end

on_webhook "/github", secret: Env.fetch("GITHUB_WEBHOOK_SECRET"), signature: :github do |req|
  event = req.json.check { |e| e["pull_request"] }?
  review(event["pull_request"]["number"]) if event["action"] == "opened"
  "ok"
end
```

`grenat serve [--listen host:port] app.grn` runs the script, then its triggers until stopped: each schedule on a task of its own (a failed run is reported, not fatal; cron schedules are in UTC: minute, hour, day, month, weekday, with ranges, lists, steps and names), and an HTTP server for the webhooks (127.0.0.1:3000 by default), each request on a task of its own. `grenat run` only declares triggers.

- **Proof.** `signature: :github` checks the body's HMAC-SHA256 (`X-Hub-Signature-256`) with the secret, in constant time; `token:` a bearer token. A request that proves nothing reaches no handler (401).
- **Taint.** The `body`, `headers` and `json` of a `WebhookRequest` are untrusted; its `method`, `path` and `query` are not.
- **Responses.** A handler's value is the response: a string (200), a hash or record (200, JSON), an integer (that status), `nil` (204).
- **Tests.** `deliver_webhook "/github", json: {…}` sends a request, signed as its sender would sign it, through the same checks, and returns `{"status" => …, "body" => …}`.

The layer below (`grenat_serve`: cron, calendar, signatures, the HTTP server) knows nothing of the language; it is the start of phase 8's `Web` and `Jobs`.

### Phase 7 status: email (`Mail`)

```ruby
def notify(summary: String) uses net("smtp.mail.com"), env
  mailer = Mail.connect(Env.fetch("SMTP_URL"))    # smtps://user:password@smtp.mail.com:465, or smtp:// (STARTTLS)
  mailer.send(from: "bot@acme.com", to: ["team@acme.com"], subject: "Weekly watch", body: summary)
end
```

Sending is a `net` effect on the SMTP server's host, checked when sending. A message reaches people: nothing untrusted goes in it (E0412, `TaintError`). Tests never send: in `grenat test`, messages are kept in `Mail.deliveries` (hashes: `from`, `to`, `subject`, `body`), empty at the start of each test.

### Phase 7 status: batches (`batch_map`)

```ruby
invoices = paths.batch_map { |path| extract(Pdf.read(path)) }   # one batch: half the price
```

`xs.batch_map { … }` runs the block for each element, and sends the model calls it makes through the provider's batch API (Anthropic's Message Batches: half the price, answered within 24 hours, usually minutes). Each element runs on a green task of its own: a task that calls a model queues its request and sleeps; once every task sleeps or is done, the queued requests go out as one batch, and each answer wakes its task where it stopped — a second call is a second round. Budgets count batched calls at half their cost. Mocks, cassettes and the test provider answer batches one request at a time, so tests need nothing new. Tasks a batched task starts (`parallel_map`…) call models directly.

### Phase 7 status: facets, the `Facetfile` and `setter`

```ruby
# Facetfile
source "https://github.com/grenat-lang/facets"      # an index: a git repository
facet "http_tools", "~> 0.3"                        # from the index, by version
facet "utils", path: "../utils"
facet "greet", git: "https://github.com/x/greet", tag: "v1.0.0"
```

A **facet** is a library: a package (`grenat.toml`, `src/lib.grn`) whose versions are its repository's tags (`v1.2.3`). An **index** is a git repository listing facets (`facets/<name>.toml`: `git = "<repository>"`). A package lists the facets it uses in its **`Facetfile`** — Grenat code, read by Grenat's parser — with Ruby's requirements (`~> 0.3` is at least 0.3 and below 1.0; `>= 1.0, < 2`; `= 1.0.0`). **`setter`** installs them: the highest version every requirement allows, found in the indexes, fetched into `.grenat/facets/<name>-<version>`, and recorded — version, commit, directory — in **`Facetfile.lock`**, which later installs keep until `setter update`. A facet's own `Facetfile` adds its facets; all of a program's facets share one namespace, so requirements on a facet must agree on one version (else the conflict is reported). `require "http_tools"` loads an installed facet; one that is listed but not installed is an error that says to run `setter install`.

```sh
setter new greet          # a facet: grenat.toml, Facetfile, src/lib.grn, tests/, README
setter publish            # tags its version (v0.1.0) for the indexes
setter add greet          # in an application: `facet "greet", "~> 0.1"` (the latest), installed
setter install | update | list
```

`setter` is a binary of its own (`grenat_setter`); the resolution is `grenat_package`'s, shared with `grenat`. The `[dependencies]` of `grenat.toml` (path and git) still work, for programs that need no index.

### Phase 7 status: conversations

```ruby
chat = Conversation.load("memory.json", model: :fast, system: "You are a helpful assistant.", keep: 10)
puts chat.say("I am Ada")        # the answer, untrusted
chat.save("memory.json")
```

A `Conversation` keeps the turns of an exchange with a model and sends them with each `say` (an `llm` effect; the answer is untrusted). Beyond `keep` exchanges (20 by default), the older half is summarized by the model and dropped: the summary (`chat.summary`, untrusted too) goes with the system prompt, so the context stays bounded without losing what matters. `history` gives the turns kept; `save` and `load` (`fs.write`, `fs.read`) keep a conversation between sessions — `load` of a file not written yet starts a new one.

### Phase 0.5 status: `grenat fmt`

`grenat fmt [--check] <files or directories>` rewrites Grenat code in its canonical layout (`grenat_fmt`): printed back from the syntax tree with two-space indentation, spaces around operators, only the parentheses the grammar needs (the printer uses the parser's binding powers), one blank line at most, end-of-line comments aligned, arrays, hashes, argument lists and method chains longer than 100 columns broken one item per line. What the syntax leaves to the author is kept: literals as written (`2_000_000`, escapes), modifiers (`x if c`, `x rescue y`), `unless`/`until`, `{ }` or `do … end` blocks, one-line `if … then … end`, `in X then y` and `else y`, heredocs (re-indented with their statement). Comments stay above the code they preceded, or at the end of its line.

Formatting cannot change a program: the result must parse to the same tree (positions aside), keep every comment, and be stable (formatting it again changes nothing); otherwise the file is left untouched and the problem reported. Every example and every code block of this specification is checked this way by the tests; `--check` fails on files not yet formatted, for CI.

### Phase 1 status

The interpreter executes the AST directly. What works:

- the core language: functions, blocks and closures, `struct`, `class`, `module`/`include`, `enum`, `case/in`, `Result` and `?`, `rescue`/`ensure`, a basic library (`Array`, `Hash`, `String`, `File`, `Dir`, `Math`, `Json`, `Env`);
- `prompt`: the return type becomes a JSON Schema (structured output), `##` comments become descriptions, the answer is validated, retried once if invalid, and returned **tainted**;
- `agent`: `spawn`, `ask`/`tell`, `@…` state, the `run` loop with the declared `tool`s and a `final_answer` tool typed by the handler's return type;
- `~T` taint: propagated through field access, interpolation, operators and blocks; `TaintError` if it reaches a function with the `net`, `shell`, `fs.write` or `human` effect; `.check`, `.approve(by: :human)`, `.trust!`;
- `usd`/`tokens`/`time` budgets (`within budget(…)`, the agents' `budget` directive), cost computed per model;
- `Runtime.on_approval`, `approve!`, `grenat test` with `assert`, `assert_equal`, `assert_raises`.

### Phase 2 status

`grenat check` and `grenat run` check the program before running it (`--unchecked` skips this). The checker is **gradual**: what it cannot type (JSON, dynamic values) becomes unknown and is never reported, to avoid false positives.

- **Taint**: the analysis follows a `~T` value through fields, interpolation, operators, blocks, arrays (`<<`, `push`), agents' `@…` state and function calls. Each function is checked for the actual taint of its arguments, so a helper function works on a clean value as well as on a tainted one, with no annotation.
- **Effects**: inferred from the body, propagated through calls (including `ask` and an agent's tools), compared against `uses`, taking literal restrictions into account (`fs.read("./docs")` covers `./docs/a.md`).
- **At run time**: `fs.read(…)` / `fs.write(…)` restrictions are enforced (`CapabilityError`, including against `../`). A method called on a tainted value keeps a tainted `self`. Taint is still checked at run time, as defense in depth.

### Phase 3 status

Each task (main program, `parallel_map` item, `race` branch, `tell` message) has its own call stack; global state and values are shared (`Arc`/`Mutex`).

- **Actors**: an agent handles **one message at a time**. `ask` runs the handler in the caller's task, under the agent's lock: an idle agent costs no thread. `tell` runs in the background; its failure is logged (`[tell] agent … failed`) and the program waits for background tasks before exiting.
- **Deadlocks detected**: a message to oneself, or a waiting cycle between agents (even across several tasks), raises `DeadlockError` with the cycle (`A → B → A`) instead of hanging.
- **`spawn_pool(T, size: n)`**: each message goes to the least busy agent, picked and reserved atomically.
- **`parallel_map(limit: n)`** (8 by default): results in order; the first error is raised and cancels the rest.
- **`race do … end`**: each statement is a branch; the first one to succeed wins, the others are cancelled; if all fail, the first error is raised.
- **Cooperative cancellation**: checked at every statement, call, block, loop iteration and during `sleep` (`Cancelled` error).
- **Shared budgets**: tasks created inside a `within budget(…)` spend the same budget.
- **Supervision**: a handler that raises makes the agent restart (fresh `@…` state) according to `strategy:` — `:one_for_one` (that agent only), `:one_for_all` (all children), `:rest_for_one` (that agent and those declared after it). Beyond `max_restarts` (3 by default) within the `within:` window (60 s), the agent is stopped and later messages raise `AgentDown`. The caller always receives the error. An unsupervised agent keeps its state.

Temporary simplifications, lifted in later phases:

| Today | Later |
|---|---|
| Gradual typing, `T?` accepted where `T` is expected | full inference, `nil` checking |
| Native functions cover numbers, strings, arrays, structs, not hashes, enums, closures or agents; values cross the interpreter boundary by copy; a built executable embeds the interpreter for the rest, unless the whole program compiles (`--native`) | more of the language in native code |
| A cancelled task finishes its in-flight LLM call (billed) before stopping | cancellation of in-flight HTTP requests |
| The `net("host")` restriction is only checked statically | HTTP client in the standard library |

### Phase 4a status: a JIT for numeric functions

When a program loads, `grenat_codegen` compiles to machine code (Cranelift) every top-level `def` whose parameters and return type are annotated `Int`, `Float` or `Bool` and whose body uses only arithmetic, comparisons, `&&`/`||`/`!`, locals, `if`/`elsif`/`else`, `while`, `return`, a few numeric methods (`abs`, `to_f`, `to_i`, `zero?`, `even?`, `odd?`, `Math.sqrt`) and calls to other such functions. Everything else stays interpreted; `grenat run --log` says which functions run natively and why the others do not.

Native code is **invisible**: the interpreter calls it only when the actual arguments have exactly the declared types, and it reproduces the interpreter's semantics to the bit — checked integer arithmetic (`OverflowError`), division rounded toward −∞ and remainder with the sign of the divisor, `ZeroDivisionError`, the same recursion limit, taint flowing from arguments to result. Differential tests run the same programs with and without the JIT (`--no-jit`, `GRENAT_JIT=0`) and require identical output.

Calls between native functions pass the recursion depth in registers and return a status next to the value: no memory traffic, no unwinding. Measured on Apple M-series, release builds, `fib(35)`:

| | Time |
|---|---|
| Interpreter | ~10.5 s |
| **Grenat JIT** | **0.05 s** |
| Rust `-O` with overflow checks (same semantics) | 0.03 s |
| Rust `-O` (no overflow checks) | 0.02 s |

That is ~1.7× Rust with equal semantics, inside the 1–2× goal. The remaining gap is the recursion-depth check Rust does not perform.

The interpreter itself now guards its native stack: a deeply nested recursion raises `StackOverflow` instead of crashing, in the main thread and in tasks.

### Phase 4b status: strings, arrays and structs, with Perceus reference counting

Native functions now also take and return `String`, `Array(T)` (`T` a scalar, a string or a struct) and structs whose fields have those types. Their bodies may use string literals and interpolation, `+`, `*`, comparisons, `length`, `[i]`, `upcase`, `strip`, `include?`…; array literals (`[]` included: the element type is inferred from later uses), `xs[i]`, `xs[i] = v`, `xs[i] += v`, `<<`/`push`, `pop`, `first`, `last`, `sum`, `+`; struct construction (`Point(x: …)`, `Point.new(…)`) and field reads; loops over blocks, compiled inline: `n.times`, `a.upto(b)`, `xs.each`, `xs.each_with_index`; `return` inside `while`.

Objects live in `grenat_runtime`: a reference count at offset 0, then the payload. Memory management is **Perceus**, as in Koka:

- a liveness analysis tells, for every read of a variable, whether it is the last one: the last use *moves* the reference, the others *borrow* it (a borrowed value used while a later operand could release the variable takes its own reference); variables that die on entering a branch or leaving a loop are dropped on that edge;
- `dup`/`drop` are inlined; freeing goes through the runtime, which releases children according to a per-type *shape*;
- **reuse**: `out = out + s` appends in place when `out` is uniquely owned, and `p = Point(x: p.x + 1.0, y: p.y)` builds the new point in the memory of the old one (drop-reuse). 1,000 iterations of either allocate nothing;
- errors and deoptimizations release everything held on the way out. Every test call checks that the live-object count comes back to where it was.

Values cross the boundary with the interpreter **by copy**, so native objects never leave the thread of the call and their counts need no atomics. Arrays being references in Grenat, the content of each array argument is written back after the call (only for functions that may modify an array, themselves or through their callees), an array passed twice stays one array, and an argument array returned comes back as the same object.

What native code cannot represent — `nil` from `xs[99]` or `[].first`, an assignment past the end that the interpreter pads with `nil`, `"x" * -1` — **deoptimizes**: the call is interpreted again from the start. The same happens when an error interrupts a function after it modified an array argument, since native code worked on a copy. Differential tests check aliasing, write-back, deoptimization and partial mutation against the interpreter.

`examples/objects.grn` (2 million particle steps, a sieve up to 2 million, a 1.1 MB CSV string), release builds:

| | Time |
|---|---|
| Interpreter | ~12.7 s |
| **Grenat JIT** (whole process: parse, check, compile, run, copies at the boundary) | **0.05 s** |
| Rust `-O` (equivalent code, `Copy` structs, `Vec<bool>`, `String::push`) | 0.013 s |

The gap to Rust comes from the copies at the boundary (the 149,000 primes cross it three times), an allocation per `to_s`, and reference counts Rust does not need for a `Copy` struct.

### Phase 4c status: `grenat build`

```sh
grenat build app.grn            # → ./app
grenat build app.grn -o bin/app
./app arg1 arg2                 # no `grenat`, no source file, nothing compiled at run time
```

`grenat build` checks the program, then compiles its eligible functions **ahead of time**: the same Cranelift code as the JIT, emitted into an object file (`cranelift-object`) instead of memory. The object also holds an *image*, exported as `grenat_image`: the program's source, the names of the compiled functions, their entry points, and the table of shapes. The system `cc` links it with `libgrenat_host.a`, a static library containing the runtime, the interpreter and a `main`.

At startup the executable parses its embedded source, links its native functions (it checks that they are exactly the ones this version would compile, then fills the table of shapes), and runs the program as `grenat run` does: same output, same exit code, same errors, `GRENAT_LOG` shows `[aot] native: …`. The CLI and the executables share `grenat_driver`, so they cannot drift apart.

To make one code generator serve both, compiled code embeds no absolute address: string literals and object shapes (plain `repr(C)` data describing how to release an object) are data of the module, referenced by symbol. Two details found on the way: data holding pointers must be 8-byte aligned, and must not be zero-fill (`bss`) since such a section cannot carry relocations — Apple's linker crashes instead of reporting it.

A release executable weighs ~6.5 MB (5.4 MB stripped) and runs `examples/objects.grn` in 0.05 s, as fast as the JIT: what runs natively is identical, only the compilation moved to build time.

### Cancellation checkpoints in native code

Every native function entry and loop iteration is a checkpoint: it reads a flag of the call's context. A ticker thread raises the flag of every running native call every 10 ms (and it starts raised), and native code then asks the host whether its task was cancelled; if so it stops with `Cancelled`, releasing everything it holds. A `race` whose losing branch is a native loop of 10¹² iterations now ends as soon as the winner does.

Counting steps instead would chain every call to the previous one: an earlier version counted down in memory, then in registers passed from call to call, and both slowed `fib(38)` by 35–45 %; so did a third hidden parameter (one more register to save around each recursive call). The final version passes one pointer (depth and context) and reads one byte: `fib(38)` runs in 0.23 s, as before.

### Phase 4d status: M:N green threads

Tasks (`parallel_map`, `race`, `tell`, the program itself) are now green threads (`grenat_green`): stackful coroutines (`corosensei`) run by one worker thread per core. A task that waits parks and its worker runs another one:

- the agent's turn, waiting for tasks, `parallel_map`/`race` results use green primitives (`Mutex`, `Condvar`, channel) that park the task, not the thread;
- `sleep` registers with a timer thread; LLM calls and standard input run on a separate thread (`blocking`) while the task parks;
- scheduling is cooperative: the interpreter yields every 4096 calls or loop iterations, native code at its checkpoints (every 10 ms);
- stacks are reserved (512 MB for the program, 16 MB per task) and only touched pages are committed; stacks of finished tasks are reused.

`parallel_map(limit: 100_000) { sleep 0.5 }` over 100,000 items: 2.8 s and 1.05 GB resident (~10 KB per waiting task); the previous version, one OS thread per task, fails to create the threads. With 10,000 tasks: 1.2 s instead of 2.4 s.

Two pitfalls of stackful coroutines, handled in `grenat_green`: a task may resume on another thread, so the compiler must not reuse a thread-local's address across a suspension (the current task is only read in functions that are never inlined), and no OS lock may be held across a suspension point (the interpreter's long-held locks became green locks). A lock hands over to the next waiter by ticket, so a late wake-up (a timer firing after its sleeper left) cannot be spent on a task that no longer waits. A hang seen once while developing this was not reproduced afterwards (hundreds of runs, including under load); both fixes above address plausible causes.

### Phase 4e status: programs without the interpreter

```sh
grenat build --native app.grn   # the whole program in machine code, ~0.5 MB
```

When every function of a program compiles (`main` included) and it has no top-level statements, `--native` builds it with no interpreter inside: the object file exports its `main`, its source and its error sites, and is linked with `libgrenat_standalone.a` (the runtime and a `main` that runs it). Native code gains, for such programs, procedures (functions without a return type), `main` with or without `args: Array(String)`, `puts`, `print`, `p` (printed by the runtime from a descriptor of the value's type, exactly as the interpreter prints) and `exit`.

Errors are reported as `grenat run` reports them: every trap and deoptimization of native code records a *site* (function, source span), written into the call's context when taken. What the interpreter would go on with — `nil` from `xs[99]` — stops a native program with a `NativeError` at that site: this is the one difference, and why `--native` is explicit. A program that needs the interpreter is refused with every reason (top-level statements, untyped parameters, agents, prompts…).

To build it, the structures shared by compiled code and its hosts moved to `grenat_runtime` (the call context, statuses, the standalone descriptor), object shapes became plain data of the module, and diagnostic rendering moved to `grenat_report`: the standalone library needs neither Cranelift, nor the parser, nor the interpreter.

`examples/objects.grn` built `--native`: 0.49 MB (0.39 MB stripped), 0.04 s; the same built with the interpreter: 6.5 MB, 0.05 s; Rust: 0.012 s. The gap is in string building: each evaluation of a literal and each `to_s` allocates a string (static literals are the next step).

Next slices (native `main`, I/O, the agent runtime in native code); M:N green threads.

### Phase 5 status: durable workflows, tests and evals

**Workflows.** A run of a `workflow` is identified by the workflow and its arguments (which must be data). Its journal is a JSON Lines file, `.grenat/journal/<name>-<hash>.jsonl`, appended and synced after each `step`: the step's value, in an exact tagged encoding (`Float`, `Money`, symbols, records, variants, errors and taint survive the round trip). Run again after a crash, the workflow replays its completed steps from the journal — their blocks do not run, no model is called — and goes on from the first missing one; a completed workflow returns its recorded result at once. Steps are numbered by name in the order they run, so a step inside a loop is journaled once per iteration. A step must return data. The checker enforces the rule of §7 (E0310): in a workflow, `llm`, `net`, `human`, `time` and `random` effects must be inside a `step`.

**Tests** (`grenat test`) never reach a real model: a call that is neither mocked nor in a cassette is an `LlmError`.

- `mock :fast, replies: [...]` (or `mock replies: [...]` for every model): the next calls get these replies, in order, shaped into what each call expects — plain text, the structured output of a `prompt` (wrapped when the type is not an object), or the `final_answer` of an agent. `call(:tool, arg: …)` is a reply calling one of the agent's tools, an error value (`LlmError("overloaded")`) makes the call fail. Mocks last until the end of their test.
- `cassette "name" do … end`: the calls of the block are replayed from `cassettes/<name>.json` (matched by their exact request, so concurrent calls may come in any order), or recorded there, with the real model, when the file does not exist yet or with `GRENAT_RECORD=1`. A recording is kept only if the block succeeds. A mock wins over the enclosing cassette.
- `fixture("name")`: a file of `fixtures/` — data for `.json` and `.jsonl`, text otherwise.

`cassettes/`, `fixtures/` and datasets are found next to the program.

**Evals** (`grenat eval <file> [name]`) call the real models. `eval "name", dataset: "rows.jsonl", threshold: 0.8, concurrency: 8 do |row| … end` gives each row (a JSON object, as a `Row` record) to the block, which returns a score: `true`/`false`, or a number from 0 to 1. `judge(:model, question, material…)` asks a model for that number, through structured output (brief reasoning, then the score). Rows run concurrently; a row that fails scores 0 and its error is shown. The report gives, per eval, the mean score against the threshold (1.0 by default), the rows that failed, the cost (counted per row) and the duration; the command fails if an eval is under its threshold.

```text
✓ routing · score 0.80 (threshold 0.80) · 5 row(s) · $0.0031 · 1.2s
✗ replies are kind · score 0.62 (threshold 0.70) · 5 row(s), 1 failed · $0.0104 · 3.4s
    row 4: LlmError: truncated response: increase the model's `max_tokens`
```

### Phase 6 status: packages

Programs of several files and packages (§2, *Files and packages*) are in `grenat_package`: the manifest, the lock file, git dependencies (with the `git` command), `require` resolution and loading. Loading gives the program's `Sources`, every file's text one after the other: each file is parsed alone first (its syntax errors reported in it), then the whole program is parsed once, so that spans stay offsets in one text and nothing downstream — the checker, the interpreter, the code generators — knows about files. Diagnostics and runtime errors are rendered in the file they point into (`grenat_report`), and built executables embed the file table next to the source, so their errors do too.

### Phase 6 status: the language server

`grenat lsp` (`grenat_lsp`) speaks the Language Server Protocol over standard input and output. Each change of a document checks its whole program — the files it requires, open buffers taking precedence over the disk, so that an unsaved change of a library shows at once in the files that use it — and publishes the diagnostics of every open document, with their codes. It formats documents as `grenat fmt` does (a document that does not parse is left alone), shows the first line and `##` documentation of the function, type, variant, field or model under the cursor, goes to its definition in whichever file it is, and lists a document's top-level symbols. Positions are counted in UTF-16 units, as the protocol requires.

### Phase 6 status: macros

Macros are expanded by `grenat_macros`, between the resolution of `require`s and the checker. The lexer reads a macro's body raw, up to the `end` at the macro's indentation (a template is not Grenat code until expanded), and `grenat fmt` prints it as written. Expansion is textual and recursive: a template is rendered, the macros it invokes are expanded in place, and the final text is parsed once with every token given the invocation's span — which is how errors in generated code point at the invocation. Inside the body of any type, a line starting with a name is now a directive: a macro invocation, or else, outside agents and supervisors, an unknown macro. The language server expands macros too, and shows a macro's documentation over its invocations.

### Phase 6 status: release builds through LLVM

`grenat build --release` (with or without `--native`) has LLVM optimize and compile the code. The front end is shared: `grenat_codegen::llvm::LlvmModule` is a `cranelift_module::Module` that records the Cranelift IR of every function and the contents of every data object instead of compiling them, then translates the whole module into LLVM IR — values and blocks keep their numbers, block parameters become `phi`s (a conditional branch goes through edge blocks), overflow checks become `llvm.s*.with.overflow`, float operations LLVM intrinsics, addresses `inttoptr`/`ptrtoint` — which `clang -O3` compiles (`GRENAT_CLANG` chooses the compiler). An instruction the translator does not know is an error, never a silent difference; differential tests build programs both ways and compare them with `grenat run`, down to overflow and division errors.

Measured on this machine (Apple M-series, `--native`): `fib(38)` 0.33 s with Cranelift, 0.26 s with LLVM; `examples/objects.grn` 0.05 s and 0.04 s. What remains is the price of the semantics — checked arithmetic, the recursion depth, the cancellation flag, a status returned with every result — not of the code generator.

For the models that recommend it (`claude-opus-5`, `claude-fable-5-1`), the client enables server-side fallbacks (`fallbacks: "default"`): a request refused by a classifier is replayed on another model instead of failing. Disable it with `model :x, …, fallbacks: false`.

Performance goal: for CPU-bound code, stay **between 1× and 2× Rust**, like Crystal or Swift. On the agent side, support **100,000 concurrent agents** on a single machine (an idle actor ≈ 2 KB).
