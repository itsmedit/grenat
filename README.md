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

```sh
cargo build
target/debug/grenat run examples/basics.grn            # the core language, no LLM
target/debug/grenat run --log examples/fib.grn        # native code: see what the JIT compiled
target/debug/grenat run --log examples/objects.grn    # strings, arrays, structs, natively
target/debug/grenat build examples/objects.grn && ./objects   # a standalone executable (needs `cc`)
target/debug/grenat build --native examples/objects.grn        # without the interpreter: ~0.5 MB
target/debug/grenat build --native --release examples/fib.grn  # optimized by LLVM (needs clang)

export ANTHROPIC_API_KEY=sk-ant-…
target/debug/grenat run --log examples/explorer.grn crates/grenat_parser        # a real agent
target/debug/grenat run examples/support_desk.grn examples/tickets.jsonl        # multi-agent + approval

target/debug/grenat new hello && cd hello    # a package: grenat.toml, src/, tests/
grenat run && grenat test                    # in a package, no file to name

target/debug/grenat check examples/*.grn     # names, types, effects, taint
target/debug/grenat fmt examples             # canonical layout (--check: only report)
target/debug/grenat test examples/triage.grn # `test` blocks: mocks and cassettes, never a real model
target/debug/grenat eval examples/triage.grn # `eval` blocks: the real model, scored on a dataset
cargo test                                   # ~250 tests: unit, integration, CLI, HTTP client, JIT, build
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
| `grenat_llm` | Claude API client (structured output, tools, fallbacks); mocks, cassettes and a scripted provider for tests |
| `grenat_types` | checker: names, types, effects, `~T` taint (E0100–E0500) |
| `grenat_codegen` | Cranelift: typing, liveness (Perceus), translation, boundary; JIT and object files; LLVM IR for release builds |
| `grenat_runtime` | reference-counted strings, arrays and records called by native code |
| `grenat_driver` | load, check and run a program (shared by the CLI and built executables) |
| `grenat_host` | static library linked into the executables of `grenat build` |
| `grenat_standalone` | static library linked into `grenat build --native` executables |
| `grenat_report` | diagnostic rendering, in the file each error points into |
| `grenat_db` | databases: SQLite (embedded) and PostgreSQL behind one interface |
| `grenat_mcp` | a Model Context Protocol client (stdio and HTTP) |
| `grenat_package` | `grenat.toml`, `require`, path and git dependencies, `grenat.lock` |
| `grenat_fmt` | the formatter |
| `grenat_lsp` | the language server |
| `grenat_macros` | macro expansion: templates of declarations |
| `grenat_green` | M:N green threads: scheduler, green locks, channels, timers |
| `grenat_interp` | interpreter: values, evaluation, prompts, agents, budgets, taint, capabilities, workflows, test doubles, evals |
| `grenat_cli` | the `grenat` binary |

External dependencies: `ureq` (HTTP + rustls), `serde_json`, `toml`, `rusqlite` (SQLite, compiled in), `postgres`, and Cranelift for native code.

## License

Your choice of [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
