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
        Path.home() / "mitts-benchmarks" / "audit_wt_ripgrep" / "wt-01",
        Path.home() / "mitts-benchmarks" / "projects" / "ripgrep",
        Path.cwd() / "projects" / "ripgrep",
        Path.cwd(),
    ]
    for c in candidates:
        if (c / "Cargo.toml").exists() and (c / "crates" / "matcher").exists():
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


def parse_cargo_tests(output: str):
    cargo_test_dict = {}
    current_cargo_bin = None

    for line in output.splitlines():
        m = re.search(r"Running (.*?) \((.*?)\)", line)
        if m:
            bin_path = m.group(2).strip()
            bin_name = bin_path.split("/")[-1]
            if "globset-" in bin_name:
                c_id = "globset"
            elif "grep-1" in bin_name or (
                "grep-" in bin_name
                and "cli" not in bin_name
                and "index" not in bin_name
                and "matcher" not in bin_name
                and "pcre2" not in bin_name
                and "printer" not in bin_name
                and "regex" not in bin_name
                and "searcher" not in bin_name
            ):
                c_id = "grep"
            elif "grep_cli-" in bin_name:
                c_id = "grep_cli"
            elif "grep_index-" in bin_name:
                c_id = "grep_index"
            elif "grep_matcher-" in bin_name:
                c_id = "grep_matcher"
            elif "integration-d513b6783aea7925" in bin_name or (
                "integration-" in bin_name
                and len(cargo_test_dict.get("grep_matcher", {})) > 0
                and "grep_matcher::integration" not in cargo_test_dict
            ):
                c_id = "grep_matcher::integration"
            elif "grep_pcre2-" in bin_name:
                c_id = "grep_pcre2"
            elif "grep_printer-" in bin_name:
                c_id = "grep_printer"
            elif "grep_regex-" in bin_name:
                c_id = "grep_regex"
            elif "grep_searcher-" in bin_name:
                c_id = "grep_searcher"
            elif "ignore-78c5ad8f496e1194" in bin_name or (
                "ignore-" in bin_name and "gitignore" not in bin_name
            ):
                c_id = "ignore"
            elif "gitignore_matched_path_or_any_parents_tests-" in bin_name:
                c_id = "ignore::gitignore_matched_path_or_any_parents_tests"
            elif "gitignore_skip_bom-" in bin_name:
                c_id = "ignore::gitignore_skip_bom"
            elif "rg-" in bin_name:
                c_id = "ripgrep::rg"
            elif "integration-" in bin_name:
                c_id = "ripgrep::integration"
            else:
                raise ValueError(f"Unknown cargo binary: {bin_name}")
            current_cargo_bin = c_id
            if current_cargo_bin not in cargo_test_dict:
                cargo_test_dict[current_cargo_bin] = {}
            continue
        m_test = re.match(r"test (.*?) \.\.\. (ok|ignored|FAILED)", line)
        if m_test and current_cargo_bin:
            cargo_test_dict[current_cargo_bin][m_test.group(1)] = m_test.group(2)

    return cargo_test_dict


def parse_mitts_tests(output: str):
    mitts_test_dict = {}
    current_mitts_bin = None

    for line in output.splitlines():
        m = re.search(r"Running (.*)", line)
        if m and "target/mitts" in line:
            bin_name = m.group(1).strip().split("/")[-1]
            if bin_name == "globset-globset_unit_test":
                m_id = "globset"
            elif bin_name == "grep-grep_unit_test":
                m_id = "grep"
            elif bin_name == "grep-cli-grep_cli_unit_test":
                m_id = "grep_cli"
            elif bin_name == "grep-index-grep_index_unit_test":
                m_id = "grep_index"
            elif bin_name == "grep-matcher-grep_matcher_unit_test":
                m_id = "grep_matcher"
            elif bin_name == "grep-matcher-integration":
                m_id = "grep_matcher::integration"
            elif bin_name == "grep-pcre2-grep_pcre2_unit_test":
                m_id = "grep_pcre2"
            elif bin_name == "grep-printer-grep_printer_unit_test":
                m_id = "grep_printer"
            elif bin_name == "grep-regex-grep_regex_unit_test":
                m_id = "grep_regex"
            elif bin_name == "grep-searcher-grep_searcher_unit_test":
                m_id = "grep_searcher"
            elif bin_name == "ignore-ignore_unit_test":
                m_id = "ignore"
            elif bin_name == "ignore-gitignore_matched_path_or_any_parents_tests":
                m_id = "ignore::gitignore_matched_path_or_any_parents_tests"
            elif bin_name == "ignore-gitignore_skip_bom":
                m_id = "ignore::gitignore_skip_bom"
            elif bin_name == "ripgrep-rg_bin_test":
                m_id = "ripgrep::rg"
            elif bin_name == "ripgrep-integration":
                m_id = "ripgrep::integration"
            else:
                raise ValueError(f"Unknown mitts binary: {bin_name}")
            current_mitts_bin = m_id
            if current_mitts_bin not in mitts_test_dict:
                mitts_test_dict[current_mitts_bin] = {}
            continue
        m_test = re.match(r"test (.*?) \.\.\. (ok|ignored|FAILED)", line)
        if m_test and current_mitts_bin:
            mitts_test_dict[current_mitts_bin][m_test.group(1)] = m_test.group(2)

    return mitts_test_dict


def main():
    parser = argparse.ArgumentParser(description="Generate defensible ripgrep test names list")
    parser.add_argument("--project-dir", type=Path, default=find_default_project_dir(), help="Path to ripgrep directory")
    parser.add_argument("--mitts-bin", type=Path, default=find_mitts_bin(), help="Path to mitts binary")
    parser.add_argument("--output", type=Path, default=Path("benchmark-results/ripgrep_test_names.txt"), help="Output path")
    args = parser.parse_args()

    project_dir = args.project_dir.resolve()
    mitts_bin = args.mitts_bin.resolve()

    print(f"Project dir: {project_dir}")
    print(f"Mitts bin:   {mitts_bin}")

    env = os.environ.copy()
    cargo_bin_dir = str(Path.home() / ".cargo" / "bin")
    if cargo_bin_dir not in env.get("PATH", ""):
        env["PATH"] = f"{cargo_bin_dir}:{env.get('PATH', '')}"

    # 1. Run Cargo
    print("Running cargo test...")
    cargo_cmd = ["cargo", "test", "--tests", "--workspace", "--no-fail-fast", "--", "--skip", "f1414"]
    p_c = subprocess.run(cargo_cmd, cwd=project_dir, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if p_c.returncode != 0:
        print(f"Cargo test exited with code {p_c.returncode}")
    cargo_tests = parse_cargo_tests(p_c.stdout)

    # 2. Run Mitts
    print("Running mitts test...")
    mitts_cmd = [str(mitts_bin), "test", "--workspace", "--no-fail-fast", "--", "--skip", "f1414"]
    p_m = subprocess.run(mitts_cmd, cwd=project_dir, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if p_m.returncode != 0:
        print(f"Mitts test exited with code {p_m.returncode}")
    mitts_tests = parse_mitts_tests(p_m.stdout)

    # 3. Combine and verify
    all_bins = sorted(set(cargo_tests.keys()) | set(mitts_tests.keys()))
    lines = []
    unique_names = set()
    mismatches = []
    ignored_count = 0
    ok_count = 0

    for b in all_bins:
        c_dict = cargo_tests.get(b, {})
        m_dict = mitts_tests.get(b, {})
        test_names = sorted(set(c_dict.keys()) | set(m_dict.keys()))
        for tname in test_names:
            c_stat = c_dict.get(tname, "missing")
            m_stat = m_dict.get(tname, "missing")
            full_test_id = f"{b}::{tname}"
            unique_names.add(full_test_id)
            if c_stat != m_stat:
                mismatches.append((full_test_id, c_stat, m_stat))
            if c_stat == "ignored":
                ignored_count += 1
            elif c_stat == "ok":
                ok_count += 1
            lines.append(f"{full_test_id} | cargo: {c_stat} | mitts: {m_stat}")

    total_tests = len(lines)
    print(f"Total tests:       {total_tests}")
    print(f"Unique test names: {len(unique_names)}")
    print(f"Passed (ok):       {ok_count}")
    print(f"Ignored:           {ignored_count}")
    print(f"Mismatches:        {len(mismatches)}")

    if total_tests != 1206:
        print(f"WARNING: Expected exactly 1206 tests, got {total_tests}", file=sys.stderr)
    if len(unique_names) != total_tests:
        print(f"ERROR: Duplicate test names detected ({total_tests} vs {len(unique_names)})", file=sys.stderr)
        sys.exit(1)
    if mismatches:
        print(f"ERROR: {len(mismatches)} status mismatches between Cargo and Mitts", file=sys.stderr)
        for m in mismatches[:10]:
            print(f"  {m[0]}: cargo={m[1]}, mitts={m[2]}", file=sys.stderr)
        sys.exit(1)

    # 4. Write output file
    args.output.parent.mkdir(parents=True, exist_ok=True)
    header = [
        "# Ripgrep Test Suite Parity Verification: Cargo vs Mitts",
        f"# Total tests: {total_tests} (Cargo: {ok_count + ignored_count}, Mitts: {ok_count + ignored_count}, Matched: {total_tests})",
        f"# Passed: {ok_count}, Ignored: {ignored_count}, Filtered out: 1 (f1414)",
        "# Format: <binary>::<test_name> | cargo: <status> | mitts: <status>",
        "",
    ]
    content = "\n".join(header + lines) + "\n"
    with open(args.output, "w", encoding="utf-8") as f:
        f.write(content)
    print(f"Successfully generated {args.output} ({len(lines)} tests)")


if __name__ == "__main__":
    main()
