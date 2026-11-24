/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import test from "node:test";

interface Pattern {
  regexp: string;
}

const manifest = JSON.parse(
  fs.readFileSync(path.join(__dirname, "..", "..", "..", "package.json"), "utf8"),
);
const [head, location] = manifest.contributes.problemMatchers[0].pattern as Pattern[];

/** Runs the two-line matcher over output the way VS Code does: a head line, then `-->`. */
function problems(output: string) {
  const found: Array<{ severity: string; code?: string; message: string; file: string; line: number; column: number }> = [];
  const lines = output.split("\n");
  for (let i = 0; i + 1 < lines.length; i++) {
    const h = new RegExp(head.regexp).exec(lines[i]);
    const l = h && new RegExp(location.regexp).exec(lines[i + 1]);
    if (h && l) {
      found.push({
        severity: h[1],
        code: h[2],
        message: h[3],
        file: l[1],
        line: Number(l[2]),
        column: Number(l[3]),
      });
    }
  }
  return found;
}

// Captured from `mitts check` on a workspace with one member (paths are relative to the
// workspace root, as Cargo prints them).
const MITTS_OUTPUT = `⚡ Mitts Build check [dev] ──────────────────────────────────────────
warning: unused variable: \`unused\`
 --> crates/a/src/lib.rs:1:35
  |
1 | pub fn f() { let x: i32 = "s"; let unused = 1; }
  |                                    ^^^^^^ help: if this is intentional, prefix it with an underscore: \`_unused\`
  |
  = note: \`#[warn(unused_variables)]\` on by default

error[E0308]: mismatched types
 --> crates/a/src/lib.rs:1:27
  |
1 | pub fn f() { let x: i32 = "s"; }
  |                     ---   ^^^ expected \`i32\`, found \`&str\`

error: aborting due to 1 previous error

  × Failed to compile a v0.1.0 [lib:a]
   ╭─[/abs/ws/crates/a/src/lib.rs:1:27]
`;

test("the problem matcher reads rustc diagnostics from Mitts output", () => {
  assert.deepEqual(problems(MITTS_OUTPUT), [
    {
      severity: "warning",
      code: undefined,
      message: "unused variable: `unused`",
      file: "crates/a/src/lib.rs",
      line: 1,
      column: 35,
    },
    {
      severity: "error",
      code: "E0308",
      message: "mismatched types",
      file: "crates/a/src/lib.rs",
      line: 1,
      column: 27,
    },
  ]);
});

test("the matcher is the one tasks reference", () => {
  assert.equal(manifest.contributes.problemMatchers[0].name, "mitts");
  const source = fs.readFileSync(path.join(__dirname, "..", "..", "..", "src", "tasks.ts"), "utf8");
  assert.ok(source.includes('["$mitts"]'));
});
