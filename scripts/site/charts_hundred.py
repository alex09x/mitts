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


def axis_label(metric, v):
    """Decade tick labels in the metric's own unit, with thousands separators."""
    unit = {"wall": " s", "cpu": " s", "runs": "", "written": " MB", "disk": " MB"}[metric]
    return format(v, ",g") + unit


def fmt_x(v):
    return ("%.1f" % v).rstrip("0").rstrip(".") + "×" if v < 10 else "%d×" % round(v)


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
        m = vals["mitts_cas"] or 1
        out.append('<text class="big" x="%d" y="%d">%s</text>' % (rx, y0 + 38, fmt_x(vals["cargo_bare"] / m)))
        out.append('<text class="ax" x="%d" y="%d">less than Cargo</text>' % (rx, y0 + 56))
        out.append('<text class="ax" x="%d" y="%d">%s less than sccache</text>' % (rx, y0 + 72, fmt_x(vals["cargo_sccache"] / m)))
    out.append("</svg>")
    return "".join(out)


def dumbbell(p, metric, unit):
    """One row per project, sorted by the gain: a grey dot for Cargo, a red dot for Mitts, a log axis."""
    pts = []
    for r in p["projects"]:
        c, m = r["swarm_m"]["cargo_bare"][metric], r["swarm_m"]["mitts_cas"][metric]
        if c and c > 0 and m is not None:
            pts.append((r["name"], c, m, r["status"]))
    if not pts:
        return ""
    pts.sort(key=lambda t: -(t[1] / max(t[2], 1e-9)))
    vals = [v for _, c, m, _ in pts for v in (c, m) if v > 0]
    lo = 10 ** math.floor(math.log10(min(vals)))
    hi = 10 ** math.ceil(math.log10(max(vals)))
    left, right, top, row = 138, 972, 38, 11
    h = top + row * len(pts) + 36

    def X(v):
        v = max(v, lo)
        return left + (math.log(v) - math.log(lo)) / (math.log(hi) - math.log(lo)) * (right - left)

    fmt = FORMAT[metric]
    out = ['<svg class="chart dumb" viewBox="0 0 1000 %d" role="img" aria-label="Per project: Cargo against Mitts, %s">' % (h, esc(metric))]
    d = lo
    while d <= hi * 1.0001:
        out.append('<line class="grid" x1="%.1f" x2="%.1f" y1="%d" y2="%d"/>' % (X(d), X(d), top - 8, h - 30))
        label = axis_label(metric, d)
        out.append('<text class="ax" x="%.1f" y="%d" text-anchor="middle">%s</text>' % (X(d), top - 14, esc(label)))
        out.append('<text class="ax" x="%.1f" y="%d" text-anchor="middle">%s</text>' % (X(d), h - 12, esc(label)))
        d *= 10
    for i, (n, c, m, st) in enumerate(pts):
        y = top + i * row + row / 2
        gain = c / m if m > 0 else float("inf")
        tip = "%s: Cargo %s, Mitts %s%s" % (n, fmt(c), fmt(m), "" if gain == float("inf") else " (%s)" % fmt_x(gain))
        out.append('<g><title>%s</title><text class="nm" x="%d" y="%.1f" text-anchor="end">%s</text>' % (esc(tip), left - 8, y + 3, esc(n)))
        out.append('<line class="link" x1="%.1f" x2="%.1f" y1="%.1f" y2="%.1f"/>' % (X(m), X(c), y, y))
        out.append('<circle class="d-cargo" cx="%.1f" cy="%.1f" r="3.2"/><circle class="d-mitts" cx="%.1f" cy="%.1f" r="3.6"/></g>' % (X(c), y, X(m), y))
    out.append("</svg>")
    return "".join(out)


def swarm_picture(avg_cargo_runs, avg_mitts_runs):
    """An illustration, not a measurement: twelve worktrees, one lane each. Cargo compiles in every lane;
    Mitts compiles once and restores the rest. The two numbers are the measured per-project means."""
    w, left, lane, gap = 1000, 120, 14, 5
    blocks = 46
    cell = (w - left - 20) / blocks
    out = ['<svg class="diagram swarm" viewBox="0 0 1000 610" role="img" aria-label="Twelve worktrees: Cargo compiles in all of them, Mitts compiles once and restores the rest">']
    for panel, (title, sub, y0) in enumerate([
        ("Cargo", "every worktree has its own target/, so every worktree compiles everything: %s compiler runs on average" % f_n(avg_cargo_runs), 8),
        ("Mitts", "the first worktree compiles; the others restore the same objects from the shared store: %s compiler runs on average" % f_n(avg_mitts_runs), 300),
    ]):
        out.append('<text class="d-t" x="0" y="%d">%s</text><text class="d-s" x="%d" y="%d">%s</text>' % (y0 + 14, title, 70, y0 + 14, esc(sub)))
        for k in range(12):
            y = y0 + 34 + k * (lane + gap)
            out.append('<text class="ax" x="%d" y="%d" text-anchor="end">worktree %d</text>' % (left - 10, y + 11, k + 1))
            for b in range(blocks):
                x = left + b * cell
                if panel == 0:
                    cls = "b-compile"
                else:
                    cls = "b-compile" if k == 0 else "b-restore"
                    if k > 0 and b == (k * 4 + 3) % blocks:
                        cls = "b-compile"
                out.append('<rect class="%s" x="%.1f" y="%d" width="%.1f" height="%d" rx="1.5"/>' % (cls, x, y, cell - 1.5, lane))
    out.append('<rect class="b-compile" x="120" y="584" width="14" height="14" rx="2"/><text class="ax" x="142" y="596">compiled</text>')
    out.append('<rect class="b-restore" x="240" y="584" width="14" height="14" rx="2"/><text class="ax" x="262" y="596">restored from the store</text>')
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
    out.append('<text class="d-s" x="0" y="196">Worktree 12 carries one deliberately failing test. A tool only counts if it reports that failure and no other.</text>')
    out.append('<text class="d-s" x="0" y="218">A shared cache that hides the failure, or leaks it into clean worktrees, is unsound and is not scored.</text>')
    out.append("</svg>")
    return "".join(out)
