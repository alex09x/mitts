# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Inline-SVG scenario bars of The Hundred. Colours come from CSS classes so both themes work."""
import math

from shell import esc

PLAT_CLASS = ["p0", "p1", "p2"]


def floor1(v):
    """Truncate to one decimal: a claimed gain is never rounded up."""
    return math.floor(v * 10 + 1e-9) / 10


def fmt_x(v):
    """'6.9×', '13.6×', '21×'. Truncated, not rounded, so a figure never claims more than was measured."""
    return ("%.1f" % floor1(v)).rstrip("0").rstrip(".") + "×" if v < 20 else "%d×" % math.floor(v)


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
