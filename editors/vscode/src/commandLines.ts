// The command lines of the extension's commands. Free of the `vscode`
// module, so that they are tested on its own.

export type Action = "run" | "test" | "check";

export interface CommandLine {
  /** The arguments given to `grenat`. */
  args: string[];
  /** The directory the command runs in. */
  cwd: string;
  /** The terminal's title. */
  title: string;
}

/**
 * `grenat run file`, `grenat test file` (the file relative to the folder it
 * runs in when inside it), or `grenat check` over the whole folder.
 */
export function commandLine(action: Action, folder: string, file?: string): CommandLine {
  if (action === "check") {
    return { args: ["check"], cwd: folder, title: "Grenat: check" };
  }
  if (!file) throw new Error(`grenat ${action} needs a file`);
  const target = relativeTo(folder, file);
  return { args: [action, target], cwd: folder, title: `Grenat: ${action} ${baseName(file)}` };
}

function relativeTo(folder: string, file: string): string {
  const prefix = folder.endsWith("/") || folder.endsWith("\\") ? folder : folder + sep(folder);
  return file.startsWith(prefix) ? file.slice(prefix.length) : file;
}

function sep(folder: string): string {
  return folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
}

function baseName(file: string): string {
  return file.split(/[\\/]/).pop() ?? file;
}
