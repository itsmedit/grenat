import * as assert from "node:assert/strict";
import { test } from "node:test";
import { commandLine } from "../commandLines";

test("run and test take the file relative to the folder", () => {
  assert.deepEqual(commandLine("run", "/work/desk", "/work/desk/src/app.grn"), {
    args: ["run", "src/app.grn"],
    cwd: "/work/desk",
    title: "Grenat: run app.grn",
  });
  assert.deepEqual(commandLine("test", "/work/desk/", "/work/desk/tests/app_test.grn").args, ["test", "tests/app_test.grn"]);
});

test("a file outside the folder keeps its absolute path", () => {
  assert.deepEqual(commandLine("run", "/work/desk", "/tmp/scratch.grn").args, ["run", "/tmp/scratch.grn"]);
});

test("Windows paths", () => {
  assert.deepEqual(commandLine("test", "C:\\desk", "C:\\desk\\tests\\a_test.grn").args, ["test", "tests\\a_test.grn"]);
});

test("check runs over the whole folder", () => {
  assert.deepEqual(commandLine("check", "/work/desk"), { args: ["check"], cwd: "/work/desk", title: "Grenat: check" });
});

test("run without a file is an error", () => {
  assert.throws(() => commandLine("run", "/work/desk"), /needs a file/);
});
