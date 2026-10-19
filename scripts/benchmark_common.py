#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Cross-platform (Linux XFS/ext4 & macOS APFS) hardware/OS/filesystem provenance,
disk usage measurement, and preflight quiet-rig verification for Mitts benchmarks.
"""

from __future__ import annotations

import hashlib
import os
import platform
import re
import resource
import subprocess
import sys
import time
from pathlib import Path

try:
    _nofile_soft, _nofile_hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    if _nofile_soft < 65536:
        _target_soft = 65536 if _nofile_hard < 0 or _nofile_hard > 65536 else _nofile_hard
        resource.setrlimit(resource.RLIMIT_NOFILE, (_target_soft, _nofile_hard))
except Exception:
    pass

CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")
if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"

WATCHED_COMPILER_BINS = {"rustc", "cargo", "sccache", "mitts", "cc1", "cc1plus", "rustdoc", "clippy-driver"}


def get_dir_size_mb(path: Path | str) -> float:
    p = Path(path)
    if not p.exists():
        return 0.0
    total = 0
    for root, _, files in os.walk(p):
        for f in files:
            fp = os.path.join(root, f)
            if not os.path.islink(fp):
                try:
                    total += os.path.getsize(fp)
                except OSError:
                    pass
    return round(total / (1024.0 * 1024.0), 2)


def get_disk_usage_mb(paths: list[Path] | Path) -> float:
    """Calculates deduplicated disk usage in MiB across paths using `du -scm`."""
    if isinstance(paths, (str, Path)):
        paths = [Path(paths)]
    valid_paths = [str(p) for p in paths if Path(p).exists()]
    if not valid_paths:
        return 0.0
    try:
        out = subprocess.check_output(["du", "-scm"] + valid_paths, stderr=subprocess.DEVNULL, text=True)
        total_line = out.strip().splitlines()[-1]
        return float(total_line.split()[0])
    except Exception:
        return round(sum(get_dir_size_mb(p) for p in valid_paths), 1)


def get_rig_info() -> dict:
    """Queries hardware and OS metadata dynamically without hardcoded rig fallbacks."""
    cpu_model = "unknown"
    ram_gb = 0
    os_name = f"{platform.system()} {platform.release()}"
    logical_cpus = os.cpu_count() or 0

    if sys.platform == "darwin":
        try:
            cpu_model = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
        except Exception:
            cpu_model = platform.machine() or "unknown"
        try:
            ram_gb = round(int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True).strip()) / (1024**3))
        except Exception:
            ram_gb = 0
        try:
            pv = subprocess.check_output(["sw_vers", "-productVersion"], text=True).strip()
            os_name = f"macOS {pv}"
        except Exception:
            pass
    else:
        try:
            with open("/proc/cpuinfo", "r", encoding="utf-8") as f:
                for line in f:
                    if "model name" in line:
                        cpu_model = line.split(":", 1)[1].strip()
                        break
        except Exception:
            cpu_model = platform.machine() or "unknown"

        try:
            with open("/proc/meminfo", "r", encoding="utf-8") as f:
                for line in f:
                    if "MemTotal" in line:
                        kb = int(line.split()[1])
                        ram_gb = round(kb / (1024 * 1024))
                        break
        except Exception:
            ram_gb = 0

        try:
            with open("/etc/os-release", "r", encoding="utf-8") as f:
                for line in f:
                    if line.startswith("PRETTY_NAME="):
                        os_name = line.split("=", 1)[1].strip().strip('"')
                        break
        except Exception:
            pass

    rustc_v = "unknown"
    clippy_v = "unknown"
    try:
        rustc_v = subprocess.check_output(["rustc", "-vV"], text=True).strip().splitlines()[0]
    except Exception:
        pass
    try:
        clippy_v = subprocess.check_output(["cargo", "clippy", "-V"], text=True).strip()
    except Exception:
        pass
    try:
        nofile_soft = resource.getrlimit(resource.RLIMIT_NOFILE)[0]
    except Exception:
        nofile_soft = None

    return {
        "cpu": cpu_model,
        "logical_cpus": logical_cpus,
        "ram_gb": ram_gb,
        "os": os_name,
        "kernel": os.uname().release,
        "rlimit_nofile_soft": nofile_soft,
        "default_rustc_version": rustc_v,
        "default_clippy_version": clippy_v,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def get_binary_provenance(mitts_bin: Path, sccache_bin: Path | None = None) -> dict:
    """Extracts SHA-256 and version provenance for Mitts and sccache binaries."""
    mitts_sha = hashlib.sha256(mitts_bin.read_bytes()).hexdigest() if mitts_bin.is_file() else ""
    mitts_v = ""
    try:
        mitts_v = subprocess.check_output([str(mitts_bin), "--version"], text=True).strip()
    except Exception:
        pass

    m_git = re.search(r"\(([^)]+)\)", mitts_v)
    git_c = m_git.group(1) if m_git else ""

    sccache_v = ""
    sccache_sha = ""
    if sccache_bin and Path(sccache_bin).is_file():
        sccache_sha = hashlib.sha256(Path(sccache_bin).read_bytes()).hexdigest()
        try:
            sccache_v = subprocess.check_output([str(sccache_bin), "--version"], text=True).strip()
        except Exception:
            pass

    return {
        "binary_sha256": mitts_sha,
        "mitts_version": mitts_v,
        "git_commit": git_c,
        "relative_path": mitts_bin.name,
        "sccache_version": sccache_v,
        "sccache_sha256": sccache_sha,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def get_filesystem_type(path: Path) -> str:
    """Returns the exact mounted filesystem type (e.g. ext4, xfs, apfs) or 'unknown'."""
    p = Path(path).resolve()
    while not p.exists() and p.parent != p:
        p = p.parent

    if sys.platform == "darwin":
        try:
            out = subprocess.check_output(["mount"], text=True, stderr=subprocess.DEVNULL)
            best_mp = ""
            best_fs = "unknown"
            p_str = str(p)
            for line in out.splitlines():
                m = re.match(r"^.+?\s+on\s+(.+?)\s+\(([^,\s)]+)", line)
                if not m:
                    continue
                mp, fstype = m.group(1), m.group(2).lower()
                if p_str == mp or p_str.startswith(mp.rstrip("/") + "/") or mp == "/":
                    if len(mp) >= len(best_mp):
                        best_mp = mp
                        best_fs = fstype
            return best_fs
        except Exception:
            return "unknown"

    try:
        res = subprocess.check_output(
            ["findmnt", "-no", "FSTYPE", "--target", str(p)],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        if res:
            return res.splitlines()[0].strip()
    except Exception:
        pass
    try:
        lines = subprocess.check_output(["df", "-T", str(p)], text=True, stderr=subprocess.DEVNULL).strip().splitlines()
        if len(lines) > 1:
            parts = lines[1].split()
            if len(parts) > 1:
                return parts[1]
    except Exception:
        pass
    return "unknown"


def check_quiet_rig(max_load: float = 1.0) -> tuple[bool, float, list[dict]]:
    """
    Checks 1-minute loadavg and whether any compiler/build processes (`rustc`, `cargo`,
    `sccache`, `mitts`, `cc1`, `cc1plus`, `rustdoc`, `clippy-driver`) are running.
    Matches exact executable basename via `ps -eo pid=,comm=,args=` to avoid
    false positives on paths containing 'mitts' or `.cargo/bin/prod-code`.
    """
    try:
        load_1m = os.getloadavg()[0]
    except Exception:
        load_1m = 0.0

    foreign_procs: list[dict] = []
    current_pid = os.getpid()
    try:
        out = subprocess.check_output(
            ["ps", "-eo", "pid=,comm=,args="],
            text=True,
            stderr=subprocess.DEVNULL,
        )
        for line in out.splitlines():
            parts = line.strip().split(None, 2)
            if len(parts) < 2:
                continue
            try:
                pid = int(parts[0])
            except ValueError:
                continue
            if pid == current_pid:
                continue
            comm_base = os.path.basename(parts[1]).lower()
            args_str = parts[2] if len(parts) > 2 else parts[1]
            if comm_base in WATCHED_COMPILER_BINS:
                foreign_procs.append({"pid": pid, "bin": comm_base, "cmd": args_str[:120]})
    except Exception:
        pass

    is_quiet = (load_1m <= max_load) and (len(foreign_procs) == 0)
    return is_quiet, round(load_1m, 2), foreign_procs


def wait_for_quiet_rig(
    timeout_s: float = 300.0,
    max_load: float = 1.0,
    poll_interval_s: float = 2.0,
) -> tuple[float, list[dict], float]:
    """
    Waits until loadavg_1m <= max_load and no compiler/build processes exist.
    Raises RuntimeError if timeout expires so caller can record or abort cleanly.
    """
    start = time.perf_counter()
    while time.perf_counter() - start < timeout_s:
        is_quiet, load_1m, foreign_procs = check_quiet_rig(max_load=max_load)
        if is_quiet:
            return load_1m, foreign_procs, round(time.perf_counter() - start, 2)
        time.sleep(poll_interval_s)

    waited_s = round(time.perf_counter() - start, 2)
    is_quiet, load_1m, foreign_procs = check_quiet_rig(max_load=max_load)
    if not is_quiet:
        raise RuntimeError(
            f"Preflight quiet rig timeout ({timeout_s}s) exceeded: "
            f"loadavg_1m={load_1m} (max_load={max_load}), foreign_processes={foreign_procs}"
        )
    return load_1m, foreign_procs, waited_s
