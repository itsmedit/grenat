// Finding the `grenat` executable: the `grenat.path` setting, a name looked
// up on PATH or a path to a file. Free of the `vscode` module, so that it is
// tested on its own.

import * as fs from "node:fs";
import * as path from "node:path";

export interface Environment {
  /** The PATH variable. */
  pathVariable: string;
  /** The PATHEXT variable on Windows (".EXE;.CMD;…"), empty elsewhere. */
  pathExtensions: string;
  /** The home directory, for a setting starting with `~/`. */
  home: string;
  /** Whether a file exists and can be executed. */
  isExecutable: (file: string) => boolean;
}

export function currentEnvironment(): Environment {
  return {
    pathVariable: process.env.PATH ?? "",
    pathExtensions: process.platform === "win32" ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT") : "",
    home: process.env.HOME ?? process.env.USERPROFILE ?? "",
    isExecutable,
  };
}

function isExecutable(file: string): boolean {
  try {
    if (!fs.statSync(file).isFile()) return false;
    fs.accessSync(file, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * The absolute path of the executable the setting names, or `undefined` when
 * there is none. A setting with a directory separator is a path; a bare name
 * is looked up on PATH.
 */
export function resolveExecutable(setting: string, env: Environment): string | undefined {
  const wanted = expandHome(setting.trim() || "grenat", env.home);
  const extensions = ["", ...env.pathExtensions.split(";").filter((e) => e.length > 0)];
  const candidates = (base: string) => extensions.map((ext) => base + ext);

  if (wanted.includes("/") || wanted.includes("\\")) {
    return candidates(path.resolve(wanted)).find(env.isExecutable);
  }
  for (const dir of env.pathVariable.split(path.delimiter)) {
    if (dir.length === 0) continue;
    const found = candidates(path.join(dir, wanted)).find(env.isExecutable);
    if (found) return found;
  }
  return undefined;
}

function expandHome(setting: string, home: string): string {
  return setting.startsWith("~/") && home ? path.join(home, setting.slice(2)) : setting;
}

/** The message shown when the executable cannot be found. */
export function notFoundMessage(setting: string): string {
  return (
    `Grenat: the executable \`${setting || "grenat"}\` was not found. ` +
    "Install Grenat (https://github.com/itsmedit/grenat#install) or set `grenat.path` to the `grenat` binary."
  );
}
