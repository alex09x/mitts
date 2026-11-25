#!/usr/bin/env python3
# Mitts Build for Sublime Text — editor integration for the Mitts Rust build tool
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Checks Mitts.sublime-build outside Sublime Text.

- the file is JSON with the commands and variants it should have;
- its `file_regex` (Sublime applies it to each line of the build output) finds the
  location of every rustc error and warning in captured Mitts output, and nothing else;
- with MITTS_SUBLIME_E2E=1, the same on the output of a real `mitts check` (the build's
  own command, from the project folder) on a broken project.

Sublime itself is not started: its build panel and result navigation are not tested.
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
failures = []
# Mitts prints non-ASCII (⚡, box drawing); Windows' console encoding is not UTF-8.
sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def expect(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        failures.append(what)


build = json.loads((HERE / "Mitts.sublime-build").read_text(encoding="utf-8"))
file_regex = re.compile(build["file_regex"])


def locations(output: str):
    found = []
    for line in output.splitlines():
        m = file_regex.search(line)
        if m:
            found.append((m.group(1).replace("\\", "/"), int(m.group(2)), int(m.group(3))))
    return found


expect(build["cmd"] == ["mitts", "build"], "the default build is `mitts build`")
expect(build["selector"] == "source.rust" and build["keyfiles"] == ["Cargo.toml"],
       "chosen automatically for Rust files in a Cargo project")
expect(build["working_dir"] == "$folder", "runs from the project folder")
expect(
    {v["name"]: v["cmd"] for v in build["variants"]} == {
        "Check": ["mitts", "check"],
        "Test": ["mitts", "test"],
        "Run": ["mitts", "run"],
        "Clippy": ["mitts", "clippy"],
        "Build --release": ["mitts", "build", "--release"],
    },
    "variants",
)

captured = (HERE / "test" / "check.txt").read_text(encoding="utf-8")
expect(
    locations(captured) == [
        ("crates/a/src/main.rs", 2, 9),
        ("crates/a/src/lib.rs", 1, 27),
        ("crates/a/src/lib.rs", 3, 20),
    ],
    "captured output: one location per error and warning",
)
expect(
    locations("   ╭─[/abs/ws/crates/a/src/lib.rs:1:27]\nerror: aborting due to 2 previous errors\n")
    == [],
    "Mitts' own report and summaries give no location",
)

if os.environ.get("MITTS_SUBLIME_E2E") == "1":
    folder = HERE / "test" / "fixtures" / "broken"
    run = subprocess.run(
        [*build["variants"][0]["cmd"]], cwd=folder, capture_output=True, text=True,
        encoding="utf-8", errors="replace",
    )
    output = run.stdout + run.stderr
    expect(run.returncode != 0, "mitts check fails on the broken project")
    expect(locations(output) == [("src/main.rs", 2, 18)],
           f"real output: the type error's location\n{output}")
else:
    print("skip e2e (MITTS_SUBLIME_E2E is not 1)")

sys.exit(1 if failures else 0)
