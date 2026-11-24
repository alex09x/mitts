/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

// Runs inside VS Code. Each step prints what it saw, so the log is the evidence.
import assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

function log(message: string): void {
  console.log(`[mitts-it] ${message}`);
}

async function until<T>(what: string, probe: () => T | undefined, ms = 240_000): Promise<T> {
  const start = Date.now();
  for (;;) {
    const value = probe();
    if (value !== undefined) {
      return value;
    }
    if (Date.now() - start > ms) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await new Promise((r) => setTimeout(r, 500));
  }
}

async function runTask(task: vscode.Task): Promise<number | undefined> {
  const ended = new Promise<number | undefined>((resolve) => {
    const sub = vscode.tasks.onDidEndTaskProcess((event) => {
      if (event.execution.task.name === task.name) {
        sub.dispose();
        resolve(event.exitCode);
      }
    });
  });
  await vscode.tasks.executeTask(task);
  return ended;
}

function mittsDiagnostics(file: vscode.Uri): vscode.Diagnostic[] {
  return vscode.languages.getDiagnostics(file).filter((d) => d.source === "mitts");
}

export async function run(): Promise<void> {
  const folder = vscode.workspace.workspaceFolders?.[0];
  assert.ok(folder, "the fixture project is open");
  const main = vscode.Uri.joinPath(folder.uri, "src", "main.rs");

  const extension = vscode.extensions.getExtension("alex09x.mitts");
  assert.ok(extension, "the extension is installed");
  await extension.activate();
  log(`extension active: ${extension.isActive}`);

  // 1. Tasks: every default task is offered, and `check` puts the compiler error on the file.
  const tasks = await vscode.tasks.fetchTasks({ type: "mitts" });
  const names = tasks.map((t) => t.name).sort();
  log(`tasks: ${names.join(", ")}`);
  for (const name of ["build", "check", "clippy", "test", "bench", "run", "doc", "clean"]) {
    assert.ok(names.includes(`mitts ${name}`), `task mitts ${name} is offered`);
  }
  const check = tasks.find((t) => t.name === "mitts check")!;
  const exitCode = await runTask(check);
  log(`mitts check exit code: ${exitCode}`);
  assert.notEqual(exitCode, 0, "check fails on the broken fixture");
  const fromTask = await until("task diagnostics", () => {
    const found = mittsDiagnostics(main).find((d) => String(d.code) === "E0308");
    return found;
  }, 30_000);
  log(`task diagnostic: ${fromTask.range.start.line + 1}:${fromTask.range.start.character + 1} ${fromTask.message}`);
  assert.equal(fromTask.severity, vscode.DiagnosticSeverity.Error);
  assert.equal(fromTask.range.start.line, 1);

  if (process.env.MITTS_IT_RUST_ANALYZER === "0") {
    log("rust-analyzer part skipped (MITTS_IT_RUST_ANALYZER=0)");
    log("ALL PASSED");
    return;
  }

  // 2. rust-analyzer: the command writes the two settings, and its on-save check then runs
  //    Mitts — Mitts' cache directory, unused until now by rust-analyzer, gets its files.
  await vscode.commands.executeCommand("mitts.useForRustAnalyzer");
  const ra = vscode.workspace.getConfiguration("rust-analyzer");
  const override = ra.get<string[]>("check.overrideCommand");
  log(`rust-analyzer.check.overrideCommand = ${JSON.stringify(override)}`);
  assert.equal(override?.[1], "check");
  assert.ok(override?.[0].endsWith("mitts"));
  assert.ok(ra.get<string>("runnables.command")?.endsWith("mitts"));

  const cache = process.env.MITTS_TEST_CACHE!;
  // Remove what the task built, so the incremental directory below can only come from
  // rust-analyzer's check.
  fs.rmSync(path.join(folder.uri.fsPath, "target"), { recursive: true, force: true });
  const before = fs.existsSync(cache) ? fs.readdirSync(cache, { recursive: true }).length : 0;
  const document = await vscode.workspace.openTextDocument(main);
  const editor = await vscode.window.showTextDocument(document);
  await editor.edit((e) => e.insert(new vscode.Position(0, 0), "// probe\n"));
  await document.save();
  const fromRa = await until(
    "rust-analyzer flycheck through Mitts",
    () => {
      const diagnostic = vscode.languages
        .getDiagnostics(main)
        // rust-analyzer's own analysis says "rust-analyzer"; its check command's output says "rustc".
        .find((d) => String(typeof d.code === "object" ? d.code.value : d.code) === "E0308" && d.source === "rustc");
      const incremental = path.join(folder.uri.fsPath, "target", "debug", "mitts-incremental");
      return diagnostic && fs.existsSync(incremental) ? diagnostic : undefined;
    },
  );
  const after = fs.readdirSync(cache, { recursive: true }).length;
  log(`rust-analyzer diagnostic (${fromRa.source}): line ${fromRa.range.start.line + 1} ${fromRa.message}`);
  log(`mitts cache entries before/after the rust-analyzer check: ${before}/${after}`);
  assert.ok(after > before, "rust-analyzer's check went through Mitts");

  // 3. Turning it off removes exactly what was written.
  await vscode.commands.executeCommand("mitts.stopUsingForRustAnalyzer");
  const cleared = vscode.workspace.getConfiguration("rust-analyzer");
  assert.equal(cleared.inspect("check.overrideCommand")?.workspaceValue, undefined);
  assert.equal(cleared.inspect("runnables.command")?.workspaceValue, undefined);
  log("rust-analyzer settings removed");
  log("ALL PASSED");
}
