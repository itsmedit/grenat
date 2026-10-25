// The language server the client launches: `grenat lsp` answers `initialize`
// with formatting (what format on save uses) and formats a document. Skipped
// when no `grenat` is found (GRENAT_PATH, else PATH).

import * as assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { test } from "node:test";
import { currentEnvironment, resolveExecutable } from "../executable";

const executable = resolveExecutable(process.env.GRENAT_PATH ?? "grenat", currentEnvironment());

function frame(message: object): string {
  const body = JSON.stringify({ jsonrpc: "2.0", ...message });
  return `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`;
}

/** Sends the messages and collects the responses until `count` have come. */
function exchange(messages: object[], count: number): Promise<Array<{ id?: number; result?: unknown }>> {
  return new Promise((resolve, reject) => {
    const server = spawn(executable as string, ["lsp"], { stdio: ["pipe", "pipe", "inherit"] });
    const responses: Array<{ id?: number; result?: unknown }> = [];
    let buffer = Buffer.alloc(0);
    const timer = setTimeout(() => {
      server.kill();
      reject(new Error("no answer from grenat lsp"));
    }, 20_000);
    server.on("error", reject);
    server.stdout.on("data", (chunk: Buffer) => {
      buffer = Buffer.concat([buffer, chunk]);
      for (;;) {
        const header = buffer.indexOf("\r\n\r\n");
        if (header < 0) return;
        const length = Number(/Content-Length: (\d+)/i.exec(buffer.subarray(0, header).toString())?.[1]);
        if (buffer.length < header + 4 + length) return;
        const message = JSON.parse(buffer.subarray(header + 4, header + 4 + length).toString());
        buffer = buffer.subarray(header + 4 + length);
        if ("id" in message && !("method" in message)) responses.push(message);
        if (responses.length === count) {
          clearTimeout(timer);
          server.stdin.end(frame({ method: "exit" }));
          resolve(responses);
        }
      }
    });
    server.stdin.write(messages.map(frame).join(""));
  });
}

test("grenat lsp offers formatting and formats", { skip: executable ? false : "grenat not found" }, async () => {
  const uri = "file:///tmp/sample.grn";
  const [init, formatted] = await exchange(
    [
      { id: 1, method: "initialize", params: { processId: null, rootUri: null, capabilities: {} } },
      { method: "initialized", params: {} },
      {
        method: "textDocument/didOpen",
        params: { textDocument: { uri, languageId: "grenat", version: 1, text: "def main\nputs   1\nend\n" } },
      },
      { id: 2, method: "textDocument/formatting", params: { textDocument: { uri }, options: { tabSize: 2, insertSpaces: true } } },
    ],
    2,
  );
  const capabilities = (init.result as { capabilities: Record<string, unknown> }).capabilities;
  assert.equal(capabilities.documentFormattingProvider, true);
  const edits = formatted.result as Array<{ newText: string }>;
  assert.ok(Array.isArray(edits) && edits.length > 0, "formatting returns edits");
  assert.match(edits.map((e) => e.newText).join(""), /^def main\n {2}puts 1\nend\n$/);
});
