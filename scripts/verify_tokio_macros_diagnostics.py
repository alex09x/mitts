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
import collections
import hashlib
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


def compute_file_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()


def extract_diagnostics(output: str):
    """
    Extracts structured diagnostics: (kind, file, line, col) as collections.Counter
    """
    pattern = re.compile(r"warning: (.*?)\n\s+--> (.*?):(\d+):(\d+)", re.MULTILINE)
    diags = collections.Counter()
    for m in pattern.finditer(output):
        msg = m.group(1)
        file_path = m.group(2).replace("\\", "/")
        line_num = int(m.group(3))
        col_num = int(m.group(4))

        # Normalize relative file path
        if "tokio-macros" in file_path:
            file_path = "tokio-macros" + file_path.split("tokio-macros")[-1]

        if "unused variable" in msg:
            kind = "unused_variables"
        elif "writing `&Vec`" in msg or "ptr_arg" in msg:
            kind = "clippy::ptr_arg"
        elif "length comparison to zero" in msg or "len_zero" in msg:
            kind = "clippy::len_zero"
        else:
            kind = msg.split(":")[0].strip()

        diags[(kind, file_path, line_num, col_num)] += 1
    return diags


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

    # Mitts provenance
    mitts_ver = subprocess.run([str(mitts_bin), "--version"], capture_output=True, text=True).stdout.strip()
    mitts_sha256 = compute_file_sha256(mitts_bin)
    cargo_ver = subprocess.run(["cargo", "--version"], capture_output=True, text=True, env=env).stdout.strip()

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

        # 1. Check mode
        print("Running cargo check -v -p tokio-macros...")
        p_c_chk = subprocess.run(
            ["cargo", "check", "-v", "-p", "tokio-macros"],
            cwd=project_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        print("Running mitts check -v -p tokio-macros...")
        p_m_chk = subprocess.run(
            [str(mitts_bin), "check", "-v", "-p", "tokio-macros"],
            cwd=project_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

        # 2. Clippy mode
        print("Running cargo clippy -v -p tokio-macros...")
        p_c_clp = subprocess.run(
            ["cargo", "clippy", "-v", "-p", "tokio-macros"],
            cwd=project_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        print("Running mitts clippy -v -p tokio-macros...")
        p_m_clp = subprocess.run(
            [str(mitts_bin), "clippy", "-v", "-p", "tokio-macros"],
            cwd=project_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

        # Write sanitized logs with provenance and exit codes
        cargo_header = (
            f"# Cargo Diagnostic Parity Log (tokio-macros)\n"
            f"# Toolchain: {cargo_ver}\n"
            f"# Exit Codes: check={p_c_chk.returncode}, clippy={p_c_clp.returncode}\n\n"
            f"=== 1. CARGO CHECK OUTPUT ===\n\n"
        )
        with open(cargo_log_path, "w", encoding="utf-8") as f:
            f.write(cargo_header)
            f.write(sanitize_paths(p_c_chk.stdout))
            f.write("\n\n=== 2. CARGO CLIPPY OUTPUT ===\n\n")
            f.write(sanitize_paths(p_c_clp.stdout))

        mitts_header = (
            f"# Mitts Diagnostic Parity Log (tokio-macros)\n"
            f"# Mitts Version: {mitts_ver}\n"
            f"# Binary SHA-256: {mitts_sha256}\n"
            f"# Exit Codes: check={p_m_chk.returncode}, clippy={p_m_clp.returncode}\n\n"
            f"=== 1. MITTS CHECK OUTPUT ===\n\n"
        )
        with open(mitts_log_path, "w", encoding="utf-8") as f:
            f.write(mitts_header)
            f.write(sanitize_paths(p_m_chk.stdout))
            f.write("\n\n=== 2. MITTS CLIPPY OUTPUT ===\n\n")
            f.write(sanitize_paths(p_m_clp.stdout))

    finally:
        print("Restoring original tokio-macros/src/lib.rs...")
        lib_rs.write_text(original_content, encoding="utf-8")

    # Structured diagnostic comparisons
    c_chk_diags = extract_diagnostics(p_c_chk.stdout)
    m_chk_diags = extract_diagnostics(p_m_chk.stdout)
    c_clp_diags = extract_diagnostics(p_c_clp.stdout)
    m_clp_diags = extract_diagnostics(p_m_clp.stdout)

    all_passed = True
    print("\nParity verification results:")

    # Exit code checks
    chk_exit_match = (p_c_chk.returncode == p_m_chk.returncode)
    clp_exit_match = (p_c_clp.returncode == p_m_clp.returncode)
    print(f"  [{'MATCH' if chk_exit_match else 'MISMATCH'}] Check exit codes: Cargo={p_c_chk.returncode}, Mitts={p_m_chk.returncode}")
    print(f"  [{'MATCH' if clp_exit_match else 'MISMATCH'}] Clippy exit codes: Cargo={p_c_clp.returncode}, Mitts={p_m_clp.returncode}")
    if not (chk_exit_match and clp_exit_match):
        all_passed = False

    # Check mode diagnostics parity
    chk_diag_match = (c_chk_diags == m_chk_diags and sum(c_chk_diags.values()) > 0)
    print(f"  [{'MATCH' if chk_diag_match else 'MISMATCH'}] Check diagnostics exact match ({sum(c_chk_diags.values())} diagnostics):")
    for d, count in sorted(c_chk_diags.items()):
        print(f"    - {d[0]} at {d[1]}:{d[2]}:{d[3]} (count={count})")
    if not chk_diag_match:
        diff_c = c_chk_diags - m_chk_diags
        diff_m = m_chk_diags - c_chk_diags
        if diff_c:
            print(f"    Cargo extra: {dict(diff_c)}")
        if diff_m:
            print(f"    Mitts extra: {dict(diff_m)}")
        all_passed = False

    # Clippy mode diagnostics parity
    clp_diag_match = (c_clp_diags == m_clp_diags and sum(c_clp_diags.values()) > 0)
    print(f"  [{'MATCH' if clp_diag_match else 'MISMATCH'}] Clippy diagnostics exact match ({sum(c_clp_diags.values())} diagnostics):")
    for d, count in sorted(c_clp_diags.items()):
        print(f"    - {d[0]} at {d[1]}:{d[2]}:{d[3]} (count={count})")
    if not clp_diag_match:
        diff_c = c_clp_diags - m_clp_diags
        diff_m = m_clp_diags - c_clp_diags
        if diff_c:
            print(f"    Cargo extra: {dict(diff_c)}")
        if diff_m:
            print(f"    Mitts extra: {dict(diff_m)}")
        all_passed = False

    if not all_passed:
        print("\nERROR: Diagnostic parity verification failed for tokio-macros!", file=sys.stderr)
        sys.exit(1)

    print(f"\nSUCCESS: tokio-macros diagnostic parity (both check and clippy) verified 100%!")
    print(f"Cargo log: {cargo_log_path}")
    print(f"Mitts log: {mitts_log_path}")


if __name__ == "__main__":
    main()
