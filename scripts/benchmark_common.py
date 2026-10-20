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

# Fraction of CPU time that may be busy (1 s sample) for the rig to count as quiet.
# The 1-minute loadavg is recorded but does not gate: a 12-worktree swarm pushes it
# far above any threshold and it then decays for minutes while the machine is idle.
QUIET_MAX_BUSY = 0.25


def set_quiet_policy(max_busy: float) -> None:
    """Sets the CPU-busy threshold used by `check_quiet_rig` / `wait_for_quiet_rig`."""
    global QUIET_MAX_BUSY
    QUIET_MAX_BUSY = max_busy


def _cpu_times_linux() -> tuple[int, int] | None:
    try:
        with open("/proc/stat", encoding="utf-8") as fh:
            fields = fh.readline().split()
    except OSError:
        return None
    if len(fields) < 5 or fields[0] != "cpu":
        return None
    values = [int(x) for x in fields[1:]]
    idle = values[3] + (values[4] if len(values) > 4 else 0)  # idle + iowait
    return sum(values), idle


def cpu_busy_fraction(sample_s: float = 1.0) -> float | None:
    """Fraction of all CPU time that was busy over the last `sample_s` seconds (0..1).

    Linux reads /proc/stat twice; macOS asks `top` for two samples. `None` when the
    platform offers neither, in which case the loadavg threshold gates instead.
    """
    if sys.platform.startswith("linux"):
        first = _cpu_times_linux()
        if first is None:
            return None
        time.sleep(sample_s)
        second = _cpu_times_linux()
        if second is None:
            return None
        total, idle = second[0] - first[0], second[1] - first[1]
        return round(1.0 - idle / total, 3) if total > 0 else None
    if sys.platform == "darwin":
        # `iostat -c 2 -w N` prints two samples; the second covers the last N seconds.
        # Its last line ends with `us sy id` followed by the three load averages.
        try:
            out = subprocess.check_output(
                ["iostat", "-c", "2", "-w", str(max(1, int(round(sample_s))))],
                text=True,
                stderr=subprocess.DEVNULL,
            )
        except Exception:
            return None
        data_lines = [ln for ln in out.splitlines() if ln.strip() and re.match(r"\s*[\d.]", ln)]
        if not data_lines:
            return None
        parts = data_lines[-1].split()
        try:
            idle = float(parts[-4])
        except (IndexError, ValueError):
            return None
        return round(1.0 - idle / 100.0, 3)
    return None


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
    try:
        sched_pri = int(subprocess.check_output(["ps", "-o", "pri=", "-p", str(os.getpid())], text=True).strip())
    except Exception:
        sched_pri = None
    try:
        proc_nice = os.nice(0)
    except Exception:
        proc_nice = None

    return {
        "cpu": cpu_model,
        "logical_cpus": logical_cpus,
        "ram_gb": ram_gb,
        "os": os_name,
        "kernel": os.uname().release,
        "rlimit_nofile_soft": nofile_soft,
        "sched_pri": sched_pri,
        "nice": proc_nice,
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


def check_quiet_rig(max_load: float = 1.0, max_busy: float | None = None) -> tuple[bool, float, list[dict], float | None]:
    """
    Decides whether the rig is quiet: no compiler/build processes (`rustc`, `cargo`,
    `sccache`, `mitts`, `cc1`, `cc1plus`, `rustdoc`, `clippy-driver`) are running and
    the CPU was at most `max_busy` busy over a 1 s sample (`QUIET_MAX_BUSY` when
    `None`). The 1-minute loadavg is returned for the record and gates only when the
    CPU sample is unavailable. Processes are matched by exact executable basename via
    `ps -eo pid=,comm=,args=` to avoid false positives on paths containing 'mitts' or
    `.cargo/bin/prod-code`.
    Returns `(is_quiet, load_1m, foreign_procs, cpu_busy)`.
    """
    threshold = QUIET_MAX_BUSY if max_busy is None else max_busy
    try:
        load_1m = os.getloadavg()[0]
    except Exception:
        load_1m = 0.0
    busy = cpu_busy_fraction(1.0)

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
            args_str = parts[2] if len(parts) > 2 else parts[1]
            # macOS truncates `comm` to 16 characters (a path becomes `/Users/alex/.`), so the
            # executable name comes from the first token of `args`; `comm` stays as a fallback.
            exe_base = os.path.basename(args_str.split(None, 1)[0]).lower()
            comm_base = os.path.basename(parts[1]).lower()
            bin_name = exe_base if exe_base in WATCHED_COMPILER_BINS else comm_base
            if bin_name not in WATCHED_COMPILER_BINS:
                continue
            # An sccache *server* (the bare binary, no compiler arguments) idles between
            # builds and consumes nothing; any cargo on the machine leaves one behind.
            # Only `sccache rustc ...` invocations are compiles.
            if bin_name == "sccache" and len(args_str.split()) <= 1:
                continue
            foreign_procs.append({"pid": pid, "bin": bin_name, "cmd": args_str[:120]})
    except Exception:
        pass

    cpu_quiet = (busy <= threshold) if busy is not None else (load_1m <= max_load)
    is_quiet = cpu_quiet and (len(foreign_procs) == 0)
    return is_quiet, round(load_1m, 2), foreign_procs, busy


def wait_for_quiet_rig(
    timeout_s: float = 300.0,
    max_load: float = 1.0,
    poll_interval_s: float = 2.0,
    max_busy: float | None = None,
) -> tuple[float, list[dict], float, float | None]:
    """
    Waits until the CPU is at most `max_busy` busy (1 s sample) and no compiler/build
    processes exist; `max_load` gates only when the CPU sample is unavailable.
    Raises RuntimeError if timeout expires so caller can record or abort cleanly.
    Returns `(loadavg_1m, foreign_procs, waited_s, cpu_busy)`.
    """
    start = time.perf_counter()
    while time.perf_counter() - start < timeout_s:
        is_quiet, load_1m, foreign_procs, busy = check_quiet_rig(max_load=max_load, max_busy=max_busy)
        if is_quiet:
            return load_1m, foreign_procs, round(time.perf_counter() - start, 2), busy
        time.sleep(poll_interval_s)

    waited_s = round(time.perf_counter() - start, 2)
    is_quiet, load_1m, foreign_procs, busy = check_quiet_rig(max_load=max_load, max_busy=max_busy)
    if not is_quiet:
        raise RuntimeError(
            f"Preflight quiet rig timeout ({timeout_s}s) exceeded: "
            f"cpu_busy={busy} (max_busy={QUIET_MAX_BUSY if max_busy is None else max_busy}), "
            f"loadavg_1m={load_1m} (max_load={max_load}), foreign_processes={foreign_procs}"
        )
    return load_1m, foreign_procs, waited_s, busy
