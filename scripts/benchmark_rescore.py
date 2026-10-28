#!/usr/bin/env python3
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Re-evaluate the T4 status gate of a finished results file from its stored evidence.

The swarm measurements, per-iteration failure counts, exit codes and diagnostics parity are
all stored per project; only the invariant rule is recomputed. Projects whose status came
from an ERROR or a SKIPPED_* stay as they are. A `.before-rescore.json` copy is kept.
"""
import json
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from benchmark_lib import evaluate_t4_invariants  # noqa: E402

PRIORITY = ("INVALID_L5", "INVALID_T5", "NOISY")


def rescore(path: Path) -> list[str]:
    data = json.loads(path.read_text())
    changes: list[str] = []
    for name, proj in data.get("projects", {}).items():
        t4 = proj.get("stages", {}).get("t4_check_clippy_and_cow")
        if not t4 or "l5a_check_swarm" not in t4:
            continue
        l5_res = {"check": t4["l5a_check_swarm"], "clippy": t4["l5b_clippy_swarm"]}
        inv = evaluate_t4_invariants(l5_res)
        old_inv = t4.get("invariants", {})
        t4["invariants"] = inv
        flags = [f for f in proj.get("status_flags", []) if f != "INVALID_L5"]
        if not inv["all_t4_sound_and_parity_ok"]:
            flags.insert(0, "INVALID_L5")
        proj["status_flags"] = flags
        if proj.get("status") in ("OK", "NOISY", "INVALID_L5", "INVALID_T5"):
            new_status = next((f for f in PRIORITY if f in flags), "OK")
            if new_status != proj["status"]:
                changes.append(f"{name}: {proj['status']} -> {new_status}")
                proj["status"] = new_status
        if old_inv.get("all_t4_sound_and_parity_ok") != inv["all_t4_sound_and_parity_ok"]:
            changes.append(f"{name}: t4 gate {old_inv.get('all_t4_sound_and_parity_ok')} -> {inv['all_t4_sound_and_parity_ok']}")
    backup = path.with_suffix(".before-rescore.json")
    if not backup.exists():
        shutil.copy2(path, backup)
    path.write_text(json.dumps(data, indent=2))
    return changes


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        out = rescore(Path(arg))
        print(f"{arg}: {len(out)} change(s)")
        for line in out:
            print("  " + line)
