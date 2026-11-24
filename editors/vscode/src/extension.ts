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
import { atLeast, locateMitts, parseVersion } from "./locate";
import { RUST_ANALYZER_KEYS, rustAnalyzerSettings } from "./rustAnalyzer";
import { MittsTaskProvider, TASK_TYPE } from "./tasks";

/** The first release with every Cargo command; older ones still work for build/check/test. */
const RECOMMENDED = "0.4.0";
const INSTALL_UNIX = "curl -fsSL https://mittsbuild.com/install.sh | sh";

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
      if (process.platform === "win32") {
        void vscode.window.showWarningMessage(
          "Mitts does not support Windows yet. See https://mittsbuild.com for the platforms it runs on.",
        );
        return;
      }
      const choice = await vscode.window.showInformationMessage(
        `Install Mitts with the official installer? It runs in a terminal: ${INSTALL_UNIX}`,
        { modal: true },
        "Run installer",
      );
      if (choice !== "Run installer") {
        return;
      }
      const terminal = vscode.window.createTerminal({ name: "Install Mitts" });
      terminal.show();
      terminal.sendText(INSTALL_UNIX);
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
      for (const [key, value] of Object.entries(rustAnalyzerSettings(exe))) {
        await config.update(key, value, vscode.ConfigurationTarget.Workspace);
      }
      void vscode.window.showInformationMessage(
        "rust-analyzer now runs its checks and runnables through Mitts in this workspace. " +
          "It still loads the project with cargo metadata.",
      );
    }),
    vscode.commands.registerCommand("mitts.stopUsingForRustAnalyzer", async () => {
      const config = vscode.workspace.getConfiguration("rust-analyzer");
      for (const key of RUST_ANALYZER_KEYS) {
        await config.update(key, undefined, vscode.ConfigurationTarget.Workspace);
      }
      void vscode.window.showInformationMessage("rust-analyzer uses Cargo again in this workspace.");
    }),
  );

  await refresh();
}

export function deactivate(): void {}
