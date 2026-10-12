#
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#

"""
System pressure and block device I/O sampling for defensible benchmarks.
Monitors memory pressure (/proc/meminfo Dirty and Writeback),
PSI I/O stall metrics (/proc/pressure/io), and physical device sectors written.
"""

import os
import re
import subprocess
import threading
import time


def get_current_dirty_mb():
    """Reads current kernel dirty page volume in megabytes."""
    try:
        with open("/proc/meminfo", "r") as f:
            for line in f:
                if line.startswith("Dirty:"):
                    return round(int(line.split()[1]) / 1024.0, 1)
    except Exception:
        pass
    return 0.0


def sync_and_wait_dirty(max_dirty_mb=1024.0, timeout_s=120.0):
    """Syncs filesystem and waits until dirty memory drops below max_dirty_mb."""
    try:
        subprocess.run(["sync"], check=True)
    except Exception:
        pass
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        d_mb = get_current_dirty_mb()
        if d_mb <= max_dirty_mb:
            return d_mb
        time.sleep(0.5)
    return get_current_dirty_mb()


def get_device_sectors_written(target_path):
    """Resolves underlying block device for target_path and reads total sectors written."""
    try:
        src = subprocess.check_output(
            ["findmnt", "-no", "SOURCE", "--target", str(target_path)],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        real_src = os.path.realpath(src)
        dev_name = os.path.basename(real_src)
        stat_file = f"/sys/class/block/{dev_name}/stat"
        if os.path.exists(stat_file):
            with open(stat_file, "r") as f:
                fields = f.read().split()
            return dev_name, int(fields[6])
    except Exception:
        pass
    return None, None


class SystemPressureSampler:
    """Background sampler recording memory pressure and PSI I/O stalls during benchmark swarms."""

    def __init__(self, interval_s=0.5):
        self.interval_s = interval_s
        self._stop_event = threading.Event()
        self._thread = None
        self.samples = []

    def _sample_once(self):
        sample = {
            "time": time.time(),
            "dirty_mb": 0.0,
            "writeback_mb": 0.0,
            "io_some_avg10": 0.0,
            "io_full_avg10": 0.0,
        }
        try:
            with open("/proc/meminfo", "r") as f:
                for line in f:
                    if line.startswith("Dirty:"):
                        sample["dirty_mb"] = round(int(line.split()[1]) / 1024.0, 1)
                    elif line.startswith("Writeback:"):
                        sample["writeback_mb"] = round(int(line.split()[1]) / 1024.0, 1)
        except Exception:
            pass

        try:
            with open("/proc/pressure/io", "r") as f:
                for line in f:
                    if line.startswith("some "):
                        m = re.search(r"avg10=([\d\.]+)", line)
                        if m:
                            sample["io_some_avg10"] = float(m.group(1))
                    elif line.startswith("full "):
                        m = re.search(r"avg10=([\d\.]+)", line)
                        if m:
                            sample["io_full_avg10"] = float(m.group(1))
        except Exception:
            pass
        return sample

    def _run(self):
        while not self._stop_event.is_set():
            self.samples.append(self._sample_once())
            self._stop_event.wait(self.interval_s)

    def start(self):
        self._stop_event.clear()
        self.samples = []
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def stop(self):
        self._stop_event.set()
        if self._thread:
            self._thread.join(timeout=2.0)
        if not self.samples:
            return {}
        dirty_vals = [s["dirty_mb"] for s in self.samples]
        wb_vals = [s["writeback_mb"] for s in self.samples]
        io_some = [s["io_some_avg10"] for s in self.samples]
        io_full = [s["io_full_avg10"] for s in self.samples]
        return {
            "samples_count": len(self.samples),
            "max_dirty_mb": max(dirty_vals) if dirty_vals else 0.0,
            "avg_dirty_mb": round(sum(dirty_vals) / len(dirty_vals), 1) if dirty_vals else 0.0,
            "max_writeback_mb": max(wb_vals) if wb_vals else 0.0,
            "avg_writeback_mb": round(sum(wb_vals) / len(wb_vals), 1) if wb_vals else 0.0,
            "max_io_some_avg10": max(io_some) if io_some else 0.0,
            "avg_io_some_avg10": round(sum(io_some) / len(io_some), 2) if io_some else 0.0,
            "max_io_full_avg10": max(io_full) if io_full else 0.0,
            "avg_io_full_avg10": round(sum(io_full) / len(io_full), 2) if io_full else 0.0,
        }
