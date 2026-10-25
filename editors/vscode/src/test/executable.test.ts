import * as assert from "node:assert/strict";
import * as path from "node:path";
import { test } from "node:test";
import { Environment, notFoundMessage, resolveExecutable } from "../executable";

function environment(files: string[], overrides: Partial<Environment> = {}): Environment {
  const present = new Set(files.map((f) => path.resolve(f)));
  return {
    pathVariable: ["/usr/bin", "/opt/grenat/bin"].join(path.delimiter),
    pathExtensions: "",
    home: "/home/ada",
    isExecutable: (file) => present.has(path.resolve(file)),
    ...overrides,
  };
}

test("a bare name is looked up on PATH, in order", () => {
  const env = environment(["/opt/grenat/bin/grenat"]);
  assert.equal(resolveExecutable("grenat", env), path.join("/opt/grenat/bin", "grenat"));
});

test("an empty setting means grenat", () => {
  const env = environment(["/usr/bin/grenat"]);
  assert.equal(resolveExecutable("  ", env), path.join("/usr/bin", "grenat"));
});

test("a path is taken as it is, ~ expanded", () => {
  const env = environment(["/home/ada/.cargo/bin/grenat"]);
  assert.equal(resolveExecutable("~/.cargo/bin/grenat", env), path.resolve("/home/ada/.cargo/bin/grenat"));
  assert.equal(resolveExecutable("/elsewhere/grenat", env), undefined);
});

test("PATHEXT extensions are tried (Windows)", () => {
  const env = environment(["/usr/bin/grenat.EXE"], { pathExtensions: ".EXE;.CMD" });
  assert.equal(resolveExecutable("grenat", env), path.join("/usr/bin", "grenat.EXE"));
});

test("nothing found is undefined, with a message naming the setting", () => {
  assert.equal(resolveExecutable("grenat", environment([])), undefined);
  assert.match(notFoundMessage("/x/grenat"), /`\/x\/grenat` was not found.*grenat\.path/);
});
