# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Generate site/docs/index.html and site/benchmarks/index.html.

    python3 scripts/site/build.py [--mitts PATH] [--bench-dir DIR]

Docs come from the installed binary's --help (so the option tables match its version) and from
docs_how.py / docs_commands.py. Benchmarks come from site/data/benchmarks.json; pass --bench-dir
(the directory holding the three platform reports) to regenerate that file first.
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import aggregate  # noqa: E402
import build_benchmarks  # noqa: E402
import build_docs  # noqa: E402

SITE = HERE.parent.parent / "site"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mitts", default="mitts")
    ap.add_argument("--bench-dir")
    args = ap.parse_args()
    data_file = SITE / "data" / "benchmarks.json"
    if args.bench_dir:
        aggregate.main(args.bench_dir, str(data_file))
    version = subprocess.run([args.mitts, "--version"], capture_output=True, text=True, check=True).stdout.split()[1]
    (SITE / "docs").mkdir(parents=True, exist_ok=True)
    (SITE / "docs" / "index.html").write_text(build_docs.build(args.mitts, version), encoding="utf-8")
    (SITE / "benchmarks").mkdir(parents=True, exist_ok=True)
    (SITE / "benchmarks" / "index.html").write_text(build_benchmarks.build(json.loads(data_file.read_text())), encoding="utf-8")
    print("wrote docs/index.html and benchmarks/index.html for mitts", version)


if __name__ == "__main__":
    main()
