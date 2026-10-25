import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { test } from "node:test";

const file = path.join(__dirname, "..", "..", "snippets", "grenat.code-snippets");
const snippets: Record<string, { prefix: string; body: string[]; description: string }> = JSON.parse(
  fs.readFileSync(file, "utf8"),
);

test("the snippets cover the main declarations", () => {
  const prefixes = Object.values(snippets).map((s) => s.prefix);
  for (const wanted of ["agent", "prompt", "tool", "workflow", "test", "struct", "on_webhook", "every"]) {
    assert.ok(prefixes.includes(wanted), `a snippet for ${wanted}`);
  }
});

test("each snippet has a description and a body closed by end", () => {
  for (const [name, snippet] of Object.entries(snippets)) {
    assert.ok(snippet.description.length > 0, `${name} has a description`);
    assert.equal(snippet.body.at(-1), "end", `${name} ends with end`);
  }
});
