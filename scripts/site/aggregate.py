# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Reduce the raw corpus reports to the public benchmark data set.

Reads the three platform reports of one measured Mitts version and writes
docs/data/benchmarks.json: per-project medians for the headline scenarios,
the per-platform geometric means, and the facts the page states (rig, version,
parameters). Absolute paths, host names and device identifiers never enter the
output. Projects the harness flagged NOISY stay in every aggregate and are
marked in the project table.

    python3 scripts/site/aggregate.py BENCH_DIR OUT.json
"""
import json
import math
import sys
from pathlib import Path

PLATFORMS = [
    ("mac_apfs_arm64.json", "macos-apfs", "macOS · APFS · Apple M2 Ultra"),
    ("rig_xfs_x86_64.json", "linux-xfs-x86", "Linux x86_64 · XFS · AMD Ryzen 9 9950X"),
    ("rama_xfs_aarch64.json", "linux-xfs-arm", "Linux aarch64 · XFS"),
]
ARMS = ["mitts_cas", "cargo_bare", "cargo_sccache", "cargo_shared"]


def med(x):
    if isinstance(x, dict):
        return x.get("median")
    return x


def med_of(xs):
    xs = sorted(x for x in xs if x is not None)
    return xs[len(xs) // 2] if xs else None


def pick(o, *path):
    for k in path:
        if not isinstance(o, dict) or k not in o:
            return None
        o = o[k]
    return o


def geomean(vals):
    vals = [v for v in vals if v and v > 0]
    return math.exp(sum(math.log(v) for v in vals) / len(vals)) if vals else None


METRICS = ["wall", "cpu", "disk", "runs", "written", "rss_max", "rss_sum"]


def swarm_metrics(arms):
    """Per arm, the 12-worktree swarm in four measures: wall seconds, CPU seconds (user + system),
    peak disk MB, compiler runs, plus MB written to the device."""
    out = {}
    for a in ARMS:
        o = arms.get(a) or {}
        user, sys_ = med(o.get("total_user_stats_s")), med(o.get("total_sys_stats_s"))
        out[a] = {
            "wall": med(o.get("wall_stats_s")),
            "cpu": (user or 0) + (sys_ or 0) if user is not None else None,
            "disk": o.get("max_disk_mb"),
            "runs": o.get("units_compiled_median"),
            "written": o.get("median_device_mb_written"),
            # Peak resident memory of the heaviest child process (ru_maxrss) in one worktree, and the
            # sum of those peaks over the 12 worktrees (median of the repeats). A detached sccache
            # server is outside the measured process tree.
            "rss_max": o.get("max_wt_rss_mb"),
            "rss_sum": med_of([(it.get("resources") or {}).get("sum_wt_rss_mb") for it in o.get("iterations", [])]),
        }
    return out


def project_row(name, pr):
    st = pr["stages"]
    row = {"name": name, "status": pr["status"], "units": pick(st, "t0_explain", "units")}
    t5 = pick(st, "t5_worktree_swarm", "arms") or {}
    row["swarm_m"] = swarm_metrics(t5)
    row["swarm_s"] = {a: med(pick(t5, a, "wall_stats_s")) for a in ARMS}
    row["swarm_written_mb"] = {a: pick(t5, a, "median_device_mb_written") for a in ARMS}
    s1 = pick(st, "t2_cold_noop_and_l3", "s1_cold_build") or {}
    row["cold_s"] = {a: med(pick(s1, a, "wall_stats_s")) for a in ARMS + ["cargo_bare_no_incr"]}
    # "Build again, nothing changed". The harness passes an explicit --cache-dir, which turns Mitts' fast
    # lock off; a default `mitts build` has it on. noop_ms is the default behaviour (fast lock for Mitts),
    # noop_cache_dir_ms keeps the explicit-cache-dir figure.
    noop = {a: med(v) for a, v in (pick(st, "t2_cold_noop_and_l3", "s2_fast_noop_ms") or {}).items()}
    fast = {a: med(v) for a, v in (pick(st, "t2_cold_noop_and_l3", "s2_fast_lock_noop_ms") or {}).items()}
    row["noop_cache_dir_ms"] = dict(noop)
    if fast.get("mitts_cas") is not None:
        noop["mitts_cas"] = fast["mitts_cas"]
    row["noop_ms"] = noop
    l3 = pick(st, "t2_cold_noop_and_l3", "l3_mutation_and_reuse") or {}
    row["edit_s"] = {a: pick(l3, a, "l3b_in_wt_edit_s") for a in ARMS}
    row["other_worktree_s"] = {a: pick(l3, a, "l3c_cross_wt_reuse_s") for a in ARMS}
    s3 = pick(st, "t3_history_and_hopping", "s3_git_history_replay") or {}
    row["history_s"] = {a: pick(s3, a, "cumulative_wall_s") for a in ARMS}
    s4 = pick(st, "t3_history_and_hopping", "s4_branch_hopping") or {}
    row["hop_s"] = {a: med(pick(s4, a, "switch_to_b_stats_s")) for a in ARMS}
    row["check_s"] = {a: med(pick(st, "t4_check_clippy_and_cow", "l5a_check_swarm", a, "wall_stats_s")) for a in ARMS}
    row["clippy_s"] = {a: med(pick(st, "t4_check_clippy_and_cow", "l5b_clippy_swarm", a, "wall_stats_s")) for a in ARMS}
    t4 = st.get("t4_check_clippy_and_cow") or {}
    row["reflink_written_mb"] = t4.get("cow_median_device_mb_written")
    row["copy_written_mb"] = t4.get("copy_median_device_mb_written")
    t0 = st.get("t0_explain") or {}
    row["explain_ms"] = med(t0.get("explain_ms"))
    row["cargo_metadata_ms"] = med(t0.get("cargo_metadata_ms"))
    inv = pick(st, "t5_worktree_swarm", "invariants") or {}
    row["swarm_sound"] = inv.get("all_t5_sound_and_parity_ok")
    row["shared_target_unsound"] = inv.get("cargo_shared_unsound_observed")
    return row


SCENARIOS = {
    "swarm_s": "12 parallel worktrees, build the tests",
    "cold_s": "one cold build",
    "noop_ms": "build again, nothing changed",
    "edit_s": "edit one file, rebuild",
    "other_worktree_s": "same code in a second worktree",
    "history_s": "replay 10 commits in one directory",
    "hop_s": "switch branch and build",
    "check_s": "12 worktrees, cargo check equivalent",
    "clippy_s": "12 worktrees, clippy",
}


def ratios(rows, scenario, base):
    out = []
    for r in rows:
        m, b = r[scenario].get("mitts_cas"), r[scenario].get(base)
        if m and b:
            out.append(b / m)
    return out


def main(src, dst):
    out = {"scenarios": SCENARIOS, "platforms": []}
    for fn, pid, label in PLATFORMS:
        d = json.loads((Path(src) / fn).read_text())
        prov, rig = d["provenance"], d["provenance"].get("rig", {})
        rows = [project_row(n, p) for n, p in sorted(d["projects"].items())]
        # Every measured project counts. A NOISY status only records that the repeats of a swarm
        # differed by more than 15%; the data stay in every aggregate.
        ok = [r for r in rows if any(v for v in r["swarm_s"].values())]
        summary = {}
        for sc in SCENARIOS:
            summary[sc] = {}
            for base in ("cargo_sccache", "cargo_bare"):
                rs = ratios(ok, sc, base)
                if rs:
                    rs.sort()
                    summary[sc][base] = {
                        "geomean": geomean(rs), "median": rs[len(rs) // 2], "n": len(rs),
                        "faster": sum(1 for x in rs if x > 1.0), "slower": sum(1 for x in rs if x < 1.0),
                        "min": rs[0], "max": rs[-1],
                    }
        complete = [r for r in ok if all(r["swarm_m"][a][m] is not None for a in ARMS[:3] for m in METRICS)]
        totals = {m: {a: sum(r["swarm_m"][a][m] for r in complete) for a in ARMS[:3]} for m in METRICS}
        out["platforms"].append({
            "id": pid, "label": label, "projects_total": len(rows), "projects_ok": len(ok),
            "totals": totals, "totals_n": len(complete),
            "noisy": [r["name"] for r in rows if r["status"] != "OK"],
            "mitts_version": prov["mitts_version"].split(" (")[0], "commit": prov["git_commit"],
            "binary_sha256": prov["binary_sha256"], "sccache": prov.get("sccache_version"),
            "timestamp": prov.get("timestamp"),
            "rig": {k: rig.get(k) for k in ("cpu", "logical_cpus", "ram_gb", "os", "default_rustc_version")},
            "filesystem": d.get("provenance", {}).get("storage", {}).get("worktree_filesystem"),
            "parameters": d["parameters"], "summary": summary, "projects": rows,
        })
    Path(dst).parent.mkdir(parents=True, exist_ok=True)
    Path(dst).write_text(json.dumps(out, indent=1, sort_keys=False))
    for p in out["platforms"]:
        print(p["label"], p["projects_ok"], "/", p["projects_total"], "NOISY:", p["noisy"])
        for sc, v in p["summary"].items():
            s = v.get("cargo_sccache")
            c = v.get("cargo_bare")
            f = lambda x: "%.2fx(%d↑/%d↓)" % (x["geomean"], x["faster"], x["slower"]) if x else "-"
            print("   %-18s vs sccache %-18s vs cargo %s" % (sc, f(s), f(c)))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
