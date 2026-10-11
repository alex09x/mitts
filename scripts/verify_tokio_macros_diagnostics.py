#
# mitts — Rust-first build tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#

import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def find_default_project_dir():
    candidates = [
        Path.home() / "mitts-benchmarks" / "audit_wt_tokio" / "wt-01",
        Path.home() / "mitts-benchmarks" / "projects" / "tokio",
        Path.cwd() / "projects" / "tokio",
        Path.cwd(),
    ]
    for c in candidates:
        if (c / "Cargo.toml").exists() and (c / "tokio-macros" / "src" / "lib.rs").exists():
            return c
    return candidates[0]


def find_mitts_bin():
    candidates = [
        Path.home() / "mitts-benchmarks" / "mitts-bin",
        Path.cwd() / "target" / "release" / "mitts",
        Path.cwd() / "target" / "debug" / "mitts",
    ]
    for c in candidates:
        if c.exists() and os.access(c, os.X_OK):
            return c
    which_m = shutil.which("mitts")
    if which_m:
        return Path(which_m)
    return candidates[0]


def sanitize_paths(text: str) -> str:
    home = str(Path.home())
    return text.replace(home, "~")


def main():
    parser = argparse.ArgumentParser(description="Verify diagnostic parity for tokio-macros between Cargo and Mitts")
    parser.add_argument("--project-dir", type=Path, default=find_default_project_dir(), help="Path to tokio repository")
    parser.add_argument("--mitts-bin", type=Path, default=find_mitts_bin(), help="Path to mitts binary")
    parser.add_argument("--results-dir", type=Path, default=Path("benchmark-results"), help="Directory for output logs")
    args = parser.parse_args()

    project_dir = args.project_dir.resolve()
    mitts_bin = args.mitts_bin.resolve()
    results_dir = args.results_dir.resolve()
    results_dir.mkdir(parents=True, exist_ok=True)

    lib_rs = project_dir / "tokio-macros" / "src" / "lib.rs"
    if not lib_rs.exists():
        print(f"ERROR: {lib_rs} not found", file=sys.stderr)
        sys.exit(1)

    print(f"Tokio project: {project_dir}")
    print(f"Mitts bin:     {mitts_bin}")
    print(f"Target file:   {lib_rs}")

    env = os.environ.copy()
    cargo_bin_dir = str(Path.home() / ".cargo" / "bin")
    if cargo_bin_dir not in env.get("PATH", ""):
        env["PATH"] = f"{cargo_bin_dir}:{env.get('PATH', '')}"

    original_content = lib_rs.read_text(encoding="utf-8")
    injection_snippet = (
        "\n\n// Intentional audit diagnostics for tokio-macros parity verification\n"
        "#[allow(dead_code)]\n"
        "fn __intentional_tokio_macro_diagnostic(v: &Vec<u8>) {\n"
        "    let unused_tokio_macro_var = 42;\n"
        "    if v.len() == 0 {}\n"
        "}\n"
    )

    cargo_log_path = results_dir / "tokio_macros_cargo.log"
    mitts_log_path = results_dir / "tokio_macros_mitts.log"

    try:
        print("Injecting intentional warning and clippy lints into tokio-macros/src/lib.rs...")
        lib_rs.write_text(original_content + injection_snippet, encoding="utf-8")

        print("Running cargo clippy -v -p tokio-macros...")
        p_c = subprocess.run(
            ["cargo", "clippy", "-v", "-p", "tokio-macros"],
            cwd=project_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        cargo_output = p_c.stdout
        with open(cargo_log_path, "w", encoding="utf-8") as f:
            f.write(sanitize_paths(cargo_output))

        print("Running mitts clippy -v -p tokio-macros...")
        p_m = subprocess.run(
            [str(mitts_bin), "clippy", "-v", "-p", "tokio-macros"],
            cwd=project_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        mitts_output = p_m.stdout
        with open(mitts_log_path, "w", encoding="utf-8") as f:
            f.write(sanitize_paths(mitts_output))

    finally:
        print("Restoring original tokio-macros/src/lib.rs...")
        lib_rs.write_text(original_content, encoding="utf-8")

    # Verify diagnostic detections
    checks = [
        ("unused variable `unused_tokio_macro_var`", "unused_tokio_macro_var"),
        ("clippy::ptr_arg (&Vec parameter)", "ptr_arg"),
        ("clippy::len_zero (length comparison to zero)", "len_zero"),
    ]

    all_passed = True
    print("\nParity verification results:")
    for label, pattern in checks:
        in_cargo = (pattern in cargo_output) or (pattern == "ptr_arg" and "writing `&Vec`" in cargo_output) or (pattern == "len_zero" and "length comparison to zero" in cargo_output)
        in_mitts = (pattern in mitts_output) or (pattern == "ptr_arg" and "writing `&Vec`" in mitts_output) or (pattern == "len_zero" and "length comparison to zero" in mitts_output)
        status = "MATCH" if in_cargo == in_mitts and in_cargo else "MISMATCH"
        print(f"  [{status}] {label}: Cargo={in_cargo}, Mitts={in_mitts}")
        if status != "MATCH":
            all_passed = False

    if not all_passed:
        print("\nERROR: Diagnostic parity verification failed for tokio-macros!", file=sys.stderr)
        sys.exit(1)

    print(f"\nSUCCESS: tokio-macros diagnostic parity verified 100%!")
    print(f"Cargo log: {cargo_log_path}")
    print(f"Mitts log: {mitts_log_path}")


if __name__ == "__main__":
    main()
