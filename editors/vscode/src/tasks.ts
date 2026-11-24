/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import * as vscode from "vscode";

export const TASK_TYPE = "mitts";

/** The tasks offered for every folder with a `Cargo.toml`, and the group each belongs to. */
export const DEFAULT_TASKS: ReadonlyArray<{
  command: string;
  args: string[];
  group?: vscode.TaskGroup;
}> = [
  { command: "build", args: [], group: vscode.TaskGroup.Build },
  { command: "check", args: [], group: vscode.TaskGroup.Build },
  { command: "clippy", args: [], group: vscode.TaskGroup.Build },
  { command: "test", args: [], group: vscode.TaskGroup.Test },
  { command: "bench", args: [] },
  { command: "run", args: [] },
  { command: "doc", args: [] },
  { command: "clean", args: [], group: vscode.TaskGroup.Clean },
];

export interface MittsTaskDefinition extends vscode.TaskDefinition {
  type: "mitts";
  command: string;
  args?: string[];
  cwd?: string;
}

/** The full argument list for one task: the command, its own args, then `mitts.extraArgs`. */
export function taskArgs(definition: MittsTaskDefinition, extraArgs: readonly string[]): string[] {
  return [definition.command, ...(definition.args ?? []), ...extraArgs];
}

export function makeTask(
  definition: MittsTaskDefinition,
  folder: vscode.WorkspaceFolder,
  executable: string,
  extraArgs: readonly string[],
): vscode.Task {
  const execution = new vscode.ProcessExecution(executable, taskArgs(definition, extraArgs), {
    cwd: definition.cwd ?? folder.uri.fsPath,
  });
  const label = ["mitts", definition.command, ...(definition.args ?? [])].join(" ");
  const task = new vscode.Task(definition, folder, label, "mitts", execution, ["$mitts"]);
  return task;
}

export class MittsTaskProvider implements vscode.TaskProvider {
  constructor(
    private readonly executable: () => string | undefined,
    private readonly extraArgs: () => readonly string[],
  ) {}

  async provideTasks(): Promise<vscode.Task[]> {
    const exe = this.executable();
    if (!exe) {
      return [];
    }
    const tasks: vscode.Task[] = [];
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      const manifest = vscode.Uri.joinPath(folder.uri, "Cargo.toml");
      try {
        await vscode.workspace.fs.stat(manifest);
      } catch {
        continue;
      }
      for (const entry of DEFAULT_TASKS) {
        const definition: MittsTaskDefinition = {
          type: TASK_TYPE,
          command: entry.command,
          args: entry.args,
        };
        const task = makeTask(definition, folder, exe, this.extraArgs());
        if (entry.group) {
          task.group = entry.group;
        }
        tasks.push(task);
      }
    }
    return tasks;
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const exe = this.executable();
    const definition = task.definition as MittsTaskDefinition;
    if (!exe || !definition.command) {
      return undefined;
    }
    const folder =
      task.scope && typeof task.scope === "object" ? task.scope : vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      return undefined;
    }
    return makeTask(definition, folder, exe, this.extraArgs());
  }
}
