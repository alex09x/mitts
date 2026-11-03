# Mitts Build — reject new formatting debt while preserving an immutable release.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

BASELINE_SHA = "8a6ae03cde84d7fdaede0b43fb1ba9e12f221a9d"


def historical_differences(source, baseline, output, read_blob):
    if baseline["source_sha"] != BASELINE_SHA:
        raise ValueError("Formatting baseline must remain the published 0.3.18 source")
    paths = re.findall(r"^Diff in (.+):\d+:\s*$", output, re.MULTILINE)
    if not paths:
        raise ValueError("rustfmt failed without recognizable formatting differences")
    allowed = set()
    for value in paths:
        path = Path(value).resolve()
        relative = path.relative_to(source.resolve()).as_posix()
        expected = baseline["files"].get(relative)
        current = hashlib.sha256(path.read_bytes()).hexdigest()
        original = hashlib.sha256(read_blob(relative)).hexdigest() if expected else None
        if expected is None or current != expected or original != expected:
            raise ValueError("Formatting required for changed/new file: " + relative)
        allowed.add(relative)
    return allowed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, default=Path.cwd())
    args = parser.parse_args()
    result = subprocess.run(
        ["cargo", "fmt", "--all", "--", "--check", "--color", "never"],
        cwd=args.source,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode == 0:
        print("Rust formatting: all files conform")
        return
    if result.returncode != 1 or result.stderr.strip():
        raise SystemExit(result.stdout + result.stderr)
    baseline = json.loads(
        Path(__file__).with_name("formatting-baseline.json").read_text()
    )

    def read_blob(relative):
        return subprocess.check_output(
            ["git", "show", BASELINE_SHA + ":" + relative], cwd=args.source
        )

    try:
        allowed = historical_differences(
            args.source, baseline, result.stdout, read_blob
        )
    except (ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error) + "\n" + result.stdout) from error
    print(
        f"Rust formatting: {len(allowed)} unchanged 0.3.18 files retain historical formatting; no new debt"
    )


if __name__ == "__main__":
    main()
