#!/usr/bin/env python3
# Mitts Build — GitHub README benchmark SVG generator
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Generate light and dark benchmark SVGs in docs/img/ from site/data/benchmarks.json."""

import argparse
from html import escape
import json
import math
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA_FILE = ROOT / "site" / "data" / "benchmarks.json"
OUT_DIR = ROOT / "docs" / "img"

SANS = '-apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif'
MONO = 'ui-monospace, "SFMono-Regular", Menlo, Consolas, "Liberation Mono", monospace'

THEMES = {
    "light": {
        "bg": "#ffffff",
        "border": "#d0d7de",
        "strip": "#f6f8fa",
        "grid": "#eaeef2",
        "track": "#f3f5f7",
        "ink": "#1f2328",
        "muted": "#59636e",
        "cargo": "#656d76",
        "sccache": "#9ba3ad",
        "mitts_bar": "#ff3b30",
        "mitts_txt": "#cf2217",
    },
    "dark": {
        "bg": "#121315",
        "border": "#2e3136",
        "strip": "#1b1d20",
        "grid": "#24272b",
        "track": "#1c1f23",
        "ink": "#f2f3f4",
        "muted": "#9aa0a8",
        "cargo": "#7a828e",
        "sccache": "#b7bec7",
        "mitts_bar": "#ff3b30",
        "mitts_txt": "#ff5449",
    },
}

ARMS = [
    ("cargo_bare", "Cargo", "cargo"),
    ("cargo_sccache", "Cargo + sccache", "sccache"),
    ("mitts_cas", "Mitts", "mitts_bar"),
]

SCENARIOS = [
    ("swarm_s", "12 worktrees", "parallel tests", "s"),
    ("other_worktree_s", "2nd worktree", "clean checkout", "s"),
    ("hop_s", "branch hop", "switch + build", "s"),
    ("noop_ms", "no-op", "no changes", "ms"),
    ("edit_s", "edit + rebuild", "1 file edited", "s"),
    ("cold_s", "cold build", "empty cache", "s"),
]

SCORE_ROWS = [
    ("wall", "Time", "12 worktrees · total", False),
    ("cpu", "CPU", "user + sys · total", False),
    ("runs", "Compiler runs", "rustc runs · total", False),
    ("written", "Disk written", "bytes written · total", False),
    ("rss_sum", "Peak RSS (12 wt)", "12-wt sum · mean/proj", True),
]


def floor1(v):
    """Truncate to one decimal: a claimed gain is never rounded up."""
    return math.floor(v * 10 + 1e-9) / 10


def fmt_x(v):
    """Truncated multiplier matching site/charts.py ('5.8×', '7×', '21×')."""
    if v >= 20:
        return "%d×" % math.floor(v + 1e-9)
    return ("%.1f" % floor1(v)).rstrip("0").rstrip(".") + "×"


def fmt_rng(vals):
    lo, hi = floor1(min(vals)), floor1(max(vals))
    a = ("%.1f" % lo).rstrip("0").rstrip(".")
    b = ("%.1f" % hi).rstrip("0").rstrip(".")
    return a + "×" if a == b else "%s–%s×" % (a, b)


def fmt_scen_val(v, unit):
    if unit == "ms":
        return "%.0f ms" % v
    return "%.1f s" % v if v >= 10 else "%.2f s" % v


def fmt_score_val(key, v):
    if key == "wall":
        return "%.0f s" % v if v < 120 else "%.0f min" % (v / 60.0)
    if key == "cpu":
        return "%.1f h" % (v / 3600.0) if v >= 3600 else "%.0f min" % (v / 60.0)
    if key == "runs":
        return "{:,.0f}".format(v)
    if v >= 1000:
        return "%.0f GB" % (v / 1000.0) if v >= 1e4 else "%.1f GB" % (v / 1000.0)
    return "%.0f MB" % v


def card_header(out, w, h, title, subtitle, t):
    out.append(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 %d %d" width="%d" height="%d" role="img" aria-label="%s">'
        % (w, h, w, h, escape(title))
    )
    out.append(
        '<rect x="0.5" y="0.5" width="%d" height="%d" rx="10" fill="%s" stroke="%s"/>'
        % (w - 1, h - 1, t["bg"], t["border"])
    )
    out.append(
        '<text x="24" y="31" fill="%s" font-family=\'%s\' font-size="15.5" font-weight="700">%s</text>'
        % (t["ink"], SANS, escape(title))
    )
    out.append(
        '<text x="24" y="50" fill="%s" font-family=\'%s\' font-size="11.5">%s</text>'
        % (t["muted"], MONO, escape(subtitle))
    )
    lx = [568, 654, 816]
    for ( _, label, ckey), x in zip(ARMS, lx):
        out.append('<rect x="%d" y="21" width="12" height="12" rx="2.5" fill="%s"/>' % (x, t[ckey]))
        out.append(
            '<text x="%d" y="31" fill="%s" font-family=\'%s\' font-size="12" font-weight="500">%s</text>'
            % (x + 18, t["ink"] if ckey == "mitts_bar" else t["muted"], SANS, escape(label))
        )
    out.append('<line x1="24" x2="%d" y1="64" y2="64" stroke="%s"/>' % (w - 24, t["border"]))


def draw_bars(out, left, span, y0, vals, fmt_fn, t):
    top = max(vals.values()) or 1.0
    for k, (arm, name, ckey) in enumerate(ARMS):
        v = vals[arm]
        by = y0 + 8 + k * 20
        bw = max(3.0, (v / top) * span)
        out.append('<rect x="%d" y="%d" width="%d" height="15" rx="3" fill="%s"/>' % (left, by, span, t["track"]))
        out.append(
            '<rect x="%d" y="%d" width="%.1f" height="15" rx="3" fill="%s"><title>%s: %s</title></rect>'
            % (left, by, bw, t[ckey], escape(name), escape(fmt_fn(v)))
        )
        is_m = arm == "mitts_cas"
        out.append(
            '<text x="%.1f" y="%d" fill="%s" font-family=\'%s\' font-size="11.5" font-weight="%s">%s</text>'
            % (left + bw + 8, by + 11.5, t["mitts_txt"] if is_m else t["ink"], MONO, "700" if is_m else "400", escape(fmt_fn(v)))
        )


def render_scenarios(data, theme_name):
    t = THEMES[theme_name]
    mac = data["platforms"][0]
    w, left, span, rx, row_h = 920, 216, 356, 664, 78
    h = 74 + row_h * len(SCENARIOS) + 54
    out = []
    card_header(
        out,
        w,
        h,
        "Average build time per project · Cargo vs Mitts",
        "%s · Mitts %s · linear scale per row (shorter is faster)" % (mac["label"], mac["mitts_version"]),
        t,
    )
    for idx, (key, title, sub, unit) in enumerate(SCENARIOS):
        y0 = 72 + idx * row_h
        if idx:
            out.append('<line x1="24" x2="%d" y1="%d" y2="%d" stroke="%s"/>' % (w - 24, y0 - 4, y0 - 4, t["grid"]))
        rows = [r for r in mac["projects"] if all(r[key].get(a) is not None for a, _, _ in ARMS)]
        n = len(rows)
        means = {a: sum(r[key][a] for r in rows) / n for a, _, _ in ARMS}
        out.append(
            '<text x="24" y="%d" fill="%s" font-family=\'%s\' font-size="14" font-weight="700">%s</text>'
            % (y0 + 25, t["ink"], SANS, escape(title))
        )
        out.append(
            '<text x="24" y="%d" fill="%s" font-family=\'%s\' font-size="11">%s · n=%d</text>'
            % (y0 + 43, t["muted"], MONO, escape(sub), n)
        )
        draw_bars(out, left, span, y0, means, lambda v, u=unit: fmt_scen_val(v, u), t)
        mean_ratio = means["cargo_bare"] / means["mitts_cas"]
        sb = mac["summary"][key]["cargo_bare"]
        ss = mac["summary"][key]["cargo_sccache"]
        big_col = t["mitts_txt"] if mean_ratio >= 1.0 else t["muted"]
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="20" font-weight="800">%s <tspan fill="%s" font-size="11.5" font-weight="500">mean vs Cargo</tspan></text>'
            % (rx, y0 + 23, big_col, SANS, escape(fmt_x(mean_ratio)), t["muted"])
        )
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11">geomean %s · %d/%d faster</text>'
            % (rx, y0 + 41, t["muted"], MONO, escape(fmt_x(sb["geomean"])), sb["faster"], sb["n"])
        )
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11">vs sccache: %s geomean</text>'
            % (rx, y0 + 56, t["muted"], MONO, escape(fmt_x(ss["geomean"])))
        )

    sy = h - 44
    out.append('<rect x="24" y="%d" width="%d" height="30" rx="6" fill="%s" stroke="%s"/>' % (sy, w - 48, t["strip"], t["border"]))
    out.append(
        '<text x="38" y="%d" fill="%s" font-family=\'%s\' font-size="11.5" font-weight="600">12-wt swarm vs Cargo:</text>'
        % (sy + 19, t["muted"], SANS)
    )
    hosts = [
        ("macOS M2 Ultra", data["platforms"][0], 216),
        ("Linux Ryzen 9 9950X", data["platforms"][1], 446),
        ("Linux ARM64", data["platforms"][2], 706),
    ]
    for hname, pl, hx in hosts:
        ratio = pl["totals"]["wall"]["cargo_bare"] / pl["totals"]["wall"]["mitts_cas"]
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11.5">%s <tspan fill="%s" font-weight="700">%s</tspan> <tspan fill="%s">(n=%d)</tspan></text>'
            % (hx, sy + 19, t["ink"], MONO, escape(hname), t["mitts_txt"], escape(fmt_x(ratio)), t["muted"], pl["totals_n"])
        )
    out.append("</svg>\n")
    return "\n".join(out)


def render_scoreboard(data, theme_name):
    t = THEMES[theme_name]
    mac = data["platforms"][0]
    n_proj = mac["totals_n"]
    w, left, span, rx, row_h = 920, 216, 356, 664, 78
    h = 74 + row_h * len(SCORE_ROWS) + 54
    out = []
    card_header(
        out,
        w,
        h,
        "12-worktree build scoreboard · %d Rust projects" % n_proj,
        "%s · Mitts %s · linear scale per row (shorter is better)" % (mac["label"], mac["mitts_version"]),
        t,
    )
    for idx, (key, title, sub, per_project) in enumerate(SCORE_ROWS):
        y0 = 72 + idx * row_h
        if idx:
            out.append('<line x1="24" x2="%d" y1="%d" y2="%d" stroke="%s"/>' % (w - 24, y0 - 4, y0 - 4, t["grid"]))
        raw = mac["totals"][key]
        scale = float(n_proj) if per_project else 1.0
        vals = {a: raw[a] / scale for a, _, _ in ARMS}
        out.append(
            '<text x="24" y="%d" fill="%s" font-family=\'%s\' font-size="14" font-weight="700">%s</text>'
            % (y0 + 25, t["ink"], SANS, escape(title))
        )
        out.append(
            '<text x="24" y="%d" fill="%s" font-family=\'%s\' font-size="11">%s</text>'
            % (y0 + 43, t["muted"], MONO, escape(sub))
        )
        draw_bars(out, left, span, y0, vals, lambda v, k=key: fmt_score_val(k, v), t)
        vs_c = raw["cargo_bare"] / raw["mitts_cas"]
        vs_s = raw["cargo_sccache"] / raw["mitts_cas"]
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="22" font-weight="800">%s</text>'
            % (rx, y0 + 24, t["mitts_txt"], SANS, escape(fmt_x(vs_c)))
        )
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11">less than Cargo</text>'
            % (rx, y0 + 41, t["muted"], MONO)
        )
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11">%s less than sccache</text>'
            % (rx, y0 + 56, t["muted"], MONO, escape(fmt_x(vs_s)))
        )

    sy = h - 44
    out.append('<rect x="24" y="%d" width="%d" height="30" rx="6" fill="%s" stroke="%s"/>' % (sy, w - 48, t["strip"], t["border"]))
    out.append(
        '<text x="38" y="%d" fill="%s" font-family=\'%s\' font-size="11.5" font-weight="600">3-host range vs Cargo:</text>'
        % (sy + 19, t["muted"], SANS)
    )
    items = [("wall", "Time", 212), ("cpu", "CPU", 334), ("runs", "Runs", 474), ("written", "Written", 616), ("rss_sum", "12-wt RSS", 754)]
    for mkey, mlabel, mx in items:
        ratios = [p["totals"][mkey]["cargo_bare"] / p["totals"][mkey]["mitts_cas"] for p in data["platforms"]]
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11">%s <tspan fill="%s" font-weight="700">%s</tspan></text>'
            % (mx, sy + 19, t["ink"], MONO, escape(mlabel), t["mitts_txt"], escape(fmt_rng(ratios)))
        )
    out.append("</svg>\n")
    return "\n".join(out)


def expected_outputs(data):
    out = {}
    for theme in ("light", "dark"):
        out["bench-scenarios-%s.svg" % theme] = render_scenarios(data, theme)
        out["bench-swarm-%s.svg" % theme] = render_scoreboard(data, theme)
    for name, svg in out.items():
        ET.fromstring(svg)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--data", type=Path, default=DATA_FILE, help="path to benchmarks.json")
    ap.add_argument("--out-dir", type=Path, default=OUT_DIR, help="output directory for SVGs")
    ap.add_argument("--check", action="store_true", help="verify committed SVGs match generated output")
    args = ap.parse_args()

    data = json.loads(args.data.read_text(encoding="utf-8"))
    expected = expected_outputs(data)

    if args.check:
        stale = []
        for name, content in sorted(expected.items()):
            path = args.out_dir / name
            if not path.is_file() or path.read_text(encoding="utf-8") != content:
                stale.append(str(path))
        if stale:
            print("Stale or missing README chart SVGs: " + ", ".join(stale), file=sys.stderr)
            return 1
        print("Verified %d README chart SVGs against %s" % (len(expected), args.data.name))
        return 0

    args.out_dir.mkdir(parents=True, exist_ok=True)
    for name, content in sorted(expected.items()):
        (args.out_dir / name).write_text(content, encoding="utf-8")
    print("Wrote %d README chart SVGs to %s" % (len(expected), args.out_dir))
    return 0


if __name__ == "__main__":
    sys.exit(main())
