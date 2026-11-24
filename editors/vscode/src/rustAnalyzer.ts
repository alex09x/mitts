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

/** What turning routing on saved: the values it wrote and the workspace values it replaced. */
export interface Saved {
  written: Record<string, unknown>;
  previous: Record<string, unknown>;
}

function same(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * The keys whose own workspace value turning routing on would replace (a value that is
 * already Mitts' counts as none), so the user is asked first.
 */
export function replacedKeys(
  current: Record<string, unknown>,
  ours: Record<string, unknown>,
): string[] {
  return Object.keys(ours).filter(
    (key) => current[key] !== undefined && !same(current[key], ours[key]),
  );
}

/**
 * What turning routing off writes for each key: the value from before routing was turned
 * on (`undefined` = no workspace value). A key the user changed since is left as it is.
 */
export function restorePlan(
  current: Record<string, unknown>,
  saved: Saved | undefined,
): Record<string, unknown> {
  const plan: Record<string, unknown> = {};
  for (const key of RUST_ANALYZER_KEYS) {
    if (!saved) {
      // Nothing recorded (routing set up by hand or by an older version): remove only a
      // value that is Mitts' own.
      const value = current[key];
      const isMitts =
        (typeof value === "string" && /(^|\/)mitts$/.test(value)) ||
        (Array.isArray(value) && typeof value[0] === "string" && /(^|\/)mitts$/.test(value[0]));
      if (isMitts) {
        plan[key] = undefined;
      }
      continue;
    }
    if (same(current[key], saved.written[key])) {
      plan[key] = saved.previous[key];
    }
  }
  return plan;
}
