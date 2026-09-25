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
| Homebrew tap (`itsmedit/homebrew-grenat`) | `brew install itsmedit/grenat/grenat` | from v0.1 |
| homebrew-core | `brew install grenat` | once the project is "notable" (~75 stars), stable release, built from source |
| AUR `grenat` (source) and `grenat-bin` (prebuilt) | `yay -S grenat` | from v0.1 |
| Arch `extra` repository | `pacman -S grenat` | once an Arch packager adopts it |
| Shell installer | `curl -fsSL https://grenat.dev/install.sh \| sh` | from v0.1 |

**Automation**: `dist` (formerly cargo-dist) generates the GitHub Action triggered on every `v*` tag. It builds the macOS arm64/x86_64 and Linux x86_64/aarch64 binaries, creates the GitHub Release, and updates the tap formula and the shell installer. The `grenat-bin` `PKGBUILD` points to the same artifacts.

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
| **7** | What real agents need, measured by ten use cases (`examples/usecases`): an I/O library with effects (`Http` client and server, `Db`, email), MCP client, multimodal prompts and the Batch API, conversations and long-term memory, a sandbox for `shell` and per-tool timeouts, triggers (`every`, webhooks) | agents in production |

### Phase 7 plan: the ten use cases

Ten realistic programs, one per kind of agent, are in `examples/usecases` (the first is `examples/support_desk.grn`): support, code review, research, data, documents, a scheduled digest, operations, a chat with memory, a team of agents, third-party tools. Each checks and passes its tests today, the model mocked, in 24 to 47 lines of logic (113 for the full support desk). Only two run for real: the others fake, between `STUBS` markers, the I/O the standard library lacks. Phase 7 is done when every stub is gone. In order:

1. ✅ **`Http` client** with effects (`net("host")` enforced at run time, JSON, headers, timeouts) — unblocks cases 2, 3, 6, 7, 10.
2. ✅ **`Db`** (SQLite and Postgres, parameterized queries, `db.read` / `db.write` effects) — case 4.
3. ✅ **Sandboxed `shell`** (a process with a timeout, no network unless declared) and per-tool timeouts — cases 2 and 7.
4. **MCP client** (`tools from mcp("…")`, capabilities granted per server, results tainted) — case 10.
5. **Multimodal prompts** (PDF, images) and the **Batch API** — case 5.
6. **Email** and **triggers** (`every 1.week`, webhooks through an `Http` server) — cases 1, 2, 6.
7. **Facets**: libraries shared like Ruby's gems. A *facet* (a garnet's face) is a package; a program lists the facets it uses in its `Facetfile`, pinned in `Facetfile.lock`; the `setter` tool (who sets stones in a jewel) creates, adds, installs, updates and publishes them, with versions (`facet "http", "~> 0.3"`) resolved from git tags through an index repository. It replaces the `[dependencies]` of `grenat.toml`.
8. Smaller gaps met while writing them: the ternary `c ? a : b`; constants in a module (`API = "…"`); HTML to text for case 3; conversations as a type (history compacted automatically) for case 8. ✅ Done: the hash shorthand `{query:}` (Ruby 3.1); a `def self.x` calling the other `def self.` of its type without a receiver.

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
