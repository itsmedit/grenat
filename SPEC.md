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
| `method_missing`, `send`, `eval`, `instance_eval` | ❌ replaced by compile-time **macros** (v0.3) |
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

A `workflow` survives crashes, redeployments and human waits lasting several days. Every `step` is **journaled** (SQLite by default, Postgres optionally). On restart, completed steps are **replayed from the journal**: no LLM call is ever billed twice.

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
│   ├── grenat_host       # static library linked into every executable (runtime + interpreter + main)
│   └── grenat_cli        # grenat run | build | test | eval | fmt
└── std/                  # standard library written in Grenat
```

Error messages: every diagnostic has a stable code, the offending line and, for taint, **the place where the LLM produced the value**. Actual output of `grenat check` when the agent's unvalidated answer is sent in `support_desk.grn`:

```
error[E0412]: an LLM-produced value reaches `send_reply` (effect `net`) without validation
   --> support_desk.grn:163:29
    |
163 |     send_reply(ticket.from, answer.body)
    |                             ^^^^^^^^^^^
note: produced here by an LLM
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
| Linker | the system `cc` (Xcode CLT on macOS, `gcc` on Arch), the only runtime dependency |
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
| **0.5** | `grenat fmt` (comment-preserving) | canonical formatter |
| **1** ✅ | Interpreter, `prompt`, `tool`, agents, budgets, taint, Anthropic client, `grenat run/test` | the first agent runs |
| **2** ✅ | Names, types, effects and `~T` taint checked **before execution**; capabilities enforced at run time | security errors before execution |
| **3** ✅ | Concurrent actor agents, real `parallel_map`/`race`, cancellation, deadlock detection, supervision | multi-agent |
| **4** 🚧 | Cranelift codegen (4a ✅ JIT for numeric functions, 4b ✅ strings/arrays/structs with Perceus RC, 4c ✅ `grenat build`, 4d ✅ M:N green threads), then fully native programs | fast native binaries |
| **5** | Durable workflows (`step` journal), cassettes, `mock`, `eval` | production-ready |
| **6** | LSP, LLVM release builds, macros, package manager | ecosystem |

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
| Native functions cover numbers, strings, arrays, structs, not hashes, enums, closures or agents; values cross the interpreter boundary by copy; a built executable embeds the interpreter for the rest | whole programs compiled natively |
| A cancelled task finishes its in-flight LLM call (billed) before stopping | cancellation of in-flight HTTP requests |
| `step` runs its block without a journal | durable journal (phase 5) |
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

Next slices: whole programs compiled without the interpreter (native `main`, I/O, the agent runtime in native code); M:N green threads.

For the models that recommend it (`claude-opus-5`, `claude-fable-5-1`), the client enables server-side fallbacks (`fallbacks: "default"`): a request refused by a classifier is replayed on another model instead of failing. Disable it with `model :x, …, fallbacks: false`.

Performance goal: for CPU-bound code, stay **between 1× and 2× Rust**, like Crystal or Swift. On the agent side, support **100,000 concurrent agents** on a single machine (an idle actor ≈ 2 KB).
