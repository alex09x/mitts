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
 * The rust-analyzer settings that route its on-save check and its runnables through Mitts.
 * rust-analyzer still loads the project with `cargo metadata` and runs build scripts with
 * Cargo; those are left alone.
 */
export function rustAnalyzerSettings(executable: string): Record<string, unknown> {
  return {
    "check.overrideCommand": [
      executable,
      "check",
      "--workspace",
      "--all-targets",
      "--message-format=json",
    ],
    "runnables.command": executable,
  };
}

/** The keys the extension writes, so that turning it off removes exactly those. */
export const RUST_ANALYZER_KEYS = Object.keys(rustAnalyzerSettings("mitts"));
