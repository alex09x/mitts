# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Worktree, filesystem, and cache lifecycle management for multi-worktree benchmarks.
"""

import os
import shutil
import subprocess
import time
from pathlib import Path

CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")
if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"

SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))


def get_disk_usage_mb(paths):
    """Calculates physical disk usage in MB using du -scm."""
    valid = [str(p) for p in paths if Path(p).exists()]
    if not valid:
        return 0.0
    try:
        for p in valid:
            subprocess.run(
                ["find", p, "-type", "d", "!", "-perm", "-u+rx", "-exec", "chmod", "u+rx", "{}", "+"],
                stderr=subprocess.DEVNULL,
            )
        out = subprocess.check_output(["du", "-scm"] + valid, stderr=subprocess.DEVNULL, text=True)
        return float(out.strip().splitlines()[-1].split()[0])
    except Exception:
        return 0.0


def read_cgroup_peak_bytes():
    """Reads cgroup v2 peak memory usage if available."""
    cgroup_paths = [
        Path("/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/memory.peak"),
        Path("/sys/fs/cgroup/memory.peak"),
    ]
    for cp in cgroup_paths:
        if cp.is_file():
            try:
                return int(cp.read_text().strip())
            except Exception:
                pass
    return 0


def prepare_worktrees(repo_dir, base_dir, commits):
    """
    Creates fresh git worktrees and touches files to assert mtimes
    are strictly newer than previous snapshots.
    """
    base_dir = Path(base_dir).resolve()
    repo_dir = Path(repo_dir).resolve()
    base_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "worktree", "prune"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wts = []
    time.sleep(1.0)
    now_ts = time.time()

    for i, commit in enumerate(commits, start=1):
        wt_dir = base_dir / f"wt-{i:02d}"
        if wt_dir.exists():
            subprocess.run(["git", "worktree", "remove", "--force", str(wt_dir)], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["chmod", "-R", "u+rwx", str(wt_dir)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            shutil.rmtree(wt_dir, ignore_errors=True)
        subprocess.run(["git", "worktree", "prune"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["git", "worktree", "add", "-f", "-d", str(wt_dir), commit], cwd=repo_dir, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        for root, _, files in os.walk(wt_dir):
            if ".git" in root:
                continue
            for f in files:
                p = Path(root) / f
                try:
                    os.utime(p, (now_ts, now_ts))
                except OSError:
                    pass

        lock_file = repo_dir / "Cargo.lock"
        if lock_file.exists() and not (wt_dir / "Cargo.lock").exists():
            shutil.copy(lock_file, wt_dir / "Cargo.lock")

        subprocess.run(["cargo", "fetch"], cwd=wt_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        wts.append((i, wt_dir, commit))

    return wts


def inject_anomaly(wt_dir, mode="check"):
    """Injects intentional diagnostics in WT 12 (unused_variables in check, plus clippy::len_zero in clippy)."""
    candidates = [
        wt_dir / "src/lib.rs",
        wt_dir / "src/main.rs",
        wt_dir / "crates/nu-protocol/src/lib.rs",
        wt_dir / "crates/nu-utils/src/lib.rs",
        wt_dir / "tokio/src/lib.rs",
        wt_dir / "serde/src/lib.rs",
        wt_dir / "crates/core/lib.rs",
    ]
    target = next((c for c in candidates if c.is_file()), None)
    if not target:
        for r, _, files in os.walk(wt_dir):
            if "lib.rs" in files and ".git" not in r:
                target = Path(r) / "lib.rs"
                break

    if target:
        with open(target, "r") as f:
            original = f.read()
        separator = "" if original.endswith("\n") else "\n"
        clippy_line = "    let _clippy_anomaly = vec![1].len() == 0;\n" if mode == "clippy" else ""
        snippet = separator + f"#[allow(dead_code)]\n#[allow(missing_docs)]\n#[doc(hidden)]\npub fn __mitts_audit_anomaly() {{\n    let unused_test_var = 42;\n{clippy_line}}}\n"
        new_content = original + snippet
        with open(target, "w") as f:
            f.write(new_content)

        anomaly_line = None
        for idx, line in enumerate(new_content.splitlines(), start=1):
            if "unused_test_var" in line:
                anomaly_line = idx
                break

        rel_path = str(target.relative_to(wt_dir))
        return {
            "relative_file": rel_path,
            "line": anomaly_line,
            "mode": mode,
        }
    return None


def snapshot_cache(source_dir, snapshot_dir):
    """Snapshots a cache directory (ignoring FIFOs, jobserver, tmp)."""
    if snapshot_dir.exists():
        shutil.rmtree(snapshot_dir, ignore_errors=True)
    if source_dir.exists():
        shutil.copytree(
            source_dir,
            snapshot_dir,
            ignore=shutil.ignore_patterns("*.fifo", "jobserver", "tmp"),
        )


def restore_cache(snapshot_dir, target_dir):
    """Restores a cache directory from a snapshot (ignoring FIFOs, jobserver, tmp)."""
    target_dir.mkdir(parents=True, exist_ok=True)
    for sub in ["cas", "ac", "metadata"]:
        target_sub = target_dir / sub
        if target_sub.exists():
            shutil.rmtree(target_sub, ignore_errors=True)
        snap_sub = snapshot_dir / sub
        if snap_sub.exists():
            shutil.copytree(snap_sub, target_sub)


def clean_sccache(sccache_dir):
    """Cleans and resets sccache daemon."""
    try:
        subprocess.run([str(SCCACHE_BIN), "--stop-server"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass
    if sccache_dir.exists():
        shutil.rmtree(sccache_dir, ignore_errors=True)
    sccache_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["SCCACHE_DIR"] = str(sccache_dir)
    try:
        subprocess.run([str(SCCACHE_BIN), "--start-server"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run([str(SCCACHE_BIN), "--zero-stats"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass
