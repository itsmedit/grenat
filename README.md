# Grenat

> Ruby's syntax, Rust's speed, agents as first-class citizens.

Grenat is a compiled programming language for building AI agent systems: typed prompts,
tools, supervised actor agents, budgets, durable workflows, and an effect system that
turns prompt injection into a **compile-time error**.

```ruby
prompt summarize(article: String) -> ~Summary using :fast
  user "Summarize: #{article}"
end

agent Researcher
  model :smart
  tools search_web, read_url
  budget usd: 2.00, time: 10.min

  on Research(topic: String) -> ~Report
    run "Investigate #{topic}"
  end
end
```

- Specification: [`SPEC.md`](SPEC.md)
- Examples: [`basics.grn`](examples/basics.grn), [`reviews.grn`](examples/reviews.grn) (native statistics + validated LLM analysis), [`explorer.grn`](examples/explorer.grn) (a real agent), [`support_desk.grn`](examples/support_desk.grn) (multi-agent, human approval), [`triage.grn`](examples/triage.grn) (tests with mocks, evals with an LLM judge), [`macros.grn`](examples/macros.grn) (compile-time code generation), [`usecases/`](examples/usecases) (ten agent use cases, the phase 7 target)

## Status

**Phase 10 — configured, not coded**: secrets encrypted per environment as with Rails
(`grenat credentials edit`), as `Secret` values the language keeps away from models and logs;
models of ten providers in `config/models.yml`, each reached by the right connector with its key
found in the credentials — the provider's name is enough.

**Phase 9 — agents operated from a browser**: `grenat console`, open source like the rest —
approvals waiting for a human, jobs and their workflow journals (retry), what the models cost
by agent, workflow and day, eval scores over time, failures and refusals, MCP servers. The
runtime records what it shows in the application's database.

**Phase 8 — applications of agents**, in the language and its toolchain, with no
framework on top: routes, records and migrations (SQLite and PostgreSQL), jobs, approvals
that wait days for a human in the database, tools and agents served to other programs over
MCP and HTTP (`expose`), and generators — `grenat new --app`, then
`grenat generate agent|workflow|record|tool|eval`, each part with its tests.

**Phase 7 — agents in production**, measured by ten real use cases (`examples/usecases`):
an HTTP client, databases, email, a sandboxed `Shell`, MCP servers, PDFs and images,
conversations with long-term memory, the Batch API, schedules and webhooks
(`grenat serve`), and facets — libraries installed by `setter` from a `Facetfile`.

**Phase 6 — ecosystem**: programs of several files and packages (`grenat.toml`, path
and git dependencies, `grenat.lock`), a language server (`grenat lsp`), compile-time
macros, and release builds optimized by LLVM (`grenat build --release`).

**Phase 5 — production-ready**: workflows are durable — each `step` is journaled, and an
interrupted run resumes where it stopped, without paying twice for a model call. Tests
never reach a real model: `mock` gives the model's replies as plain values, `cassette`
records real calls once and replays them. `eval` measures quality on a dataset, with
`judge` (an LLM as a judge), and fails under a threshold.

**Phase 4 — native code**: functions over numbers, strings, arrays and structs are
compiled to machine code by a Cranelift JIT when the program loads — `fib(35)` runs in
0.05 s, about 1.7× Rust with the same overflow semantics, 200× faster than the
interpreter. Objects are reference counted, Perceus style: no garbage collector, no leak,
in-place updates of uniquely owned values. `grenat build` compiles a program ahead of time
into a standalone executable. Tasks are M:N green threads: 100,000 concurrent tasks fit in
~1 GB on a few OS threads. Agents are
actors (one message at a time, deadlocks detected, supervision with restarts), and
`parallel_map` and `race` run truly in parallel. Before running anything, `grenat` checks
names, types, effects and taint: an unvalidated model answer that reaches the network is
a **compile-time error**.

## Install

**macOS or Linux, with Homebrew:**

```sh
brew install itsmedit/grenat/grenat
```

**Any Linux with glibc** (Ubuntu 20.04+, Debian 11+, Fedora, RHEL 9, Amazon Linux 2023…) **or macOS, without a package manager** — the install script puts `grenat` and `setter` in `~/.grenat` (in `/usr/local` as root), checks the archive's SHA-256, and adds them to your `PATH`:

```sh
curl -sSL https://github.com/itsmedit/grenat/releases/latest/download/install.sh | sh
# options: sh -s -- --version v0.1.1 | --prefix DIR | --no-modify-path | --uninstall
```

A fresh EC2 instance (Amazon Linux 2023), for instance:

```sh
sudo dnf install -y gcc                 # the C linker `grenat build` uses (run, test and serve need none)
curl -sSL https://github.com/itsmedit/grenat/releases/latest/download/install.sh | sh
exec $SHELL -l                          # a new shell, with grenat on the PATH
grenat new --app hello && cd hello && grenat test
```

**apt or dnf**, with the packages of a release:

```sh
curl -LO https://github.com/itsmedit/grenat/releases/download/v0.1.1/grenat_0.1.1_amd64.deb
sudo apt install ./grenat_0.1.1_amd64.deb                  # Ubuntu, Debian (arm64: _arm64.deb)
sudo dnf install https://github.com/itsmedit/grenat/releases/download/v0.1.1/grenat-0.1.1-1.x86_64.rpm   # Fedora, RHEL, Amazon Linux (aarch64: .aarch64.rpm)
```

**Docker** — the official image, for amd64 and arm64, with a C linker for `grenat build`:

```sh
docker run --rm -v "$PWD":/app ghcr.io/itsmedit/grenat test
docker run --rm -v "$PWD":/app -p 3000:3000 ghcr.io/itsmedit/grenat serve --listen 0.0.0.0:3000
```

```dockerfile
# your application's image
FROM ghcr.io/itsmedit/grenat:0.1.1
COPY . /app
CMD ["serve", "--listen", "0.0.0.0:3000"]
```

**From the sources** (Rust, and a C linker: Xcode's command line tools on macOS):

```sh
cargo install --locked --path crates/grenat_cli && cargo install --locked --path crates/grenat_setter
cargo build --release -p grenat_host -p grenat_standalone     # the libraries `grenat build` links
mkdir -p ~/.cargo/lib/grenat && cp target/release/libgrenat_{host,standalone}.a ~/.cargo/lib/grenat/
```

Alpine (musl) is not supported by the binaries: use a glibc distribution, or the Docker image.

## Try it


```sh
cargo build
target/debug/grenat run examples/basics.grn            # the core language, no LLM
target/debug/grenat run --log examples/fib.grn        # native code: see what the JIT compiled
target/debug/grenat run --log examples/objects.grn    # strings, arrays, structs, natively
target/debug/grenat build examples/objects.grn && ./objects   # a standalone executable (needs `cc`)
target/debug/grenat build --native examples/objects.grn        # without the interpreter: ~0.5 MB
target/debug/grenat build --native --release examples/fib.grn  # optimized by LLVM (needs clang)

export ANTHROPIC_API_KEY=sk-ant-…            # or, in an application: grenat credentials edit
target/debug/grenat run --log examples/explorer.grn crates/grenat_parser        # a real agent
target/debug/grenat run examples/support_desk.grn examples/tickets.jsonl        # multi-agent + approval

target/debug/grenat new --app desk && cd desk # an application: database, models, routes, tests
grenat generate agent triage                 # a part and its tests (also workflow, record, tool, eval)
grenat migrate && grenat test && grenat serve
grenat console                               # its operations console: http://127.0.0.1:4000

target/debug/grenat new hello && cd hello    # a package: grenat.toml, Facetfile, src/, tests/
setter add http_tools                        # a facet (library) from an index, like a gem
grenat run && grenat test                    # in a package, no file to name

target/debug/grenat check examples/*.grn     # names, types, effects, taint
target/debug/grenat fmt examples             # canonical layout (--check: only report)
target/debug/grenat test examples/triage.grn # `test` blocks: mocks and cassettes, never a real model
target/debug/grenat eval examples/triage.grn # `eval` blocks: the real model, scored on a dataset
cargo test                                   # ~440 tests: unit, integration, CLI, HTTP, MCP, JIT, build
```

## Editors

`grenat lsp` is a language server (diagnostics as you type, formatting, hover, go to
definition, symbols). In Neovim:

```lua
vim.filetype.add({ extension = { grn = "grenat" } })
vim.api.nvim_create_autocmd("FileType", { pattern = "grenat", callback = function()
  vim.lsp.start({ name = "grenat", cmd = { "grenat", "lsp" } })
end })
```

## Layout

| Crate | Role |
|---|---|
| `grenat_lexer` | tokens, interpolation, heredocs, `##` doc comments |
| `grenat_ast` | syntax tree |
| `grenat_parser` | recursive descent + Pratt, diagnostics with error recovery |
| `grenat_llm` | model providers: the catalog, Anthropic's Messages API and Chat Completions (OpenAI, Gemini, Mistral, Ollama…); mocks, cassettes and a scripted provider for tests |
| `grenat_types` | checker: names, types, effects, `~T` taint (E0100–E0500) |
| `grenat_codegen` | Cranelift: typing, liveness (Perceus), translation, boundary; JIT and object files; LLVM IR for release builds |
| `grenat_runtime` | reference-counted strings, arrays and records called by native code |
| `grenat_driver` | load, check and run a program (shared by the CLI and built executables) |
| `grenat_host` | static library linked into the executables of `grenat build` |
| `grenat_standalone` | static library linked into `grenat build --native` executables |
| `grenat_report` | diagnostic rendering, in the file each error points into |
| `grenat_db` | databases: SQLite (embedded) and PostgreSQL behind one interface |
| `grenat_mcp` | the Model Context Protocol: a client (stdio and HTTP), and the server side of `expose` |
| `grenat_serve` | triggers: cron schedules, webhook signatures, the HTTP server of `grenat serve` |
| `grenat_generate` | `grenat new --app` and `grenat generate`: an application's parts, with their tests |
| `grenat_ops` | the operations store: jobs, approvals, model calls, events, eval runs, workflow journals |
| `grenat_console` | `grenat console`: pages and actions of the operations console, and who may use it |
| `grenat_config` | the application's configuration: encrypted credentials per environment, `config/*.yml` |
| `grenat_setter` | `setter`: creates, adds, installs and publishes facets (libraries) |
| `grenat_package` | `grenat.toml`, `require`, facets (`Facetfile`, versions, indexes), path and git dependencies |
| `grenat_fmt` | the formatter |
| `grenat_lsp` | the language server |
| `grenat_macros` | macro expansion: templates of declarations |
| `grenat_green` | M:N green threads: scheduler, green locks, channels, timers |
| `grenat_interp` | interpreter: values, evaluation, prompts, agents, budgets, taint, capabilities, workflows, test doubles, evals |
| `grenat_cli` | the `grenat` binary |

External dependencies: `ureq` (HTTP + rustls), `serde_json`, `toml`, `rusqlite` (SQLite, compiled in), `postgres`, and Cranelift for native code.

## License

Your choice of [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
