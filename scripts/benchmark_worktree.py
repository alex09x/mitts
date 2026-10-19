#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Git commit history discovery, deterministic per-commit lockfile restoration,
worktree preparation, repository copy helpers, and WT-12 diagnostic anomaly injection.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import time
from pathlib import Path

from t5_parsers import DEFECT_TEST_NAME, select_workspace_targets

ANOMALY_FN_NAME = "__mitts_audit_anomaly"
ANOMALY_VAR_NAME = "unused_test_var"

__all__ = [
    "ANOMALY_FN_NAME",
    "ANOMALY_VAR_NAME",
    "DEFECT_TEST_NAME",
    "assert_dir_empty_or_absent",
    "chmod_rmtree",
    "copy_repo_tree",
    "ensure_commit_history",
    "git_checkout_clean",
    "inject_anomaly",
    "prepare_commit_locks_and_fetch",
    "prepare_swarm_worktrees",
    "restore_commit_lock",
]


def chmod_rmtree(path: Path) -> None:
    p = Path(path)
    if p.exists() or p.is_symlink():
        if p.is_symlink() or p.is_file():
            p.unlink(missing_ok=True)
            return
        subprocess.run(["chmod", "-R", "u+rwx", str(p)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        shutil.rmtree(p, ignore_errors=True)


def assert_dir_empty_or_absent(path: Path) -> None:
    p = Path(path)
    if p.exists() and any(p.iterdir()):
        raise RuntimeError(f"Cache/target directory was not purged cleanly: {p}")


def copy_repo_tree(src: Path, dst: Path) -> None:
    """Copies a repository tree ignoring only the root-level `target` directory."""
    src_res = Path(src).resolve()
    chmod_rmtree(dst)

    def _ignore(cur_dir: str, names: list[str]) -> list[str]:
        if Path(cur_dir).resolve() == src_res and "target" in names:
            return ["target"]
        return []

    shutil.copytree(src_res, dst, symlinks=True, ignore=_ignore)


def restore_commit_lock(repo_or_wt_dir: Path, commit: str, commit_locks: dict[str, Path] | None) -> None:
    """Unconditionally restores the deterministic `Cargo.lock` snapshot for `commit`."""
    if not commit_locks:
        return
    if commit not in commit_locks:
        raise KeyError(f"Missing Cargo.lock snapshot for commit {commit}")
    snap = commit_locks[commit]
    if not snap.is_file():
        raise FileNotFoundError(f"Cargo.lock snapshot file missing: {snap}")
    dst = repo_or_wt_dir / "Cargo.lock"
    if dst.exists() or dst.is_symlink():
        dst.chmod(0o644)
    shutil.copy2(snap, dst)


def git_checkout_clean(repo_dir: Path, commit: str, commit_locks: dict[str, Path] | None = None) -> None:
    if commit != "HEAD":
        r = subprocess.run(["git", "checkout", "-f", commit], cwd=repo_dir, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if r.returncode != 0:
            raise RuntimeError(f"git checkout -f {commit} failed in {repo_dir}: {r.stderr.strip()}")
        if (repo_dir / ".gitmodules").exists():
            subprocess.run(["git", "submodule", "update", "--init", "--recursive", "--force"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    restore_commit_lock(repo_dir, commit, commit_locks)


def ensure_commit_history(repo_dir: Path, needed: int = 20) -> list[str]:
    try:
        cnt = int(subprocess.check_output(["git", "rev-list", "--first-parent", "--count", "HEAD"], cwd=repo_dir, text=True, stderr=subprocess.DEVNULL).strip())
        if cnt < needed:
            subprocess.run(["git", "fetch", "--depth", str(needed + 10), "origin"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=90)
    except Exception:
        pass
    try:
        out = subprocess.check_output(["git", "log", "--first-parent", "-n", str(needed), "--format=%H"], cwd=repo_dir, text=True, stderr=subprocess.DEVNULL)
        commits = [c.strip() for c in out.splitlines() if c.strip()]
        return commits if commits else ["HEAD"]
    except Exception:
        return ["HEAD"]


def prepare_commit_locks_and_fetch(
    src_dir: Path,
    proj_work: Path,
    ctx: dict,
    head_commit: str,
    unique_commits: list[str],
    res: dict,
) -> dict[str, Path]:
    """Resolves/fetches dependencies per commit, snapshots `Cargo.lock` after fetch, and pre-unpacks `registry/src`."""
    git_checkout_clean(src_dir, head_commit)
    lock_tracked = (
        subprocess.run(["git", "ls-files", "--error-unmatch", "Cargo.lock"], cwd=src_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    )
    res["cargo_lock_tracked_in_git"] = lock_tracked
    lock_path = src_dir / "Cargo.lock"
    if not lock_path.exists():
        subprocess.run(["cargo", "generate-lockfile"], cwd=src_dir, env=ctx["fetch_env"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    master_lock = proj_work / "master_Cargo.lock"
    if lock_path.is_file():
        shutil.copy2(lock_path, master_lock)
    else:
        master_lock = None

    commit_locks: dict[str, Path] = {}
    commit_lock_sha256: dict[str, str] = {}
    fetch_exit_codes: dict[str, int] = {}
    lock_rewritten_commits: list[str] = []
    for c in unique_commits:
        git_checkout_clean(src_dir, c)
        c_tracked = (
            subprocess.run(["git", "ls-files", "--error-unmatch", "Cargo.lock"], cwd=src_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
        )
        if not c_tracked:
            if master_lock and master_lock.is_file():
                shutil.copy2(master_lock, src_dir / "Cargo.lock")
            pf = subprocess.run(["cargo", "fetch"], cwd=src_dir, env=ctx["fetch_env"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        else:
            pf = subprocess.run(["cargo", "fetch", "--locked"], cwd=src_dir, env=ctx["fetch_env"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            if pf.returncode != 0:
                pf = subprocess.run(["cargo", "fetch"], cwd=src_dir, env=ctx["fetch_env"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
                if pf.returncode == 0:
                    lock_rewritten_commits.append(c[:12])
        fetch_exit_codes[c[:12]] = pf.returncode
        if pf.returncode != 0:
            raise RuntimeError(f"cargo fetch failed on commit {c[:12]}: {pf.stderr.strip()[-300:]}")
        c_lock = proj_work / f"lock_{c[:12]}.lock"
        if (src_dir / "Cargo.lock").is_file():
            shutil.copy2(src_dir / "Cargo.lock", c_lock)
            commit_locks[c] = c_lock
            commit_lock_sha256[c[:12]] = hashlib.sha256(c_lock.read_bytes()).hexdigest()
        subprocess.run(["cargo", "metadata", "--offline", "--format-version", "1"], cwd=src_dir, env=ctx["envs"]["cargo_bare"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)

    if head_commit in commit_locks:
        commit_locks["HEAD"] = commit_locks[head_commit]
        hb = commit_locks[head_commit].read_bytes()
        res["cargo_lock_sha256"] = commit_lock_sha256[head_commit[:12]]
        res["lock_packages"] = hb.decode("utf-8", errors="replace").count("[[package]]")
    else:
        res["cargo_lock_sha256"] = None
        res["lock_packages"] = 0
    ctx["cargo_locked"] = bool(commit_locks)
    res["lock_rewritten"] = bool(lock_rewritten_commits)
    res["lock_rewritten_commits"] = lock_rewritten_commits
    res["commit_lock_sha256"] = commit_lock_sha256
    res["fetch_exit_codes"] = fetch_exit_codes
    git_checkout_clean(src_dir, head_commit, commit_locks)
    return commit_locks


def prepare_swarm_worktrees(
    repo_dir: Path,
    wt_root: Path,
    commits: list[str],
    commit_locks: dict[str, Path] | None = None,
) -> list[tuple[int, Path, str]]:
    chmod_rmtree(wt_root)
    wt_root.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "worktree", "prune"], cwd=repo_dir, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    now_ts = time.time()
    wts: list[tuple[int, Path, str]] = []
    for i, c in enumerate(commits, start=1):
        wt_d = wt_root / f"wt-{i:02d}"
        r = subprocess.run(["git", "worktree", "add", "--force", "--detach", str(wt_d), c], cwd=repo_dir, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if r.returncode != 0:
            raise RuntimeError(f"git worktree add failed for {wt_d} @ {c}: {r.stderr.strip()}")
        if (wt_d / ".gitmodules").exists():
            subprocess.run(["git", "submodule", "update", "--init", "--recursive", "--force"], cwd=wt_d, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        restore_commit_lock(wt_d, c, commit_locks)
        for root, _, files in os.walk(wt_d):
            if ".git" in Path(root).parts:
                continue
            for f in files:
                try:
                    os.utime(Path(root) / f, (now_ts, now_ts))
                except OSError:
                    pass
        wts.append((i, wt_d, c))
    return wts


def inject_anomaly(wt_dir: Path, mode: str = "check", env: dict[str, str] | None = None) -> dict:
    """
    Injects intentional diagnostics in WT-12 (`unused_variables` in check,
    plus `clippy::len_zero` in clippy) using deterministic `cargo metadata`
    workspace target discovery.
    """
    wt_dir = Path(wt_dir).resolve()
    info = select_workspace_targets(wt_dir, env=env)
    target = info["lib_or_bin_src"]
    if not target or not target.is_file():
        raise RuntimeError(f"Could not locate a valid lib/bin target source file in {wt_dir}")

    original = target.read_text(encoding="utf-8", errors="replace")
    separator = "" if original.endswith("\n") else "\n"
    clippy_line = "    let _clippy_anomaly = vec![1].len() == 0;\n" if mode == "clippy" else ""
    snippet = (
        separator
        + f"#[allow(dead_code)]\n#[allow(missing_docs)]\n#[doc(hidden)]\n"
        + f"pub fn {ANOMALY_FN_NAME}() {{\n"
        + f"    let {ANOMALY_VAR_NAME} = 42;\n"
        + f"{clippy_line}}}\n"
    )
    new_content = original + snippet
    target.write_text(new_content, encoding="utf-8")

    anomaly_line = None
    for idx, line in enumerate(new_content.splitlines(), start=1):
        if ANOMALY_VAR_NAME in line:
            anomaly_line = idx
            break

    return {
        "relative_file": str(target.relative_to(wt_dir)),
        "line": anomaly_line,
        "mode": mode,
        "expected_symbol": ANOMALY_VAR_NAME,
    }
