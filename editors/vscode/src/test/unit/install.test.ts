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
import test from "node:test";
import { installCommand } from "../../install";

test("Windows gets the PowerShell installer, through powershell so cmd.exe runs it too", () => {
  const line = installCommand("win32");
  assert.match(line, /^powershell -NoProfile -ExecutionPolicy Bypass -Command "/);
  assert.match(line, /irm https:\/\/mittsbuild\.com\/install\.ps1 \| iex"$/);
});

test("macOS and Linux get the shell installer", () => {
  for (const platform of ["darwin", "linux"] as const) {
    assert.equal(installCommand(platform), "curl -fsSL https://mittsbuild.com/install.sh | sh");
  }
});
