#!/usr/bin/env python3
# Mitts Build for Zed — editor integration for the Mitts Rust build tool
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks tasks.json and settings.json outside Zed.

- both are JSON with comments (Zed's format) with the expected content: every task runs
  Mitts, uses only variables Zed's Rust support defines, and carries Zed's Rust tags;
- with MITTS_ZED_E2E=1, on a broken project: the workspace check task fails with the
  type error, and the command rust-analyzer gets from settings.json prints the
  error as a `compiler-message` JSON line, as `cargo check --message-format=json` does.

Zed itself is not started.
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
FIXTURE = HERE / "test" / "fixtures" / "broken"
failures = []
# Mitts prints non-ASCII (⚡, box drawing); Windows' console encoding is not UTF-8.
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

# Zed's own variables and the ones its Rust support adds (crates/languages/src/rust.rs).
KNOWN = {
    "ZED_DIRNAME", "ZED_STEM", "ZED_WORKTREE_ROOT", "ZED_FILE",
    "ZED_CUSTOM_RUST_PACKAGE", "ZED_CUSTOM_RUST_BIN_NAME", "ZED_CUSTOM_RUST_BIN_KIND",
    "ZED_CUSTOM_RUST_BIN_REQUIRED_FEATURES_FLAG", "ZED_CUSTOM_RUST_BIN_REQUIRED_FEATURES",
    "ZED_CUSTOM_RUST_TEST_FRAGMENT", "ZED_CUSTOM_RUST_DOC_TEST_NAME",
    "ZED_CUSTOM_RUST_TEST_NAME", "ZED_CUSTOM_RUST_MANIFEST_DIRNAME",
}
TAGS = {"rust-test", "rust-doc-test", "rust-mod-test", "rust-main"}
CHECK = ["mitts", "check", "--workspace", "--all-targets", "--message-format=json"]


def expect(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        failures.append(what)


def jsonc(path: Path):
    """JSON with // comments and trailing commas, as Zed reads it."""
    text = re.sub(r'("(?:\\.|[^"\\])*")|//[^\n]*', lambda m: m.group(1) or "",
                  path.read_text(encoding="utf-8"))
    text = re.sub(r",(\s*[}\]])", r"\1", text)
    return json.loads(text)


tasks = jsonc(HERE / "tasks.json")
settings = jsonc(HERE / "settings.json")

expect(all(t["command"] == "mitts" for t in tasks), "every task runs Mitts")
used = {v for t in tasks for s in [t["label"], t.get("cwd", ""), *t["args"]]
        for v in re.findall(r"\$(\w+)", s)}
expect(used <= KNOWN, f"only Zed's variables: unknown {sorted(used - KNOWN)}")
expect({tag for t in tasks for tag in t.get("tags", [])} == TAGS,
       "the run buttons of Zed's Rust tests, doc tests, test modules and mains")
ra = settings["lsp"]["rust-analyzer"]["initialization_options"]
expect(ra["check"]["overrideCommand"] == CHECK, "rust-analyzer checks through Mitts")
expect(ra["runnables"]["command"] == "mitts", "rust-analyzer runnables through Mitts")


def substitute(arg: str) -> str:
    values = {
        "ZED_DIRNAME": str(FIXTURE / "src"),
        "ZED_CUSTOM_RUST_PACKAGE": "broken",
        "ZED_CUSTOM_RUST_MANIFEST_DIRNAME": str(FIXTURE),
    }
    return re.sub(r"\$(\w+)", lambda m: values.get(m.group(1), m.group(0)), arg)


if os.environ.get("MITTS_ZED_E2E") == "1":
    task = next(t for t in tasks if t["label"] == "mitts check all targets (workspace)")
    run = subprocess.run([task["command"], *map(substitute, task["args"])],
                         cwd=substitute(task["cwd"]), capture_output=True, text=True,
                         encoding="utf-8", errors="replace")
    expect(run.returncode != 0 and "E0308" in run.stdout + run.stderr,
           "the workspace check task fails with the type error")

    run = subprocess.run(CHECK, cwd=FIXTURE, capture_output=True, text=True,
                         encoding="utf-8", errors="replace")
    messages = []
    for line in run.stdout.splitlines():
        try:
            messages.append(json.loads(line))
        except json.JSONDecodeError:
            failures.append(f"not JSON on stdout: {line!r}")
    errors = [m for m in messages if m.get("reason") == "compiler-message"
              and m["message"]["level"] == "error" and m["message"].get("code")]
    # --all-targets checks the binary and its test build; each reports the error, as
    # under Cargo.
    expect(bool(errors) and all(e["message"]["code"]["code"] == "E0308" for e in errors),
           f"rust-analyzer's command gives the error as JSON\n{run.stdout[:2000]}")
    expect({tuple(e["target"]["kind"]) for e in errors} == {("bin",), ("test",)},
           "for the binary and for its test build")
    spans = {(s["file_name"].replace("\\", "/"), s["line_start"], s["column_start"])
             for e in errors for s in e["message"]["spans"] if s["is_primary"]}
    expect(spans == {("src/main.rs", 2, 18)}, f"at src/main.rs:2:18: {sorted(spans)}")
    expect(any(m.get("reason") == "build-finished" for m in messages),
           "and ends with build-finished, as Cargo")
else:
    print("skip e2e (MITTS_ZED_E2E is not 1)")

sys.exit(1 if failures else 0)
