# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Inline-SVG charts for the benchmark page. Colours come from CSS classes so both themes work."""
import math

from shell import esc

PLAT_CLASS = ["p0", "p1", "p2"]


def fmt_x(v):
    return ("%.1f" % v).rstrip("0").rstrip(".") + "×" if v < 10 else "%d×" % round(v)


def log_x(v, lo, hi, x0, x1):
    return x0 + (math.log(v) - math.log(lo)) / (math.log(hi) - math.log(lo)) * (x1 - x0)


def ratio_bars(data, base, scenarios, title):
    """Grouped horizontal bars of geometric-mean speedup; log axis; the 1x line is the baseline."""
    lo, hi = 0.5, 100.0
    left, right, top = 250, 820, 34
    row = 20 * len(data["platforms"]) + 22
    h = top + row * len(scenarios) + 40
    out = ['<svg class="chart" viewBox="0 0 860 %d" role="img" aria-label="%s">' % (h, esc(title))]
    for t in (0.5, 1, 2, 5, 10, 20, 50, 100):
        x = log_x(t, lo, hi, left, right)
        out.append('<line class="grid%s" x1="%.1f" x2="%.1f" y1="%d" y2="%d"/>' % (" one" if t == 1 else "", x, x, top - 8, h - 30))
        out.append('<text class="ax" x="%.1f" y="%d" text-anchor="middle">%s</text>' % (x, h - 12, ("%g×" % t)))
    x1 = log_x(1, lo, hi, left, right)
    for i, (key, label) in enumerate(scenarios):
        y0 = top + i * row
        out.append('<text class="lab" x="%d" y="%d" text-anchor="end">%s</text>' % (left - 14, y0 + row / 2 - 4, esc(label)))
        for j, p in enumerate(data["platforms"]):
            s = p["summary"].get(key, {}).get(base)
            if not s:
                continue
            v = s["geomean"]
            x = log_x(max(lo, min(hi, v)), lo, hi, left, right)
            by = y0 + 4 + j * 20
            out.append('<rect class="bar %s" x="%.1f" y="%d" width="%.1f" height="14" rx="2"><title>%s: %s geometric mean, %d of %d projects faster</title></rect>'
                       % (PLAT_CLASS[j], min(x, x1), by, abs(x - x1), esc(p["label"]), fmt_x(v), s["faster"], s["n"]))
            tx = x + 6 if v >= 1 else x - 6
            out.append('<text class="val" x="%.1f" y="%d" text-anchor="%s">%s</text>' % (tx, by + 11, "start" if v >= 1 else "end", fmt_x(v)))
    out.append("</svg>")
    return "".join(out)


def legend(data):
    return '<ul class="legend">%s</ul>' % "".join('<li><i class="sw %s"></i>%s</li>' % (PLAT_CLASS[j], esc(p["label"])) for j, p in enumerate(data["platforms"]))


def scatter(platform, scenario, base, unit_label):
    """One point per project: baseline on x, Mitts on y, both log. Below the diagonal means Mitts is faster."""
    pts = [(r["name"], r[scenario].get(base), r[scenario].get("mitts_cas")) for r in platform["projects"] if r["status"] == "OK"]
    pts = [(n, b, m) for n, b, m in pts if b and m and b > 0 and m > 0]
    if not pts:
        return ""
    vals = [v for _, b, m in pts for v in (b, m)]
    lo = 10 ** math.floor(math.log10(min(vals)))
    hi = 10 ** math.ceil(math.log10(max(vals)))
    x0, x1, y0, y1 = 56, 330, 270, 24
    out = ['<svg class="chart small" viewBox="0 0 360 310" role="img" aria-label="%s: Mitts against %s per project">' % (esc(platform["label"]), base)]
    d = lo
    while d <= hi * 1.0001:
        px, py = log_x(d, lo, hi, x0, x1), log_x(d, lo, hi, y0, y1)
        out.append('<line class="grid" x1="%.1f" x2="%.1f" y1="%d" y2="%d"/><line class="grid" x1="%d" x2="%d" y1="%.1f" y2="%.1f"/>' % (px, px, y1, y0, x0, x1, py, py))
        out.append('<text class="ax" x="%.1f" y="288" text-anchor="middle">%g</text><text class="ax" x="48" y="%.1f" text-anchor="end">%g</text>' % (px, d, py + 4, d))
        d *= 10
    out.append('<line class="diag" x1="%d" y1="%d" x2="%d" y2="%d"/>' % (x0, y0, x1, y1))
    for n, b, m in pts:
        out.append('<circle class="pt" cx="%.1f" cy="%.1f" r="3.2"><title>%s: %s %g %s, Mitts %g %s</title></circle>'
                   % (log_x(b, lo, hi, x0, x1), log_x(m, lo, hi, y0, y1), esc(n), base, b, unit_label, m, unit_label))
    out.append('<text class="ax" x="%d" y="304" text-anchor="middle">%s (%s)</text>' % ((x0 + x1) // 2, "sccache" if base == "cargo_sccache" else "plain cargo", unit_label))
    out.append('<text class="ax" transform="translate(12 %d) rotate(-90)" text-anchor="middle">Mitts (%s)</text>' % ((y0 + y1) // 2, unit_label))
    out.append("</svg>")
    return "".join(out)


def strip_hist(platform, scenario, base):
    """Distribution of per-project speedups on a log axis: one tick per project."""
    rs = []
    for r in platform["projects"]:
        if r["status"] != "OK":
            continue
        m, b = r[scenario].get("mitts_cas"), r[scenario].get(base)
        if m and b:
            rs.append(b / m)
    if not rs:
        return ""
    lo, hi = 0.25, 200.0
    out = ['<svg class="chart strip" viewBox="0 0 860 74" role="img" aria-label="Distribution of per-project speedups">']
    for t in (0.5, 1, 2, 5, 10, 20, 50, 100):
        x = log_x(t, lo, hi, 30, 830)
        out.append('<line class="grid%s" x1="%.1f" x2="%.1f" y1="8" y2="46"/><text class="ax" x="%.1f" y="64" text-anchor="middle">%g×</text>' % (" one" if t == 1 else "", x, x, x, t))
    for v in rs:
        x = log_x(max(lo, min(hi, v)), lo, hi, 30, 830)
        out.append('<line class="tick%s" x1="%.1f" x2="%.1f" y1="14" y2="40"/>' % (" slow" if v < 1 else "", x, x))
    out.append("</svg>")
    return "".join(out)
