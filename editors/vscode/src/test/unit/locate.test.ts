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
import { atLeast, locateMitts, parseVersion } from "../../locate";
import { RUST_ANALYZER_KEYS, rustAnalyzerSettings } from "../../rustAnalyzer";

const files = (...existing: string[]) => (p: string) => existing.includes(p);
const base = { platform: "linux" as NodeJS.Platform, home: "/home/u", cargoHome: undefined };

test("a configured path wins and is never replaced by a PATH match", () => {
  const env = { ...base, pathVar: "/usr/bin", isFile: files("/usr/bin/mitts") };
  assert.equal(locateMitts("/opt/mitts", { ...env, isFile: files("/opt/mitts") }), "/opt/mitts");
  assert.equal(locateMitts("/opt/missing", env), undefined);
  assert.equal(
    locateMitts("~/bin/mitts", { ...env, isFile: files("/home/u/bin/mitts") }),
    "/home/u/bin/mitts",
  );
});

test("PATH is searched in order, then the Cargo bin directory", () => {
  const isFile = files("/b/mitts", "/c/mitts", "/home/u/.cargo/bin/mitts");
  assert.equal(locateMitts("", { ...base, pathVar: "/a::/b:/c", isFile }), "/b/mitts");
  assert.equal(
    locateMitts("", { ...base, pathVar: "/a", isFile }),
    "/home/u/.cargo/bin/mitts",
  );
  assert.equal(
    locateMitts("", {
      ...base,
      cargoHome: "/ch",
      pathVar: "/a",
      isFile: files("/ch/bin/mitts", "/home/u/.cargo/bin/mitts"),
    }),
    "/ch/bin/mitts",
  );
  assert.equal(locateMitts("", { ...base, pathVar: "", isFile: files() }), undefined);
});

test("versions are read from `mitts --version` and compared", () => {
  assert.equal(parseVersion("mitts 0.4.0 (ce77e476)\n"), "0.4.0");
  assert.equal(parseVersion("mitts 0.5.0-rc.1"), "0.5.0-rc.1");
  assert.equal(parseVersion("cargo 1.90.0"), undefined);
  assert.ok(atLeast("0.4.0", "0.4.0"));
  assert.ok(atLeast("0.10.0", "0.4.0"));
  assert.ok(!atLeast("0.3.39", "0.4.0"));
});

test("rust-analyzer settings name only the check command and runnables", () => {
  const settings = rustAnalyzerSettings("/x/mitts");
  assert.deepEqual(settings["check.overrideCommand"], [
    "/x/mitts",
    "check",
    "--workspace",
    "--all-targets",
    "--message-format=json",
  ]);
  assert.equal(settings["runnables.command"], "/x/mitts");
  assert.deepEqual(RUST_ANALYZER_KEYS, ["check.overrideCommand", "runnables.command"]);
});
