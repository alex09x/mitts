/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import { execFile } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as vscode from "vscode";
import { installCommand } from "./install";
import { atLeast, locateMitts, parseVersion } from "./locate";
import {
  RUST_ANALYZER_KEYS,
  nextSaved,
  replacedKeys,
  restorePlan,
  rustAnalyzerSettings,
  type Saved,
} from "./rustAnalyzer";

/** Where the values from before routing was turned on are kept (per workspace, survives reloads). */
const SAVED_KEY = "mitts.rustAnalyzer.saved";

function workspaceValues(config: vscode.WorkspaceConfiguration): Record<string, unknown> {
  const values: Record<string, unknown> = {};
  for (const key of RUST_ANALYZER_KEYS) {
    values[key] = config.inspect(key)?.workspaceValue;
  }
  return values;
}
import { MittsTaskProvider, TASK_TYPE } from "./tasks";

/** The first release with every Cargo command; older ones still work for build/check/test. */
const RECOMMENDED = "0.4.0";

function findExecutable(): string | undefined {
  const configured = vscode.workspace.getConfiguration("mitts").get<string>("path", "");
  return locateMitts(configured, {
    platform: process.platform,
    pathVar: process.env.PATH,
    home: os.homedir(),
    cargoHome: process.env.CARGO_HOME,
    isFile: (p) => {
      try {
        return fs.statSync(p).isFile();
      } catch {
        return false;
      }
    },
  });
}

function versionOf(executable: string): Promise<string | undefined> {
  return new Promise((resolve) => {
    execFile(executable, ["--version"], { timeout: 10_000 }, (error, stdout) => {
      resolve(error ? undefined : parseVersion(stdout));
    });
  });
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 0);
  status.command = "mitts.showVersion";
  context.subscriptions.push(status);

  const refresh = async (): Promise<void> => {
    const exe = findExecutable();
    if (!exe) {
      status.text = "$(warning) Mitts";
      status.tooltip = "Mitts not found. Run “Mitts: Install Mitts” or set mitts.path.";
      status.command = "mitts.install";
      status.show();
      return;
    }
    const version = await versionOf(exe);
    status.text = version ? `$(package) Mitts ${version}` : "$(warning) Mitts";
    status.tooltip = version
      ? atLeast(version, RECOMMENDED)
        ? exe
        : `${exe}\nMitts ${RECOMMENDED} or newer adds every Cargo command.`
      : `${exe} did not answer --version`;
    status.command = "mitts.showVersion";
    status.show();
  };

  context.subscriptions.push(
    vscode.tasks.registerTaskProvider(
      TASK_TYPE,
      new MittsTaskProvider(findExecutable, () =>
        vscode.workspace.getConfiguration("mitts").get<string[]>("extraArgs", []),
      ),
    ),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("mitts")) {
        void refresh();
      }
    }),
    vscode.commands.registerCommand("mitts.showVersion", async () => {
      const exe = findExecutable();
      if (!exe) {
        void vscode.commands.executeCommand("mitts.install");
        return;
      }
      const version = await versionOf(exe);
      void vscode.window.showInformationMessage(
        version ? `Mitts ${version} at ${exe}` : `${exe} did not answer --version`,
      );
    }),
    vscode.commands.registerCommand("mitts.install", async () => {
      const line = installCommand(process.platform);
      const choice = await vscode.window.showInformationMessage(
        `Install Mitts with the official installer? It runs in a terminal: ${line}`,
        { modal: true },
        "Run installer",
      );
      if (choice !== "Run installer") {
        return;
      }
      const terminal = vscode.window.createTerminal({ name: "Install Mitts" });
      terminal.show();
      terminal.sendText(line);
      const done = vscode.window.onDidCloseTerminal((closed) => {
        if (closed === terminal) {
          done.dispose();
          void refresh();
        }
      });
      context.subscriptions.push(done);
    }),
    vscode.commands.registerCommand("mitts.useForRustAnalyzer", async () => {
      const exe = findExecutable();
      if (!exe) {
        void vscode.window.showWarningMessage("Mitts not found. Install it first.");
        return;
      }
      if (!vscode.extensions.getExtension("rust-lang.rust-analyzer")) {
        void vscode.window.showWarningMessage(
          "The rust-analyzer extension is not installed, so there is nothing to route through Mitts.",
        );
        return;
      }
      const config = vscode.workspace.getConfiguration("rust-analyzer");
      const ours = rustAnalyzerSettings(exe);
      const current = workspaceValues(config);
      const replaced = replacedKeys(current, ours);
      if (replaced.length > 0) {
        const choice = await vscode.window.showWarningMessage(
          `This workspace already sets ${replaced.map((k) => `rust-analyzer.${k}`).join(" and ")}. ` +
            "Replace with Mitts? “Stop using Mitts for rust-analyzer” puts your values back.",
          { modal: true },
          "Replace",
        );
        if (choice !== "Replace") {
          return;
        }
        // The confirmation covered the values it showed; if they changed meanwhile, stop.
        const now = vscode.workspace.getConfiguration("rust-analyzer");
        if (JSON.stringify(workspaceValues(now)) !== JSON.stringify(current)) {
          void vscode.window.showWarningMessage(
            "The rust-analyzer settings changed while you were deciding. Nothing was changed; run the command again.",
          );
          return;
        }
      }
      // Per key: a key still on Mitts' last value keeps its original previous value.
      const saved = nextSaved(current, ours, context.workspaceState.get<Saved>(SAVED_KEY));
      await context.workspaceState.update(SAVED_KEY, saved);
      for (const [key, value] of Object.entries(ours)) {
        await config.update(key, value, vscode.ConfigurationTarget.Workspace);
      }
      void vscode.window.showInformationMessage(
        "rust-analyzer now runs its checks and runnables through Mitts in this workspace. " +
          "It still loads the project with cargo metadata.",
      );
    }),
    vscode.commands.registerCommand("mitts.stopUsingForRustAnalyzer", async () => {
      const config = vscode.workspace.getConfiguration("rust-analyzer");
      const plan = restorePlan(workspaceValues(config), context.workspaceState.get<Saved>(SAVED_KEY));
      for (const [key, value] of Object.entries(plan)) {
        await config.update(key, value, vscode.ConfigurationTarget.Workspace);
      }
      await context.workspaceState.update(SAVED_KEY, undefined);
      const kept = RUST_ANALYZER_KEYS.filter((key) => !(key in plan));
      void vscode.window.showInformationMessage(
        kept.length === 0
          ? "rust-analyzer settings in this workspace are back to what they were before Mitts."
          : `Restored what Mitts had set. Left as you changed it: ${kept
              .map((k) => `rust-analyzer.${k}`)
              .join(", ")}.`,
      );
    }),
  );

  await refresh();
}

export function deactivate(): void {}
