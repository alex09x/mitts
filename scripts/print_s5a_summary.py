#!/usr/bin/env python3
import json
from pathlib import Path

path = Path("docs/results_s5a_audit_defensible.json")
with open(path) as f:
    d = json.load(f)

print("=" * 80)
print("MITTS S5a DEFENSIBLE MULTI-WORKTREE CONCURRENCY AUDIT (5 RUNS)")
print("=" * 80)

for p, data in d.items():
    print("\n" + "#" * 60)
    print("PROJECT:", p.upper())
    print("#" * 60)
    
    print("\n[Commit Distribution across 12 Worktrees]")
    dist = {}
    for c in data["commit_distribution"]:
        key = f"{c['commit']} ({c['relative_pos']})"
        dist[key] = dist.get(key, 0) + 1
    for k, v in dist.items():
        print(f"  {v} worktrees at {k}")
    
    print("\n[Wall Clock Time - 5 Iterations]")
    for s in ["mitts", "cargo_bare", "cargo_shared", "sccache"]:
        ws = data[s]["wall_stats"]
        samples_str = ", ".join(f"{x:.3f}s" for x in ws["samples"])
        print(f"  {s:15s}: Median={ws['median']:.3f}s | IQR={ws['iqr']:.3f}s | Min={ws['min']:.3f}s | Max={ws['max']:.3f}s | [{samples_str}]")
    
    print("\n[Peak RSS (ru_maxrss from wait4)]")
    for s in ["mitts", "cargo_bare", "cargo_shared", "sccache"]:
        rs = data[s]["rss_stats"]
        print(f"  {s:15s}: Median={rs['median']:.1f} MB (Min={rs['min']:.1f}, Max={rs['max']:.1f})")

    print("\n[Disk Usage (du -scm, ext4 NVMe)]")
    for s in ["mitts", "cargo_bare", "cargo_shared", "sccache"]:
        ds = data[s]["disk_stats"]
        print(f"  {s:15s}: Median={ds['median']:.1f} MB (Min={ds['min']:.1f}, Max={ds['max']:.1f})")

    cmp = data["comparison_vs_best_baseline"]
    print(f"\n[Comparison vs Best Competitor Baseline]")
    print(f"  Best Baseline: {cmp['best_baseline_name']} ({cmp['best_baseline_time_sec']:.3f}s)")
    print(f"  Mitts Median:  {cmp['mitts_time_sec']:.3f}s")
    print(f"  Ratio:         {cmp['ratio_vs_best']}")

    ua = data["unit_audit"]
    print(f"\n[Unit Hit/Miss Breakdown (Run 1 - Cold target, warm CAS only on HEAD)]")
    print(f"  Total planned: {ua['total_units']}")
    print(f"  Hits from CAS: {ua['hits']} ({ua['hit_pct']}%)")
    print(f"  Total Misses:  {ua['misses']}")
    print(f"  Breakdown:")
    for cat, val in ua["breakdown"].items():
        print(f"    - {cat:25s}: {val}")
