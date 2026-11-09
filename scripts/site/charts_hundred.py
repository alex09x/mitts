# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""The scoreboard and per-project charts of The Hundred (inline SVG, colours from CSS classes)."""
import math

from shell import esc

ARMS = [("cargo_bare", "Cargo", "c-cargo"), ("cargo_sccache", "Cargo + sccache", "c-scc"), ("mitts_cas", "Mitts", "c-mitts")]

# key, title, what it counts, value formatter name
ROWS = [
    ("wall", "Time", "12 worktrees building at once"),
    ("cpu", "CPU", "processor time, user + system"),
    ("runs", "Compiler runs", "rustc invocations"),
    ("written", "Disk written", "bytes reaching the device"),
    ("disk", "Disk in use", "peak, worktrees plus stores"),
]


def f_time(s):
    if s < 120:
        return "%.0f s" % s
    if s < 7200:
        return "%.0f min" % (s / 60)
    return "%.1f h" % (s / 3600)


def f_exact(s):
    """'34 s', '5 min 12 s': per-project times keep the seconds so close bars stay distinguishable."""
    s = int(s)
    return "%d s" % s if s < 120 else "%d min %02d s" % (s // 60, s % 60)


def f_cpu(s):
    return "%.0f h" % (s / 3600) if s >= 7200 else f_time(s)


def f_mb(mb):
    if mb >= 1e6:
        return "%.1f TB" % (mb / 1e6)
    if mb >= 1000:
        return "%.0f GB" % (mb / 1000) if mb >= 1e4 else "%.1f GB" % (mb / 1000)
    return "%.0f MB" % mb


def f_n(n):
    return "{:,.0f}".format(n)


FORMAT = {"wall": f_time, "cpu": f_cpu, "runs": f_n, "written": f_mb, "disk": f_mb}


def scoreboard(p):
    """Five measures, three bars each, linear scale: the length of a bar is what it cost."""
    w, left, span, rx, row = 1000, 250, 400, 790, 96
    h = 54 + row * len(ROWS)
    out = ['<svg class="chart board" viewBox="0 0 %d %d" role="img" aria-label="%s: Cargo, Cargo with sccache and Mitts, totals over %d projects">'
           % (w, h, esc(p["label"]), p["totals_n"])]
    x = left
    for _, name, cls in ARMS:
        out.append('<rect class="%s" x="%d" y="10" width="14" height="14" rx="3"/><text class="ax" x="%d" y="22">%s</text>' % (cls, x, x + 22, esc(name)))
        x += 150 if name != "Cargo + sccache" else 190
    for i, (key, title, sub) in enumerate(ROWS):
        y0 = 46 + i * row
        vals = {a: p["totals"][key][a] for a, _, _ in ARMS}
        top = max(vals.values()) or 1
        fmt = FORMAT[key]
        out.append('<text class="lab-big" x="0" y="%d">%s</text><text class="ax" x="0" y="%d">%s</text>' % (y0 + 20, esc(title), y0 + 40, esc(sub)))
        for k, (a, name, cls) in enumerate(ARMS):
            v = vals[a]
            bw = max(3.0, v / top * span)
            by = y0 + k * 26
            out.append('<rect class="%s" x="%d" y="%d" width="%.1f" height="20" rx="3"><title>%s: %s</title></rect>' % (cls, left, by, bw, esc(name), fmt(v)))
            out.append('<text class="val%s" x="%.1f" y="%d">%s</text>' % (" mitts" if a == "mitts_cas" else "", left + bw + 8, by + 15, fmt(v)))
        out.append('<text class="big" x="%d" y="%d">%s</text>' % (rx, y0 + 38, saved(key, vals["cargo_bare"] - vals["mitts_cas"])))
        out.append('<text class="ax" x="%d" y="%d">less than Cargo</text>' % (rx, y0 + 56))
        out.append('<text class="ax" x="%d" y="%d">%s less than sccache</text>' % (rx, y0 + 72, saved(key, vals["cargo_sccache"] - vals["mitts_cas"])))
    out.append("</svg>")
    return "".join(out)


def saved(key, d):
    """The absolute saving in the metric's own unit, truncated: '63 min', '84,056', '530 GB'."""
    if key in ("wall", "cpu"):
        if d < 120:
            return "%d s" % math.floor(d)
        if d < 7200:
            return "%d min" % math.floor(d / 60)
        return "%.1f h" % (math.floor(d / 360) / 10)
    if key == "runs":
        return f_n(math.floor(d))
    if d >= 1e6:
        return "%.1f TB" % (math.floor(d / 1e5) / 10)
    return "%d GB" % math.floor(d / 1000) if d >= 1000 else "%d MB" % math.floor(d)


def median(xs):
    xs = sorted(x for x in xs if x is not None)
    return xs[len(xs) // 2] if xs else None

def f_dur(s):
    """A duration in seconds, truncated: '20 ms', '3.4 s', '56 s', '5 min 22 s'."""
    if s < 1:
        return "%d ms" % math.floor(s * 1000)
    if s < 10:
        return "%.1f s" % (math.floor(s * 10) / 10)
    return f_exact(s)


def legend_row(out, x=270, y=8):
    for _, name, cls in ARMS:
        out.append('<rect class="%s" x="%d" y="%d" width="14" height="14" rx="3"/><text class="ax" x="%d" y="%d">%s</text>' % (cls, x, y, x + 22, y + 12, esc(name)))
        x += 150 if name != "Cargo + sccache" else 190


def groups(label, rows, left=270, span=520):
    """Rows of (title, note, {arm: value}, formatter): three bars per row, zero-based, linear, each row on its own scale,
    the measured value written at the end of every bar."""
    bar, gap, top = 15, 4, 36
    row = 3 * (bar + gap) + 14
    out = ['<svg class="chart times" viewBox="0 0 1000 %d" role="img" aria-label="%s">' % (top + row * len(rows), esc(label))]
    legend_row(out, left)
    for i, (name, note, vals, fmt) in enumerate(rows):
        y0 = top + i * row
        top_v = max(vals.values()) or 1
        out.append('<text class="lab-big" x="0" y="%d">%s</text>' % (y0 + 17, esc(name)))
        if note:
            out.append('<text class="ax" x="0" y="%d">%s</text>' % (y0 + 36, esc(note)))
        for k, (a, aname, cls) in enumerate(ARMS):
            v = vals[a]
            bw = max(3.0, v / top_v * span)
            by = y0 + k * (bar + gap)
            out.append('<rect class="%s" x="%d" y="%d" width="%.1f" height="%d" rx="3"><title>%s: %s</title></rect>' % (cls, left, by, bw, bar, esc(aname), fmt(v)))
            out.append('<text class="val%s" x="%.1f" y="%d">%s</text>' % (" mitts" if a == "mitts_cas" else "", left + bw + 8, by + 12, fmt(v)))
    out.append("</svg>")
    return "".join(out)


def project_times(p, count=8):
    """How long one project takes to build 12 worktrees: a typical project (median over the machine's
    projects) and the slowest ones."""
    rows = []
    typ = {a: median([r["swarm_s"].get(a) for r in p["projects"]]) for a, _, _ in ARMS}
    if all(typ.values()):
        rows.append(("A typical project", "median of %d" % p["projects_ok"], typ, f_dur))
    slow = sorted((r for r in p["projects"] if all(r["swarm_s"].get(a) for a, _, _ in ARMS)), key=lambda r: -r["swarm_s"]["cargo_bare"])[:count]
    rows += [(r["name"], "slowest by Cargo" if i == 0 else "", {a: r["swarm_s"][a] for a, _, _ in ARMS}, f_dur) for i, r in enumerate(slow)]
    return groups("%s: time to build 12 worktrees, Cargo, Cargo with sccache and Mitts" % p["label"], rows)


# scenario key, title, what it is, unit divisor to seconds
SCENARIOS = [
    ("swarm_s", "12 worktrees at once", "build the tests", 1), ("other_worktree_s", "Second worktree", "after the first", 1),
    ("hop_s", "Switch branch", "and build", 1), ("check_s", "12 worktrees: check", "", 1), ("clippy_s", "12 worktrees: clippy", "", 1),
    ("noop_ms", "Rebuild, no changes", "nothing to do", 1000), ("edit_s", "Edit one file, rebuild", "", 1),
    ("history_s", "Replay 10 commits", "in one directory", 1), ("cold_s", "One cold build", "empty store", 1),
]


def scenario_times(p):
    """Every scenario for a typical project (the median over the projects that have all three tools), in real time."""
    rows = []
    for key, title, note, div in SCENARIOS:
        have = [r for r in p["projects"] if all(r[key].get(a) for a, _, _ in ARMS)]
        if not have:
            continue
        vals = {a: median([r[key][a] / div for r in have]) for a, _, _ in ARMS}
        rows.append((title, ("%s, " % note if note else "") + "median of %d" % len(have), vals, f_dur))
    return groups("%s: typical project, time per scenario" % p["label"], rows)


def project_bars(p, metric):
    """One row per project, longest Cargo first: a Cargo bar and a Mitts bar from zero, with the values written out."""
    fmt = f_dur if metric == "wall" else FORMAT[metric]
    pts = [(r["name"], r["swarm_m"]["cargo_bare"][metric], r["swarm_m"]["mitts_cas"][metric]) for r in p["projects"]
           if r["swarm_m"]["cargo_bare"][metric] and r["swarm_m"]["mitts_cas"][metric] is not None]
    pts.sort(key=lambda t: -t[1])
    top_v = pts[0][1] if pts else 1
    left, span, top, row = 150, 560, 40, 22
    out = ['<svg class="chart bars" viewBox="0 0 1000 %d" role="img" aria-label="Per project, Cargo and Mitts, %s">' % (top + row * len(pts), esc(metric))]
    out.append('<rect class="c-cargo" x="%d" y="8" width="14" height="14" rx="3"/><text class="ax" x="%d" y="20">Cargo</text><rect class="c-mitts" x="%d" y="8" width="14" height="14" rx="3"/><text class="ax" x="%d" y="20">Mitts</text>' % (left, left + 22, left + 110, left + 132))
    out.append('<text class="ax" x="%d" y="20" text-anchor="end">Cargo</text><text class="ax" x="990" y="20" text-anchor="end">Mitts</text>' % 880)
    for i, (n, c, m) in enumerate(pts):
        y = top + i * row
        out.append('<g><title>%s: Cargo %s, Mitts %s</title><text class="nm" x="%d" y="%d" text-anchor="end">%s</text>' % (esc(n), fmt(c), fmt(m), left - 10, y + 12, esc(n)))
        out.append('<rect class="c-cargo" x="%d" y="%d" width="%.1f" height="8" rx="2"/><rect class="c-mitts" x="%d" y="%d" width="%.1f" height="8" rx="2"/>' % (left, y, max(1.5, c / top_v * span), left, y + 9, max(1.5, m / top_v * span)))
        out.append('<text class="val" x="880" y="%d" text-anchor="end">%s</text><text class="val mitts" x="990" y="%d" text-anchor="end">%s</text></g>' % (y + 12, fmt(c), y + 12, fmt(m)))
    out.append("</svg>")
    return "".join(out)


def method_flow():
    """What was measured, drawn as a flow: corpus, scenario, three tools, five readings."""
    out = ['<svg class="diagram flow" viewBox="0 0 1000 250" role="img" aria-label="Method: 100 projects, 12 worktrees on 4 commits, three tools, five readings">']
    boxes = [(140, "100 projects", "open-source Rust", "crates.io and GitHub"),
             (180, "12 worktrees", "4 commits, tests built", "at the same moment"),
             (180, "3 tools", "Cargo, Cargo + sccache,", "Mitts; same machine"),
             (180, "3 repeats", "median of each reading", "idle machine each run"),
             (160, "5 readings", "time, CPU, runs,", "disk written, disk used")]
    x = 0
    for i, (wd, t, a, b) in enumerate(boxes):
        out.append('<rect class="d-box" x="%d" y="40" width="%d" height="120" rx="8"/>' % (x, wd))
        out.append('<text class="d-t" x="%d" y="78">%s</text><text class="d-s" x="%d" y="104">%s</text><text class="d-s" x="%d" y="124">%s</text>' % (x + 14, esc(t), x + 14, esc(a), x + 14, esc(b)))
        if i:
            out.append('<path class="d-arrow2" d="M%d 100 h24 m-7 -6 l7 6 l-7 6"/>' % (x - 32))
        x += wd + 40
    out.append('<text class="d-s" x="0" y="196">Worktree 12 adds a failing test. Untimed smoke runs after the build check that worktree 12 reports it and a clean worktree does not.</text>')
    out.append('<text class="d-s" x="0" y="218">A shared cache that hides the failure, or leaks it into clean worktrees, is unsound and is not scored.</text>')
    out.append("</svg>")
    return "".join(out)
