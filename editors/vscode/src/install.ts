/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/**
 * The official installer, as a line typed into the editor's terminal. On Windows (0.5.0
 * and later) it is the PowerShell installer; started through `powershell -Command`, the
 * same line works whether the terminal runs PowerShell, pwsh or cmd.exe.
 */
export function installCommand(platform: NodeJS.Platform): string {
  if (platform === "win32") {
    return 'powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://mittsbuild.com/install.ps1 | iex"';
  }
  return "curl -fsSL https://mittsbuild.com/install.sh | sh";
}
