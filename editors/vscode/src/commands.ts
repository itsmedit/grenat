// "Grenat: Run File", "Grenat: Test File", "Grenat: Check Workspace": the
// command runs in a terminal of its own, `grenat` started directly (no shell,
// so no quoting).

import * as path from "node:path";
import * as vscode from "vscode";
import { Action, commandLine } from "./commandLines";

export type ExecutableLookup = () => string | undefined;

export function registerCommands(context: vscode.ExtensionContext, executable: ExecutableLookup): void {
  const register = (id: string, action: Action) =>
    context.subscriptions.push(vscode.commands.registerCommand(id, (uri?: vscode.Uri) => runAction(action, executable, uri)));
  register("grenat.runFile", "run");
  register("grenat.testFile", "test");
  register("grenat.checkWorkspace", "check");
}

async function runAction(action: Action, executable: ExecutableLookup, uri?: vscode.Uri): Promise<void> {
  const exe = executable();
  if (!exe) return;

  if (action === "check") {
    const folder = await pickFolder();
    if (!folder) {
      void vscode.window.showErrorMessage("Grenat: open a folder to check its files.");
      return;
    }
    await vscode.workspace.saveAll(false);
    start(exe, commandLine("check", folder.uri.fsPath));
    return;
  }

  const document = await documentFor(uri);
  if (!document || document.uri.scheme !== "file") {
    void vscode.window.showErrorMessage(`Grenat: open a saved .grn file to ${action} it.`);
    return;
  }
  if (document.isDirty) await document.save();
  const file = document.uri.fsPath;
  const folder = vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath ?? path.dirname(file);
  start(exe, commandLine(action, folder, file));
}

async function documentFor(uri?: vscode.Uri): Promise<vscode.TextDocument | undefined> {
  if (uri) return vscode.workspace.openTextDocument(uri);
  return vscode.window.activeTextEditor?.document;
}

async function pickFolder(): Promise<vscode.WorkspaceFolder | undefined> {
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (folders.length <= 1) return folders[0];
  const active = vscode.window.activeTextEditor?.document.uri;
  const current = active && vscode.workspace.getWorkspaceFolder(active);
  return current ?? vscode.window.showWorkspaceFolderPick({ placeHolder: "The folder to check" });
}

function start(exe: string, line: ReturnType<typeof commandLine>): void {
  const terminal = vscode.window.createTerminal({
    name: line.title,
    shellPath: exe,
    shellArgs: line.args,
    cwd: line.cwd,
  });
  terminal.show();
}
