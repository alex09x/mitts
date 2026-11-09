# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""The scoreboard and per-project charts of The Hundred (inline SVG, colours from CSS classes)."""
import math
import statistics

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



def f_dur(s):
    """A duration in seconds, truncated (never rounded up): '20 ms', '3.4 s', '56 s', '5 min 22 s', '8.9 h'."""
    if s < 1:
        return "%d ms" % math.floor(s * 1000)
    if s < 10:
        return "%.1f s" % (math.floor(s * 10) / 10)
    s = int(s)
    if s < 120:
        return "%d s" % s
    if s < 7200:
        return "%d min %02d s" % (s // 60, s % 60)
    return "%.1f h" % (math.floor(s / 360) / 10)


def f_mb(mb):
    """Megabytes, truncated: '812 MB', '9.9 GB', '641 GB', '1.2 TB'."""
    if mb >= 1e6:
        return "%.1f TB" % (math.floor(mb / 1e5) / 10)
    if mb >= 1e4:
        return "%d GB" % math.floor(mb / 1000)
    if mb >= 1000:
        return "%.1f GB" % (math.floor(mb / 100) / 10)
    return "%d MB" % math.floor(mb)


def f_n(n):
    return "{:,.0f}".format(math.floor(n))


FORMAT = {"wall": f_dur, "cpu": f_dur, "runs": f_n, "written": f_mb, "disk": f_mb}


def up_dur(s):
    """The same precision as f_dur, rounded up: Mitts is never shown faster than it was measured."""
    if s >= 120:
        return "%d min %02d s" % divmod(math.ceil(s - 1e-9), 60) if s < 7200 else "%.1f h" % (math.ceil(s / 360 - 1e-9) / 10)
    if s < 1:
        ms = math.ceil(s * 1000 - 1e-9)
        return "%d ms" % ms if ms < 1000 else "1.0 s"
    if s < 10:
        tenths = math.ceil(s * 10 - 1e-9)
        return "%.1f s" % (tenths / 10) if tenths < 100 else "10 s"
    return "%d s" % math.ceil(s - 1e-9)


def up_mb(mb):
    if mb >= 1e6:
        return "%.1f TB" % (math.ceil(mb / 1e5 - 1e-9) / 10)
    if mb >= 1e4:
        return "%d GB" % math.ceil(mb / 1000 - 1e-9)
    if mb >= 1000:
        return "%.1f GB" % (math.ceil(mb / 100 - 1e-9) / 10)
    return "%d MB" % math.ceil(mb - 1e-9)


UP = {"wall": up_dur, "cpu": up_dur, "runs": lambda n: "{:,.0f}".format(math.ceil(n - 1e-9)), "written": up_mb, "disk": up_mb}


def fmt_arm(metric, arm, v):
    """One value as printed: baselines truncated, Mitts rounded up, so a printed claim never exceeds the measurement."""
    return (UP if arm == "mitts_cas" else FORMAT)[metric](v)

# per measure: (threshold, size of one unit, suffix, decimals), the first row whose threshold the largest value reaches
UNITS = {
    "wall": [(7200, 3600.0, " h", 1), (120, 60.0, " min", 0), (0, 1.0, " s", 0)],
    "runs": [(0, 1.0, "", 0)],
    "written": [(1e6, 1e6, " TB", 1), (1e4, 1e3, " GB", 0), (1e3, 1e3, " GB", 1), (0, 1.0, " MB", 0)],
}
UNITS["cpu"] = UNITS["wall"]
UNITS["disk"] = UNITS["written"]


def board_row(key, vals):
    """Texts of one scoreboard row in one common unit. Baselines are truncated and Mitts is rounded up, so every claimed
    gain is at most the measured one and the saving printed beside the bars is exactly the difference of the printed values.
    Returns ({arm: text}, saving against Cargo, saving against sccache)."""
    mx = max(vals.values())
    unit, sfx, d = next((u, s, dd) for th, u, s, dd in UNITS[key] if mx >= th)
    k = 10 ** d
    q = {a: (math.ceil(v / unit * k - 1e-9) if a == "mitts_cas" else math.floor(v / unit * k + 1e-9)) for a, v in vals.items()}

    def num(n):
        return "{:,.{p}f}".format(max(n, 0) / k, p=d) + sfx

    return {a: num(n) for a, n in q.items()}, num(q["cargo_bare"] - q["mitts_cas"]), num(q["cargo_sccache"] - q["mitts_cas"])


def median(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def scoreboard(p):
    """Five measures, three bars each, linear scale: the length of a bar is what it cost."""
    w, left, span, rx, row = 1000, 250, 400, 790, 96
    h = 54 + row * len(ROWS)
    out = ['<svg class="chart board" viewBox="0 0 %d %d" role="img" aria-label="%s: Cargo, Cargo with sccache and Mitts, totals over %d projects">'
           % (w, h, esc(p["label"]), p["totals_n"])]
    legend_row(out, left, 10)
    for i, (key, title, sub) in enumerate(ROWS):
        y0 = 46 + i * row
        vals = {a: p["totals"][key][a] for a, _, _ in ARMS}
        top = max(vals.values()) or 1
        txt, save_c, save_s = board_row(key, vals)
        out.append('<text class="lab-big" x="0" y="%d">%s</text><text class="ax" x="0" y="%d">%s</text>' % (y0 + 20, esc(title), y0 + 40, esc(sub)))
        for k, (a, name, cls) in enumerate(ARMS):
            bw = max(3.0, vals[a] / top * span)
            by = y0 + k * 26
            out.append('<rect class="%s" x="%d" y="%d" width="%.1f" height="20" rx="3"><title>%s: %s</title></rect>' % (cls, left, by, bw, esc(name), txt[a]))
            out.append('<text class="val%s" x="%.1f" y="%d">%s</text>' % (" mitts" if a == "mitts_cas" else "", left + bw + 8, by + 15, txt[a]))
        out.append('<text class="big" x="%d" y="%d">%s</text>' % (rx, y0 + 38, save_c))
        out.append('<text class="ax" x="%d" y="%d">less than Cargo</text>' % (rx, y0 + 56))
        out.append('<text class="ax" x="%d" y="%d">%s less than sccache</text>' % (rx, y0 + 72, save_s))
    out.append("</svg>")
    return "".join(out)


def legend_row(out, x=270, y=8):
    for _, name, cls in ARMS:
        out.append('<rect class="%s" x="%d" y="%d" width="14" height="14" rx="3"/><text class="ax" x="%d" y="%d">%s</text>' % (cls, x, y, x + 22, y + 12, esc(name)))
        x += 150 if name != "Cargo + sccache" else 190


def groups(label, rows, left=270, span=520):
    """Rows of (title, [note lines], {arm: value}): three bars per row, zero-based, linear, each row on its own scale,
    the measured value written at the end of every bar."""
    bar, gap, top = 15, 4, 36
    row = 3 * (bar + gap) + 14
    out = ['<svg class="chart times" viewBox="0 0 1000 %d" role="img" aria-label="%s">' % (top + row * len(rows), esc(label))]
    legend_row(out, left)
    for i, (name, notes, vals) in enumerate(rows):
        y0 = top + i * row
        top_v = max(vals.values()) or 1
        out.append('<text class="lab-big" x="0" y="%d">%s</text>' % (y0 + 17, esc(name)))
        for j, line in enumerate(notes):
            out.append('<text class="ax" x="0" y="%d">%s</text>' % (y0 + 36 + j * 16, esc(line)))
        for k, (a, aname, cls) in enumerate(ARMS):
            v = vals[a]
            bw = max(3.0, v / top_v * span)
            by = y0 + k * (bar + gap)
            out.append('<rect class="%s" x="%d" y="%d" width="%.1f" height="%d" rx="3"><title>%s: %s</title></rect>' % (cls, left, by, bw, bar, esc(aname), fmt_arm("wall", a, v)))
            out.append('<text class="val%s" x="%.1f" y="%d">%s</text>' % (" mitts" if a == "mitts_cas" else "", left + bw + 8, by + 12, fmt_arm("wall", a, v)))
    out.append("</svg>")
    return "".join(out)


def project_times(p, count=8):
    """How long one project takes to build 12 worktrees: the median project (each tool's own median over the machine's
    projects) and the slowest ones."""
    rows = []
    typ = {a: median([r["swarm_s"].get(a) for r in p["projects"]]) for a, _, _ in ARMS}
    if all(v is not None for v in typ.values()):
        rows.append(("Median project", ["each tool's own median", "over %d projects" % p["projects_ok"]], typ))
    slow = sorted((r for r in p["projects"] if all(r["swarm_s"].get(a) is not None for a, _, _ in ARMS)), key=lambda r: -r["swarm_s"]["cargo_bare"])[:count]
    rows += [(r["name"], ["slowest with Cargo"] if i == 0 else [], {a: r["swarm_s"][a] for a, _, _ in ARMS}) for i, r in enumerate(slow)]
    return groups("%s: time to build 12 worktrees, Cargo, Cargo with sccache and Mitts" % p["label"], rows)


# scenario key, title, what it is, unit divisor to seconds
SCENARIOS = [
    ("swarm_s", "12 worktrees at once", "build the tests", 1), ("other_worktree_s", "Second worktree", "after the first", 1),
    ("hop_s", "Switch branch", "and build", 1), ("check_s", "12 worktrees: check", "", 1), ("clippy_s", "12 worktrees: clippy", "", 1),
    ("noop_ms", "Rebuild, no changes", "nothing to do", 1000), ("edit_s", "Edit one file, rebuild", "", 1),
    ("history_s", "Replay 10 commits", "in one directory", 1), ("cold_s", "One cold build", "empty store", 1),
]


def scenario_times(p):
    """Every scenario in real time: each tool's median over the projects that have all three tools, and in how many
    of those projects Mitts finished first (the medians of the three tools need not come from the same project)."""
    rows = []
    for key, title, note, div in SCENARIOS:
        have = [r for r in p["projects"] if all(r[key].get(a) is not None for a, _, _ in ARMS)]
        if not have:
            continue
        vals = {a: median([r[key][a] / div for r in have]) for a, _, _ in ARMS}
        first = sum(1 for r in have if r[key]["mitts_cas"] < r[key]["cargo_bare"])
        rows.append((title, [("%s · " % note if note else "") + "median of %d" % len(have), "Mitts first on %d of %d" % (first, len(have))], vals))
    return groups("%s: median project, time per scenario" % p["label"], rows)


def project_bars(p, metric):
    """One row per project, longest Cargo first: a Cargo bar and a Mitts bar from zero, with the values written out."""
    pts =[(r["name"], r["swarm_m"]["cargo_bare"][metric], r["swarm_m"]["mitts_cas"][metric]) for r in p["projects"]
           if r["swarm_m"]["cargo_bare"][metric] is not None and r["swarm_m"]["mitts_cas"][metric] is not None]
    pts.sort(key=lambda t: -t[1])
    top_v = max([max(c, m) for _, c, m in pts] or [1]) or 1
    left, span, top, row = 150, 620, 40, 22
    out = ['<svg class="chart bars" viewBox="0 0 1000 %d" role="img" aria-label="Per project, Cargo and Mitts, %s">' % (top + row * len(pts), esc(metric))]
    out.append('<rect class="c-cargo" x="%d" y="8" width="14" height="14" rx="3"/><text class="ax" x="%d" y="20">Cargo</text><rect class="c-mitts" x="%d" y="8" width="14" height="14" rx="3"/><text class="ax" x="%d" y="20">Mitts</text>' % (left, left + 22, left + 110, left + 132))
    out.append('<text class="ax" x="880" y="20" text-anchor="end">Cargo</text><text class="ax" x="990" y="20" text-anchor="end">Mitts</text>')
    for i, (n, c, m) in enumerate(pts):
        y = top + i * row
        if i % 2:
            out.append('<rect class="stripe" x="0" y="%d" width="1000" height="%d"/>' % (y - 5, row))
        out.append('<g><title>%s: Cargo %s, Mitts %s</title><text class="nm" x="%d" y="%d" text-anchor="end">%s</text>' % (esc(n), fmt_arm(metric, "cargo_bare", c), fmt_arm(metric, "mitts_cas", m), left - 10, y + 12, esc(n)))
        out.append('<rect class="c-cargo" x="%d" y="%d" width="%.1f" height="8" rx="2"/><rect class="c-mitts" x="%d" y="%d" width="%.1f" height="8" rx="2"/>' % (left, y, max(1.5, c / top_v * span), left, y + 9, max(1.5, m / top_v * span)))
        out.append('<text class="val" x="880" y="%d" text-anchor="end">%s</text><text class="val mitts" x="990" y="%d" text-anchor="end">%s</text></g>' % (y + 12, fmt_arm(metric, "cargo_bare", c), y + 12, fmt_arm(metric, "mitts_cas", m)))
    out.append("</svg>")
    return "".join(out)


def method_flow():
    """What was measured, drawn as a flow: corpus, scenario, three tools, five readings."""
    out = ['<svg class="diagram flow" viewBox="0 0 1000 250" role="img" aria-label="Method: 100 projects, 12 worktrees on 4 commits, three tools, five readings">']
    boxes = [(156, "100 projects", "open-source Rust", "crates.io and GitHub"),
             (176, "12 worktrees", "4 commits, tests built", "at the same moment"),
             (176, "3 tools", "Cargo, Cargo + sccache,", "Mitts; same machine"),
             (176, "3 repeats", "median of each reading", "idle machine each run"),
             (176, "5 readings", "time, CPU, runs,", "disk written, disk used")]
    x = 0
    for i, (wd, t, a, b) in enumerate(boxes):
        out.append('<rect class="d-box" x="%d" y="40" width="%d" height="120" rx="8"/>' % (x, wd))
        out.append('<text class="d-t" x="%d" y="78">%s</text><text class="d-s" x="%d" y="104">%s</text><text class="d-s" x="%d" y="124">%s</text>' % (x + 14, esc(t), x + 14, esc(a), x + 14, esc(b)))
        if i:
            out.append('<path class="d-arrow2" d="M%d 100 h20 m-7 -6 l7 6 l-7 6"/>' % (x - 28))
        x += wd + 35
    out.append('<text class="d-s" x="0" y="196">Worktree 12 adds a failing test. Untimed smoke runs after the build check that worktree 12 reports it and a clean worktree does not.</text>')
    out.append('<text class="d-s" x="0" y="218">A shared cache that hides the failure, or leaks it into clean worktrees, is unsound and is not scored.</text>')
    out.append("</svg>")
    return "".join(out)
