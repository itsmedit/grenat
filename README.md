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
- Examples: [`basics.grn`](examples/basics.grn), [`explorer.grn`](examples/explorer.grn) (a real agent), [`support_desk.grn`](examples/support_desk.grn) (multi-agent, human approval)

## Status

**Phase 4 — native code**: functions over numbers, strings, arrays and structs are
compiled to machine code by a Cranelift JIT when the program loads — `fib(35)` runs in
0.05 s, about 1.7× Rust with the same overflow semantics, 200× faster than the
interpreter. Objects are reference counted, Perceus style: no garbage collector, no leak,
in-place updates of uniquely owned values. Agents are
actors (one message at a time, deadlocks detected, supervision with restarts), and
`parallel_map` and `race` run truly in parallel. Before running anything, `grenat` checks
names, types, effects and taint: an unvalidated model answer that reaches the network is
a **compile-time error**.

```sh
cargo build
target/debug/grenat run examples/basics.grn            # the core language, no LLM
target/debug/grenat run --log examples/fib.grn        # native code: see what the JIT compiled
target/debug/grenat run --log examples/objects.grn    # strings, arrays, structs, natively

export ANTHROPIC_API_KEY=sk-ant-…
target/debug/grenat run --log examples/explorer.grn crates/grenat_parser        # a real agent
target/debug/grenat run examples/support_desk.grn examples/tickets.jsonl        # multi-agent + approval

target/debug/grenat check examples/*.grn     # names, types, effects, taint
target/debug/grenat test my_file.grn         # `test "…" do … end` blocks
cargo test                                   # ~190 tests: unit, integration, CLI, HTTP client, JIT
```

## Layout

| Crate | Role |
|---|---|
| `grenat_lexer` | tokens, interpolation, heredocs, `##` doc comments |
| `grenat_ast` | syntax tree |
| `grenat_parser` | recursive descent + Pratt, diagnostics with error recovery |
| `grenat_llm` | Claude API client (structured output, tools, fallbacks), scripted provider for tests |
| `grenat_types` | checker: names, types, effects, `~T` taint (E0100–E0500) |
| `grenat_codegen` | Cranelift JIT: typing, liveness (Perceus), translation, boundary |
| `grenat_runtime` | reference-counted strings, arrays and records called by native code |
| `grenat_interp` | interpreter: values, evaluation, prompts, agents, budgets, taint, capabilities |
| `grenat_cli` | the `grenat` binary |

Only two external dependencies: `ureq` (HTTP + rustls) and `serde_json`.

## License

Your choice of [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
