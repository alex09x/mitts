# Mitts Build — first-party LLVM LCOV summary for GitHub Actions.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
import json
from pathlib import Path


def summarize(lcov, source):
    source = Path(source).resolve()
    lines = {}
    files = set()
    retained = []
    for record in Path(lcov).read_text().split("end_of_record"):
        entries = record.strip().splitlines()
        names = [line[3:] for line in entries if line.startswith("SF:")]
        if len(names) != 1:
            continue
        path = Path(names[0])
        path = (path if path.is_absolute() else source / path).resolve()
        try:
            relative = path.relative_to(source)
        except ValueError:
            continue
        if not path.is_file() or (
            relative.parts[0] != "src" and relative != Path("build.rs")
        ):
            continue
        files.add(relative.as_posix())
        retained.append("\n".join(entries) + "\nend_of_record\n")
        for entry in entries:
            if entry.startswith("DA:"):
                number, count, *_ = entry[3:].split(",")
                number, count = int(number), int(count)
                if number < 1 or count < 0:
                    raise ValueError("Invalid LLVM line coverage counter")
                key = (relative.as_posix(), number)
                lines[key] = lines.get(key, False) or count > 0
    if not lines:
        raise ValueError("No first-party source coverage data was produced")
    covered = sum(lines.values())
    return {
        "scope": "src/**/*.rs and build.rs; unit and integration tests",
        "files": len(files),
        "lines": len(lines),
        "covered": covered,
        "percent": 100 * covered / len(lines),
    }, "".join(retained)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("lcov", type=Path)
    parser.add_argument("source", type=Path)
    parser.add_argument("summary", type=Path)
    args = parser.parse_args()
    result, filtered = summarize(args.lcov, args.source)
    args.lcov.with_name("first-party-lcov.info").write_text(filtered)
    args.lcov.with_name("summary.json").write_text(json.dumps(result, indent=2) + "\n")
    text = (
        f"### Rust line coverage: {result['percent']:.2f}%\n\n"
        f"{result['covered']} / {result['lines']} executable lines across {result['files']} first-party files.\n\n"
        "Scope: `src/**/*.rs` and `build.rs`, measured by unit/integration tests. "
        "Dependency and test-harness sources are excluded; documentation tests run separately.\n"
    )
    with args.summary.open("a") as output:
        output.write(text)
    print(text)


if __name__ == "__main__":
    main()
