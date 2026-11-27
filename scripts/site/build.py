# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Generate site/docs/, site/the-hundred/, site/worktrees/ and site/sitemap.xml, and fill the
scoreboard and header blocks of site/index.html.

    python3 scripts/site/build.py [--mitts PATH] [--bench-dir DIR]

Docs come from the installed binary's --help (so the option tables match its version) and from
docs_how.py / docs_commands.py. Benchmarks come from docs/data/benchmarks.json; pass --bench-dir
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
import build_hundred  # noqa: E402
import shell  # noqa: E402
import build_docs  # noqa: E402
import build_worktrees  # noqa: E402

SITE = HERE.parent.parent / "site"
PAGES = ["", "docs/", "worktrees/", "the-hundred/"]


def sitemap():
    urls = "".join("  <url><loc>%s%s</loc></url>\n" % (shell.SITE_URL, p) for p in PAGES)
    return ('<?xml version="1.0" encoding="UTF-8"?>\n'
            '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n%s</urlset>\n' % urls)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mitts", default="mitts")
    ap.add_argument("--bench-dir")
    args = ap.parse_args()
    data_file = HERE.parent.parent / "docs" / "data" / "benchmarks.json"
    if args.bench_dir:
        aggregate.main(args.bench_dir, str(data_file))
    version = subprocess.run([args.mitts, "--version"], capture_output=True, text=True, check=True).stdout.split()[1]
    (SITE / "docs").mkdir(parents=True, exist_ok=True)
    (SITE / "docs" / "index.html").write_text(build_docs.build(args.mitts, version), encoding="utf-8")
    data = json.loads(data_file.read_text())
    (SITE / "the-hundred").mkdir(parents=True, exist_ok=True)
    (SITE / "the-hundred" / "index.html").write_text(build_hundred.build(data), encoding="utf-8")
    (SITE / "worktrees").mkdir(parents=True, exist_ok=True)
    (SITE / "worktrees" / "index.html").write_text(build_worktrees.build(), encoding="utf-8")
    (SITE / "sitemap.xml").write_text(sitemap(), encoding="utf-8")
    home = SITE / "index.html"
    text = home.read_text(encoding="utf-8")
    home_meta = shell.meta("", "Mitts — a Rust build cache shared across git worktrees",
                           "A Rust build system with a machine-wide, content-addressed cache: git worktrees and "
                           "checkouts of a Cargo project reuse compiled dependencies instead of rebuilding them.").rstrip()
    for name, block in (("hundred", build_hundred.home_block(data)), ("header", shell.header(0)),
                        ("meta", home_meta)):
        start, end = "<!-- %s:start -->" % name, "<!-- %s:end -->" % name
        a, b = text.index(start) + len(start), text.index(end)
        text = text[:a] + "\n" + block + "\n  " + text[b:]
    home.write_text(text, encoding="utf-8")
    print("wrote docs/, the-hundred/, worktrees/, sitemap.xml and the home scoreboard for mitts", version)


if __name__ == "__main__":
    main()
