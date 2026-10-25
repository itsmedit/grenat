# Grenat for Visual Studio Code

Support for [Grenat](https://github.com/itsmedit/grenat), the agentic programming language
with Ruby's syntax (`.grn` files and `Facetfile`s).

## Features

- **Highlighting**: keywords, Grenat's declarations (`agent`, `prompt`, `tool`, `workflow`,
  `struct`, `enum`, `test`, `eval`, `on_webhook`, `every`, `limit`, `mcp`, `expose`, `macro`,
  `native def`…), the effects of a `uses` list, untrusted types `~T`, symbols, strings with
  `#{…}` interpolation, heredocs (`<<~P`), numbers with units (`2.min`, `10.per_second`), `##`
  doc comments apart from `#` comments, `{{x}}` and `{% … %}` in macros. Standard scope names:
  any theme colours it.
- **Language server** (`grenat lsp`): diagnostics as you type, formatting (on save by default),
  hover, go to definition, document symbols.
- **Commands** (Command Palette):
  - `Grenat: Run File` — `grenat run <file>` in a terminal;
  - `Grenat: Test File` — `grenat test <file>`;
  - `Grenat: Check Workspace` — `grenat check` over the folder;
  - `Grenat: Restart Language Server`.
- **Snippets**: `agent`, `prompt`, `tool`, `workflow`, `test`, `struct`, `on_webhook`, `every`.
- **Editing**: comments with `#`, brackets and quotes closed, `{{ }}` and `{% %}` in macros,
  indentation after `def`, `do`, `agent`… and back at `end`, folding up to `end`, words
  including their `?` or `!` (`approve!`, `ok?`), `##` continued on Enter.

## Requirements

The `grenat` executable (see [Install](https://github.com/itsmedit/grenat#install)), on `PATH`
or named by `grenat.path`. Without it, highlighting and snippets work; the extension says it
cannot find `grenat` and the language server and commands stay off.

## Settings

| Setting | Default | |
|---|---|---|
| `grenat.path` | `grenat` | the executable: a name looked up on `PATH`, or a path (`~/.cargo/bin/grenat`) |
| `grenat.trace.server` | `off` | `messages` or `verbose` traces the language server in the Grenat output |

For Grenat files the extension sets `editor.formatOnSave`, two spaces of indentation and itself
as the formatter; change them under `"[grenat]"` in your settings.

## Install

The extension is not on the Marketplace: install the `.vsix` attached to a
[release](https://github.com/itsmedit/grenat/releases):

```sh
code --install-extension grenat-0.1.5.vsix
```

or build it from the repository:

```sh
cd editors/vscode
npm ci
npx @vscode/vsce package      # grenat-<version>.vsix
code --install-extension grenat-*.vsix
```

## Development

```sh
npm ci
npm test    # compiles, lints, runs the grammar tests and the unit tests
```

- `test/grammar/*.grn`: assertions on scopes (`vscode-tmgrammar-test`).
- `test/snap/*.grn`: snapshots of examples and of `llms.txt`'s samples (`vscode-tmgrammar-snap`;
  after a deliberate grammar change, `npx vscode-tmgrammar-snap -u "test/snap/**/*.grn"`).
- `src/test/`: unit tests (`node --test`); one starts the real `grenat lsp` when found
  (`GRENAT_PATH`, else `PATH`) and checks its formatting.

## License

MIT or Apache-2.0, at your option, as Grenat itself (see [LICENSE](LICENSE)).
