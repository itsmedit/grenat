// The extension's entry point: starts the language server and registers the
// commands. When `grenat` cannot be found, says so once and goes on without
// the server (highlighting and snippets still work).

import * as vscode from "vscode";
import { LanguageClient } from "vscode-languageclient/node";
import { createClient } from "./client";
import { registerCommands } from "./commands";
import { currentEnvironment, notFoundMessage, resolveExecutable } from "./executable";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel("Grenat");
  context.subscriptions.push(output);

  registerCommands(context, () => findExecutable(output));
  context.subscriptions.push(
    vscode.commands.registerCommand("grenat.restartServer", () => restart(output)),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("grenat.path")) void restart(output);
    }),
  );

  await startServer(output);
}

export async function deactivate(): Promise<void> {
  await stopServer();
}

function setting(): string {
  return vscode.workspace.getConfiguration("grenat").get<string>("path", "grenat");
}

/** The `grenat` executable, or `undefined` after telling the user. */
function findExecutable(output: vscode.OutputChannel): string | undefined {
  const found = resolveExecutable(setting(), currentEnvironment());
  if (!found) {
    const message = notFoundMessage(setting());
    output.appendLine(message);
    void vscode.window.showErrorMessage(message, "Open Settings").then((choice) => {
      if (choice) void vscode.commands.executeCommand("workbench.action.openSettings", "grenat.path");
    });
  }
  return found;
}

async function startServer(output: vscode.OutputChannel): Promise<void> {
  const executable = findExecutable(output);
  if (!executable) return;
  output.appendLine(`Starting ${executable} lsp`);
  client = createClient(executable, output);
  try {
    await client.start();
  } catch (error) {
    output.appendLine(`The language server did not start: ${String(error)}`);
    void vscode.window.showErrorMessage(`Grenat: \`${executable} lsp\` did not start (see the Grenat output).`);
    client = undefined;
  }
}

async function stopServer(): Promise<void> {
  const running = client;
  client = undefined;
  if (running) await running.stop();
}

async function restart(output: vscode.OutputChannel): Promise<void> {
  await stopServer();
  await startServer(output);
}
