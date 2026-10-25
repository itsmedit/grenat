// The language client: `grenat lsp` over standard input and output, for
// diagnostics, formatting (and so format on save), hover, go to definition
// and document symbols.

import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";

export function createClient(executable: string, output: vscode.OutputChannel): LanguageClient {
  const run = { command: executable, args: ["lsp"] };
  const serverOptions: ServerOptions = { run, debug: run };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "grenat" },
      { scheme: "untitled", language: "grenat" },
    ],
    outputChannel: output,
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.grn"),
    },
  };
  return new LanguageClient("grenat", "Grenat Language Server", serverOptions, clientOptions);
}
