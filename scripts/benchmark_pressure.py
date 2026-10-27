#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
System pressure, physical block-device write accounting, and filesystem allocation
delta helpers for Mitts benchmarks on Linux (XFS/ext4) and macOS (APFS).
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path


def get_current_dirty_mb() -> float | None:
    """Current kernel dirty page volume in MiB on Linux, or None on macOS."""
    if not os.path.exists("/proc/meminfo"):
        return None
    try:
        with open("/proc/meminfo", "r", encoding="utf-8") as f:
            for line in f:
                if line.startswith("Dirty:"):
                    return round(int(line.split()[1]) / 1024.0, 1)
    except Exception:
        pass
    return None


def sync_and_wait_dirty(max_dirty_mb: float = 1024.0, timeout_s: float = 120.0) -> float | None:
    """Runs sync and waits until Linux kernel dirty memory drops below max_dirty_mb."""
    try:
        subprocess.run(["sync"], check=False)
    except Exception:
        pass
    d0 = get_current_dirty_mb()
    if d0 is None:
        return None
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        d_mb = get_current_dirty_mb()
        if d_mb is None or d_mb <= max_dirty_mb:
            return d_mb
        time.sleep(0.5)
    return get_current_dirty_mb()


def _linux_maj_min_for_path(p: Path) -> str | None:
    try:
        st = os.stat(p)
        maj, min_v = os.major(st.st_dev), os.minor(st.st_dev)
        if os.path.exists(f"/sys/dev/block/{maj}:{min_v}/stat"):
            return f"{maj}:{min_v}"
    except Exception:
        pass
    try:
        mm = subprocess.check_output(
            ["findmnt", "-no", "MAJ:MIN", "--target", str(p)],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        if mm and os.path.exists(f"/sys/dev/block/{mm}/stat"):
            return mm
    except Exception:
        pass
    try:
        src = subprocess.check_output(
            ["findmnt", "-no", "SOURCE", "--target", str(p)],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        src = src.split("[", 1)[0]
        real_src = os.path.realpath(src)
        dev_name = os.path.basename(real_src)
        if os.path.exists(f"/sys/class/block/{dev_name}/stat"):
            return f"class:{dev_name}"
    except Exception:
        pass
    return None


_DARWIN_DISKS_CACHE: dict[tuple[str, ...], set[str]] = {}


def _darwin_whole_disks(paths: list[Path]) -> set[str]:
    # `diskutil info` takes about a second per call and the device behind a path does
    # not change during a run, so the answer is kept for the process lifetime.
    key = tuple(str(p) for p in paths)
    if key in _DARWIN_DISKS_CACHE:
        return _DARWIN_DISKS_CACHE[key]
    disks = _darwin_whole_disks_uncached(paths)
    _DARWIN_DISKS_CACHE[key] = disks
    return disks


def _darwin_whole_disks_uncached(paths: list[Path]) -> set[str]:
    disks: set[str] = set()
    for p in paths:
        try:
            df_out = subprocess.check_output(["df", "-P", str(p)], text=True, stderr=subprocess.DEVNULL)
            lines = df_out.strip().splitlines()
            dev_node = lines[-1].split()[0] if len(lines) >= 2 else str(p)
            out = subprocess.check_output(["diskutil", "info", dev_node], text=True, stderr=subprocess.DEVNULL)
            phys = None
            part_whole = None
            for line in out.splitlines():
                if "APFS Physical Store:" in line:
                    m = re.search(r"(disk\d+)", line.split(":", 1)[1])
                    if m:
                        phys = m.group(1)
                elif "Part of Whole:" in line:
                    part_whole = line.split(":", 1)[1].strip()
            if phys:
                disks.add(phys)
            elif part_whole:
                disks.add(part_whole)
        except Exception:
            pass
    return disks


def get_device_bytes_written(paths: list[Path] | Path) -> tuple[str | None, int | None]:
    """Returns (device_id_str, total_bytes_written); see `get_device_io_bytes`."""
    dev, written, _ = get_device_io_bytes(paths)
    return dev, written


def get_device_io_bytes(paths: list[Path] | Path) -> tuple[str | None, int | None, int | None]:
    """
    Returns (device_id_str, total_bytes_written, total_bytes_read) across all unique
    underlying block devices for `paths`. Returns (None, None, None) if unavailable
    (never fake 0).
    """
    if isinstance(paths, (str, Path)):
        paths = [Path(paths)]
    resolved = []
    for p in paths:
        cur = Path(p).resolve()
        while not cur.exists() and cur.parent != cur:
            cur = cur.parent
        resolved.append(cur)

    if sys.platform == "darwin":
        whole_disks = _darwin_whole_disks(resolved)
        if not whole_disks:
            return None, None, None
        try:
            drv_out = subprocess.check_output(
                ["ioreg", "-c", "IOBlockStorageDriver", "-r", "-l", "-d", "3", "-w0"],
                text=True,
                stderr=subprocess.DEVNULL,
            )
            disk_writes: dict[str, int] = {}
            disk_reads: dict[str, int] = {}
            for block in drv_out.split("+-o IOBlockStorageDriver"):
                m_w = re.search(r'"Bytes \(Write\)"\s*=\s*(\d+)', block)
                m_r = re.search(r'"Bytes \(Read\)"\s*=\s*(\d+)', block)
                if not m_w:
                    continue
                for dname in whole_disks:
                    if f'"BSD Name" = "{dname}"' in block:
                        disk_writes[dname] = int(m_w.group(1))
                        disk_reads[dname] = int(m_r.group(1)) if m_r else 0
            if disk_writes:
                return ",".join(sorted(disk_writes.keys())), sum(disk_writes.values()), sum(disk_reads.values())
        except Exception:
            pass
        return None, None, None

    devs: dict[str, int] = {}
    reads: dict[str, int] = {}
    for p in resolved:
        key = _linux_maj_min_for_path(p)
        if not key or key in devs:
            continue
        stat_path = (
            f"/sys/class/block/{key.split(':', 1)[1]}/stat"
            if key.startswith("class:")
            else f"/sys/dev/block/{key}/stat"
        )
        try:
            fields = Path(stat_path).read_text(encoding="utf-8").split()
            devs[key] = int(fields[6]) * 512  # sectors written
            reads[key] = int(fields[2]) * 512  # sectors read
        except Exception:
            pass

    if not devs:
        return None, None, None
    return ",".join(sorted(devs.keys())), sum(devs.values()), sum(reads.values())


def get_df_used_mb(paths: list[Path] | Path) -> float | None:
    """
    Returns physical filesystem used space in MiB via statvfs (`df`), summed across
    unique `st_dev` devices in `paths` so both work_dir and cache_dir are covered.
    """
    if isinstance(paths, (str, Path)):
        paths = [Path(paths)]
    seen_devs: set[int] = set()
    total_used = 0
    for raw_p in paths:
        p = Path(raw_p).resolve()
        while not p.exists() and p.parent != p:
            p = p.parent
        try:
            dev = os.stat(p).st_dev
            if dev in seen_devs:
                continue
            seen_devs.add(dev)
            total_used += shutil.disk_usage(p).used
        except Exception:
            return None
    return round(total_used / (1024.0 * 1024.0), 1) if seen_devs else None


class SystemPressureSampler:
    """Background sampler of dirty/writeback pages and PSI I/O stalls during a swarm."""

    def __init__(self, interval_s: float = 0.5):
        self.interval_s = interval_s
        self._stop_event = threading.Event()
        self._thread: threading.Thread | None = None
        self.samples: list[dict] = []

    def _sample_once(self) -> dict | None:
        if not os.path.exists("/proc/meminfo"):
            return None
        sample = {
            "time": time.time(),
            "dirty_mb": 0.0,
            "writeback_mb": 0.0,
            "io_some_avg10": 0.0,
            "io_full_avg10": 0.0,
            "cpu_some_avg10": 0.0,
        }
        try:
            with open("/proc/meminfo", "r", encoding="utf-8") as f:
                for line in f:
                    if line.startswith("Dirty:"):
                        sample["dirty_mb"] = round(int(line.split()[1]) / 1024.0, 1)
                    elif line.startswith("Writeback:"):
                        sample["writeback_mb"] = round(int(line.split()[1]) / 1024.0, 1)
        except Exception:
            pass
        try:
            with open("/proc/pressure/io", "r", encoding="utf-8") as f:
                for line in f:
                    m = re.search(r"avg10=([\d\.]+)", line)
                    if not m:
                        continue
                    if line.startswith("some "):
                        sample["io_some_avg10"] = float(m.group(1))
                    elif line.startswith("full "):
                        sample["io_full_avg10"] = float(m.group(1))
        except Exception:
            pass
        try:
            with open("/proc/pressure/cpu", "r", encoding="utf-8") as f:
                for line in f:
                    if line.startswith("some "):
                        m = re.search(r"avg10=([\d\.]+)", line)
                        if m:
                            sample["cpu_some_avg10"] = float(m.group(1))
        except Exception:
            pass
        return sample

    def _run(self):
        while not self._stop_event.is_set():
            s = self._sample_once()
            if s is not None:
                self.samples.append(s)
            self._stop_event.wait(self.interval_s)

    def start(self):
        self._stop_event.clear()
        self.samples = []
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def stop(self) -> dict:
        self._stop_event.set()
        if self._thread:
            self._thread.join(timeout=2.0)
        if not self.samples:
            return {
                "samples_count": 0, "max_dirty_mb": None, "avg_dirty_mb": None,
                "max_writeback_mb": None, "avg_writeback_mb": None,
                "max_io_some_avg10": None, "avg_io_some_avg10": None,
                "max_io_full_avg10": None, "avg_io_full_avg10": None,
                "max_cpu_some_avg10": None, "avg_cpu_some_avg10": None,
            }
        dirty_vals = [s["dirty_mb"] for s in self.samples]
        wb_vals = [s["writeback_mb"] for s in self.samples]
        io_some = [s["io_some_avg10"] for s in self.samples]
        io_full = [s["io_full_avg10"] for s in self.samples]
        cpu_some = [s["cpu_some_avg10"] for s in self.samples]

        def avg(vals: list[float]) -> float:
            return round(sum(vals) / len(vals), 2) if vals else 0.0

        return {
            "samples_count": len(self.samples),
            "max_dirty_mb": max(dirty_vals),
            "avg_dirty_mb": round(avg(dirty_vals), 1),
            "max_writeback_mb": max(wb_vals),
            "avg_writeback_mb": round(avg(wb_vals), 1),
            "max_io_some_avg10": max(io_some),
            "avg_io_some_avg10": avg(io_some),
            "max_io_full_avg10": max(io_full),
            "avg_io_full_avg10": avg(io_full),
            "max_cpu_some_avg10": max(cpu_some),
            "avg_cpu_some_avg10": avg(cpu_some),
        }
