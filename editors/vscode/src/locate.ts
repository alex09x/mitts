/*
 * Mitts Build for VS Code — editor integration for the Mitts Rust build tool
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

import * as path from "path";

/** What finding the executable may look at; tests pass fakes. */
export interface LocateEnv {
  platform: NodeJS.Platform;
  pathVar: string | undefined;
  home: string | undefined;
  cargoHome: string | undefined;
  isFile(p: string): boolean;
}

/**
 * Where `mitts` is: the configured path when set (it must exist; it is never second-guessed),
 * else the first match on PATH, else `$CARGO_HOME/bin` or `~/.cargo/bin`, where the
 * installer and crates.io installs put it. `undefined` when it is nowhere.
 */
export function locateMitts(configured: string, env: LocateEnv): string | undefined {
  const exe = env.platform === "win32" ? "mitts.exe" : "mitts";
  const trimmed = configured.trim();
  if (trimmed !== "") {
    const expanded =
      trimmed.startsWith("~/") && env.home ? path.join(env.home, trimmed.slice(2)) : trimmed;
    return env.isFile(expanded) ? expanded : undefined;
  }
  const sep = env.platform === "win32" ? ";" : ":";
  for (const dir of (env.pathVar ?? "").split(sep)) {
    if (dir === "") {
      continue;
    }
    const candidate = path.join(dir, exe);
    if (env.isFile(candidate)) {
      return candidate;
    }
  }
  const cargoBin = env.cargoHome
    ? path.join(env.cargoHome, "bin")
    : env.home
      ? path.join(env.home, ".cargo", "bin")
      : undefined;
  if (cargoBin) {
    const candidate = path.join(cargoBin, exe);
    if (env.isFile(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

/** `mitts 0.4.0 (abcdef12…)` -> `0.4.0`. */
export function parseVersion(output: string): string | undefined {
  const match = /^mitts\s+(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)/m.exec(output.trim());
  return match ? match[1] : undefined;
}

/** True when `version` is at least `minimum` (both `X.Y.Z`, pre-release ignored). */
export function atLeast(version: string, minimum: string): boolean {
  const parts = (v: string) => v.split("-")[0].split(".").map((n) => Number.parseInt(n, 10));
  const a = parts(version);
  const b = parts(minimum);
  for (let i = 0; i < 3; i++) {
    if ((a[i] ?? 0) !== (b[i] ?? 0)) {
      return (a[i] ?? 0) > (b[i] ?? 0);
    }
  }
  return true;
}
