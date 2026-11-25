/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

// Starts a real VS Code with this extension and rust-analyzer, on a copy of the fixture
// project, and runs src/test/integration in it. Needs `mitts` and a Rust toolchain on PATH.
import { spawnSync } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import {
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
  runTests,
} from "@vscode/test-electron";

async function main(): Promise<void> {
  const root = path.resolve(__dirname, "..", "..");
  const work = fs.mkdtempSync(path.join(os.tmpdir(), "mitts-vscode-"));
  const project = path.join(work, "project");
  fs.cpSync(path.join(root, "fixtures", "broken"), project, { recursive: true });
  const cache = path.join(work, "mitts-cache");

  const vscodeExe = await downloadAndUnzipVSCode(process.env.VSCODE_VERSION ?? "stable");
  const [cli, ...cliArgs] = resolveCliArgsFromVSCodeExecutablePath(vscodeExe);
  const extensionsDir = path.join(work, "extensions");
  const shell = process.platform === "win32";

  // MITTS_IT_VSIX: the packaged extension installs into this VS Code the way a user's does
  // (`code --install-extension`), and VS Code then lists it with the package's version.
  const vsix = process.env.MITTS_IT_VSIX;
  if (vsix) {
    const installed = path.join(work, "vsix-extensions");
    const cliRun = (args: string[]) =>
      spawnSync(cli, [...cliArgs, "--extensions-dir", installed, ...args], { encoding: "utf8", shell });
    const add = cliRun(["--install-extension", path.resolve(vsix)]);
    console.log(add.stdout, add.stderr);
    if (add.status !== 0) {
      throw new Error(`code --install-extension ${vsix} failed`);
    }
    const listed = cliRun(["--list-extensions", "--show-versions"]).stdout;
    const version = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")).version;
    console.log(`[mitts-it] installed extensions: ${listed.trim()}`);
    if (!listed.split(/\r?\n/).includes(`alex09x.mitts@${version}`)) {
      throw new Error(`alex09x.mitts@${version} is not listed after installing the VSIX`);
    }
  }
  // rust-analyzer loads projects with `cargo metadata`; the repository's CI does not launch
  // Cargo, so it runs this test with MITTS_IT_RUST_ANALYZER=0 (tasks only). The
  // clean-machine check runs the rust-analyzer part.
  const withRustAnalyzer = process.env.MITTS_IT_RUST_ANALYZER !== "0";
  const install = !withRustAnalyzer ? { status: 0 } : spawnSync(
    cli,
    [...cliArgs, "--extensions-dir", extensionsDir, "--install-extension", "rust-lang.rust-analyzer"],
    { stdio: "inherit", shell },
  );
  if (install.status !== 0) {
    throw new Error("could not install rust-analyzer into the test VS Code");
  }

  await runTests({
    vscodeExecutablePath: vscodeExe,
    extensionDevelopmentPath: root,
    extensionTestsPath: path.join(__dirname, "integration", "index"),
    launchArgs: [project, "--extensions-dir", extensionsDir, "--disable-workspace-trust"],
    extensionTestsEnv: {
      MITTS_CACHE_DIR: cache,
      MITTS_TEST_CACHE: cache,
      MITTS_IT_RUST_ANALYZER: withRustAnalyzer ? "1" : "0",
    },
  });
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
