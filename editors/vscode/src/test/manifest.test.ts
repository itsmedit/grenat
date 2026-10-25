import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { test } from "node:test";

const root = path.join(__dirname, "..", "..");
const manifest = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const cargo = path.join(root, "..", "..", "Cargo.toml");

test("the extension's version is the workspace's", { skip: fs.existsSync(cargo) ? false : "no Cargo.toml" }, () => {
  const version = /\[workspace\.package\][^[]*?version\s*=\s*"([^"]+)"/.exec(fs.readFileSync(cargo, "utf8"))?.[1];
  assert.equal(manifest.version, version);
});

test("every command is contributed and has a title", () => {
  const ids = manifest.contributes.commands.map((c: { command: string }) => c.command);
  assert.deepEqual(ids.sort(), ["grenat.checkWorkspace", "grenat.restartServer", "grenat.runFile", "grenat.testFile"]);
});

test("format on save is on for Grenat, through this extension's formatter", () => {
  const defaults = manifest.contributes.configurationDefaults["[grenat]"];
  assert.equal(defaults["editor.formatOnSave"], true);
  assert.equal(defaults["editor.defaultFormatter"], `${manifest.publisher}.${manifest.name}`);
});
