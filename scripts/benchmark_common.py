#!/usr/bin/env python3
"""
Common utilities for Mitts defensible benchmarking on Linux/x86_64.
Provides process measurement (wall-clock, CPU, RSS via wait4),
cache management, worktree helpers, and disk measurement.
"""

import os
import shutil
import subprocess
import time
from pathlib import Path

CARGO_BIN_DIR = str(Path.home() / ".cargo" / "bin")
if CARGO_BIN_DIR not in os.environ.get("PATH", ""):
    os.environ["PATH"] = f"{CARGO_BIN_DIR}:{os.environ.get('PATH', '')}"

MITTS_BIN = Path(
    os.environ.get(
        "MITTS_BIN",
        shutil.which("mitts") or (Path.home() / "mitts/target/release/mitts")
    )
)
SCCACHE_BIN = Path(shutil.which("sccache") or (Path.home() / ".cargo/bin/sccache"))


def measure_command(cmd, cwd, env=None):
    """Executes a command and measures wall-clock, CPU time, and peak RSS using wait4."""
    if env is None:
        env = os.environ.copy()
    else:
        env = env.copy()
    if CARGO_BIN_DIR not in env.get("PATH", ""):
        env["PATH"] = f"{CARGO_BIN_DIR}:{env.get('PATH', '')}"

    start = time.perf_counter()
    p = subprocess.Popen(
        cmd,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    _, status, ru = os.wait4(p.pid, 0)
    stdout, stderr = p.communicate()
    elapsed = time.perf_counter() - start

    exit_code = os.waitstatus_to_exitcode(status) if hasattr(os, "waitstatus_to_exitcode") else (status >> 8)
    utime = ru.ru_utime
    stime = ru.ru_stime
    total_cpu = utime + stime
    cpu_percent = (total_cpu / max(elapsed, 0.001)) * 100.0
    max_rss_mb = ru.ru_maxrss / 1024.0

    return {
        "wall_sec": round(elapsed, 3),
        "user_sec": round(utime, 3),
        "sys_sec": round(stime, 3),
        "total_cpu_sec": round(total_cpu, 3),
        "cpu_percent": round(cpu_percent, 1),
        "max_rss_mb": round(max_rss_mb, 1),
        "exit_code": exit_code,
        "stdout_tail": stdout[-300:] if stdout else "",
        "stderr_tail": stderr[-300:] if stderr else ""
    }


def clean_sccache(sccache_dir):
    """Stops sccache, empties its directory, restarts and zeroes stats."""
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


def get_dir_size_mb(path):
    """Recursively calculates directory size in MB."""
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


def get_disk_usage_mb(paths):
    """Calculates deduplicated physical disk usage in MB across paths using du -scm."""
    if isinstance(paths, (str, Path)):
        paths = [paths]
    valid_paths = [str(p) for p in paths if Path(p).exists()]
    if not valid_paths:
        return 0.0
    try:
        out = subprocess.check_output(["du", "-scm"] + valid_paths, stderr=subprocess.DEVNULL, text=True)
        total_line = out.strip().splitlines()[-1]
        return float(total_line.split()[0])
    except Exception:
        return round(sum(get_dir_size_mb(p) for p in valid_paths), 1)


def get_binary_version(cwd, project_name, extra_dir=None):
    """Validates parity by checking binary --version output."""
    candidates = [
        Path(cwd) / "target" / "mitts" / "debug" / project_name,
        Path(cwd) / "target" / "mitts" / "debug" / "rg",
        Path(cwd) / ".mitts" / "target" / "debug" / project_name,
        Path(cwd) / "target" / "debug" / project_name,
        Path(cwd) / ".mitts" / "target" / "debug" / "rg",
        Path(cwd) / "target" / "debug" / "rg",
        Path(os.environ.get("SHARED_TARGET_ROOT", Path.home() / "mitts-benchmarks/shared_cargo_target")) / project_name / "debug" / project_name,
        Path(os.environ.get("SHARED_TARGET_ROOT", Path.home() / "mitts-benchmarks/shared_cargo_target")) / project_name / "debug" / "rg",
    ]
    if extra_dir:
        candidates.extend([
            Path(extra_dir) / "debug" / project_name,
            Path(extra_dir) / "debug" / "rg",
        ])
    bin_path = next((c for c in candidates if c.is_file() and os.access(c, os.X_OK)), None)
    if not bin_path:
        for lib_dir in [
            Path(cwd) / "target" / "mitts" / "debug" / "deps",
            Path(cwd) / ".mitts" / "target" / "debug" / "deps",
            Path(cwd) / "target" / "debug" / "deps",
        ]:
            if lib_dir.exists():
                rlibs = list(lib_dir.glob(f"lib{project_name}*.rlib"))
                if rlibs:
                    return f"{project_name} rlib ({rlibs[0].name})"
        return None
    try:
        res = subprocess.run([str(bin_path), "--version"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=5)
        if res.returncode == 0:
            return res.stdout.strip().split("\n")[0]
    except Exception:
        pass
    return None


def get_rig_info():
    """Queries hardware and toolchain metadata dynamically without writing hostname."""
    cpu_model = "AMD Ryzen 9 9950X 16-Core Processor"
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if "model name" in line:
                    cpu_model = line.split(":", 1)[1].strip()
                    break
    except Exception:
        pass

    ram_gb = 192
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if "MemTotal" in line:
                    kb = int(line.split()[1])
                    ram_gb = round(kb / (1024 * 1024))
                    break
    except Exception:
        pass

    os_name = "Linux"
    try:
        with open("/etc/os-release") as f:
            for line in f:
                if line.startswith("PRETTY_NAME="):
                    os_name = line.split("=", 1)[1].strip().strip('"')
                    break
    except Exception:
        pass

    kernel = os.uname().release
    rustc_v = subprocess.check_output(["rustc", "-vV"], text=True).strip().splitlines()[0]
    clippy_v = subprocess.check_output(["cargo", "clippy", "-V"], text=True).strip()

    return {
        "cpu": cpu_model,
        "ram_gb": ram_gb,
        "os": os_name,
        "kernel": kernel,
        "rustc_version": rustc_v,
        "clippy_version": clippy_v,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def get_binary_provenance(mitts_bin: Path):
    """Extracts SHA-256 and git commit information for the Mitts binary."""
    import hashlib
    import re
    sha = ""
    if mitts_bin.exists():
        with open(mitts_bin, "rb") as f:
            sha = hashlib.sha256(f.read()).hexdigest()

    mitts_v = ""
    try:
        mitts_v = subprocess.check_output([str(mitts_bin), "--version"], text=True).strip()
    except Exception:
        pass

    git_c = ""
    m_git = re.search(r"\(([^)]+)\)", mitts_v)
    if m_git:
        git_c = m_git.group(1)

    sccache_v = ""
    try:
        sccache_v = subprocess.check_output([str(SCCACHE_BIN), "--version"], text=True).strip()
    except Exception:
        pass

    return {
        "binary_sha256": sha,
        "mitts_version": mitts_v,
        "git_commit": git_c,
        "relative_path": mitts_bin.name,
        "sccache_version": sccache_v,
    }

