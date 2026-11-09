#!/usr/bin/env python3
# Mitts Build — GitHub README benchmark SVG generator
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Generate light and dark benchmark SVGs in docs/img/ from docs/data/benchmarks.json.

Values are shown as real time, counts and gigabytes, formatted by the same rules as the site
(scripts/site/charts_hundred.py): baselines truncated and Mitts rounded up, so no figure exceeds the measurement.
Scoreboard rows share one unit; scenario medians are written in their own natural unit.
"""

import argparse
from html import escape
import json
import statistics
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA_FILE = ROOT / "docs" / "data" / "benchmarks.json"
OUT_DIR = ROOT / "docs" / "img"

sys.path.insert(0, str(Path(__file__).resolve().parent))
import charts_hundred as ch  # noqa: E402

SANS = '-apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif'
MONO = 'ui-monospace, "SFMono-Regular", Menlo, Consolas, "Liberation Mono", monospace'

THEMES = {
    "light": {
        "bg": "#ffffff",
        "border": "#d0d7de",
        "strip": "#f6f8fa",
        "grid": "#eaeef2",
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
    ("wall", "Time", "12 worktrees · total"),
    ("cpu", "CPU", "user + sys · total"),
    ("runs", "Compiler runs", "rustc runs · total"),
    ("written", "Disk written", "bytes written · total"),
]

HOSTS = ["macOS", "Linux x86", "Linux ARM"]


def fmt_mitts_ver(raw):
    v = raw.strip()
    if v.lower().startswith("mitts "):
        v = v[6:].strip()
    return "Mitts %s" % v


def svg_open(out, w, h, title, t):
    out.append(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 %d %d" width="%d" height="%d" role="img" aria-label="%s">'
        % (w, h, w, h, escape(title))
    )
    out.append(
        '<rect x="0.5" y="0.5" width="%d" height="%d" rx="10" fill="%s" stroke="%s"/>'
        % (w - 1, h - 1, t["bg"], t["border"])
    )


def card_header(out, w, h, title, subtitle, t, aria=None):
    svg_open(out, w, h, aria or title, t)
    out.append(
        '<text x="24" y="31" fill="%s" font-family=\'%s\' font-size="15.5" font-weight="700">%s</text>'
        % (t["ink"], SANS, escape(title))
    )
    out.append(
        '<text x="24" y="50" fill="%s" font-family=\'%s\' font-size="11.5">%s</text>'
        % (t["muted"], MONO, escape(subtitle))
    )
    lx = [568, 654, 816]
    for (_, label, ckey), x in zip(ARMS, lx):
        out.append('<rect x="%d" y="21" width="12" height="12" rx="2.5" fill="%s"/>' % (x, t[ckey]))
        out.append(
            '<text x="%d" y="31" fill="%s" font-family=\'%s\' font-size="12" font-weight="500">%s</text>'
            % (x + 18, t["ink"] if ckey == "mitts_bar" else t["muted"], SANS, escape(label))
        )
    out.append('<line x1="24" x2="%d" y1="64" y2="64" stroke="%s"/>' % (w - 24, t["border"]))


def draw_bars(out, left, span, y0, vals, texts, t):
    top = max(vals.values()) or 1.0
    for k, (arm, name, ckey) in enumerate(ARMS):
        by = y0 + 8 + k * 20
        bw = max(3.0, (vals[arm] / top) * span)
        out.append(
            '<rect x="%d" y="%d" width="%.1f" height="15" rx="3" fill="%s"><title>%s: %s</title></rect>'
            % (left, by, bw, t[ckey], escape(name), escape(texts[arm]))
        )
        is_m = arm == "mitts_cas"
        out.append(
            '<text x="%.1f" y="%.1f" fill="%s" font-family=\'%s\' font-size="11.5" font-weight="%s">%s</text>'
            % (left + bw + 12, by + 11.5, t["mitts_txt"] if is_m else t["ink"], MONO, "700" if is_m else "400", escape(texts[arm]))
        )


def text(out, x, y, fill, family, size, weight, body):
    out.append(
        '<text x="%s" y="%s" fill="%s" font-family=\'%s\' font-size="%s" font-weight="%s">%s</text>'
        % (x, y, fill, family, size, weight, escape(body))
    )


def dur(arm, seconds):
    """A duration for a scenario median: truncated for the baselines, rounded up for Mitts, at the precision of ch.f_dur."""
    return ch.fmt_arm("wall", arm, seconds)


def totals(p, key):
    return {a: p["totals"][key][a] for a, _, _ in ARMS}


def first_counts(rows, key):
    """On how many projects Mitts finished strictly before Cargo and before Cargo + sccache."""
    n = len(rows)
    vc = sum(1 for r in rows if r[key]["mitts_cas"] < r[key]["cargo_bare"])
    vs = sum(1 for r in rows if r[key]["mitts_cas"] < r[key]["cargo_sccache"])
    return vc, vs, n


def render_hero(data, theme_name):
    """The picture at the top of the README: four totals of the 12-worktree run, Mitts big, Cargo beside it."""
    t = THEMES[theme_name]
    mac = data["platforms"][0]
    w, h = 920, 244
    out = []
    card_header(
        out,
        w,
        h,
        "%d Rust projects · 12 worktrees built at once" % mac["totals_n"],
        "%s · %s · totals over all projects" % (mac["label"], fmt_mitts_ver(mac["mitts_version"])),
        t,
        aria="Mitts versus Cargo and sccache on %d Rust projects built in 12 worktrees at once" % mac["totals_n"],
    )
    col_w, gap = 208, 16
    for i, (key, title, _sub) in enumerate(SCORE_ROWS):
        x = 24 + i * (col_w + gap)
        raw = totals(mac, key)
        txt, save_c, _save_s = ch.board_row(key, raw)
        text(out, x, 92, t["muted"], SANS, 12.5, 600, title.upper())
        text(out, x, 134, t["mitts_txt"], SANS, 34, 800, txt["mitts_cas"])
        text(out, x, 156, t["muted"], MONO, 11, 400, "Cargo %s · sccache %s" % (txt["cargo_bare"], txt["cargo_sccache"]))
        top = max(raw.values())
        for k, (arm, _name, ckey) in enumerate(ARMS):
            bw = max(3.0, raw[arm] / top * (col_w - 8))
            out.append(
                '<rect x="%d" y="%d" width="%.1f" height="9" rx="3" fill="%s"/>' % (x, 168 + k * 14, bw, t[ckey])
            )
        text(out, x, 226, t["ink"], SANS, 12.5, 600, "%s less than Cargo" % save_c)
    out.append("</svg>\n")
    return "\n".join(out)


def render_scenarios(data, theme_name):
    t = THEMES[theme_name]
    mac = data["platforms"][0]
    w, left, span, rx, row_h = 920, 216, 348, 664, 78
    h = 74 + row_h * len(SCENARIOS) + 54
    out = []
    card_header(
        out,
        w,
        h,
        "Build time of a median project · Cargo vs Mitts",
        "%s · %s · linear scale per row (shorter is faster)" % (mac["label"], fmt_mitts_ver(mac["mitts_version"])),
        t,
    )
    for idx, (key, title, sub, unit) in enumerate(SCENARIOS):
        y0 = 72 + idx * row_h
        if idx:
            out.append('<line x1="24" x2="%d" y1="%d" y2="%d" stroke="%s"/>' % (w - 24, y0 - 4, y0 - 4, t["grid"]))
        rows = [r for r in mac["projects"] if all(r[key].get(a) is not None for a, _, _ in ARMS)]
        k = 0.001 if unit == "ms" else 1.0
        meds = {a: statistics.median(r[key][a] for r in rows) * k for a, _, _ in ARMS}
        texts = {a: dur(a, v) for a, v in meds.items()}
        text(out, 24, y0 + 25, t["ink"], SANS, 14, 700, title)
        text(out, 24, y0 + 43, t["muted"], MONO, 11, 400, "%s · n=%d" % (sub, len(rows)))
        draw_bars(out, left, span, y0, meds, texts, t)
        vc, vs, n = first_counts(rows, key)
        big = t["mitts_txt"] if vc * 2 > n else t["muted"]
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="20" font-weight="800">%d of %d <tspan fill="%s" font-size="11.5" font-weight="500">Mitts first vs Cargo</tspan></text>'
            % (rx, y0 + 23, big, SANS, vc, n, t["muted"])
        )
        text(out, rx, y0 + 43, t["muted"], MONO, 11, 400, "%d of %d first vs sccache" % (vs, n))

    sy = h - 44
    out.append('<rect x="24" y="%d" width="%d" height="30" rx="6" fill="%s" stroke="%s"/>' % (sy, w - 48, t["strip"], t["border"]))
    text(out, 38, sy + 19, t["muted"], SANS, 11.5, 600, "12-worktree total time:")
    for name, pl, hx in zip(HOSTS, data["platforms"], (212, 428, 654)):
        txt = ch.board_row("wall", totals(pl, "wall"))[0]
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11.5">%s <tspan fill="%s" font-weight="700">%s → %s</tspan> <tspan fill="%s">(n=%d)</tspan></text>'
            % (hx, sy + 19, t["ink"], MONO, escape(name), t["mitts_txt"], escape(txt["cargo_bare"]), escape(txt["mitts_cas"]), t["muted"], pl["totals_n"])
        )
    out.append("</svg>\n")
    return "\n".join(out)


def render_scoreboard(data, theme_name):
    t = THEMES[theme_name]
    mac = data["platforms"][0]
    n_proj = mac["totals_n"]
    w, left, span, rx, row_h = 920, 216, 348, 664, 78
    h = 74 + row_h * len(SCORE_ROWS) + 54
    out = []
    card_header(
        out,
        w,
        h,
        "12-worktree build scoreboard · %d Rust projects" % n_proj,
        "%s · %s · linear scale per row (shorter is better)" % (mac["label"], fmt_mitts_ver(mac["mitts_version"])),
        t,
    )
    for idx, (key, title, sub) in enumerate(SCORE_ROWS):
        y0 = 72 + idx * row_h
        if idx:
            out.append('<line x1="24" x2="%d" y1="%d" y2="%d" stroke="%s"/>' % (w - 24, y0 - 4, y0 - 4, t["grid"]))
        raw = totals(mac, key)
        texts, save_c, save_s = ch.board_row(key, raw)
        text(out, 24, y0 + 25, t["ink"], SANS, 14, 700, title)
        text(out, 24, y0 + 43, t["muted"], MONO, 11, 400, sub)
        draw_bars(out, left, span, y0, raw, texts, t)
        text(out, rx, y0 + 24, t["mitts_txt"], SANS, 22, 800, save_c)
        text(out, rx, y0 + 41, t["muted"], MONO, 11, 400, "less than Cargo")
        text(out, rx, y0 + 56, t["muted"], MONO, 11, 400, "%s less than sccache" % save_s)

    sy = h - 44
    out.append('<rect x="24" y="%d" width="%d" height="30" rx="6" fill="%s" stroke="%s"/>' % (sy, w - 48, t["strip"], t["border"]))
    text(out, 38, sy + 19, t["muted"], SANS, 11.5, 600, "Compiler runs, all machines:")
    for name, pl, hx in zip(HOSTS, data["platforms"], (212, 428, 654)):
        txt = ch.board_row("runs", totals(pl, "runs"))[0]
        out.append(
            '<text x="%d" y="%d" fill="%s" font-family=\'%s\' font-size="11.5">%s <tspan fill="%s" font-weight="700">%s → %s</tspan> <tspan fill="%s">(n=%d)</tspan></text>'
            % (hx, sy + 19, t["ink"], MONO, escape(name), t["mitts_txt"], escape(txt["cargo_bare"]), escape(txt["mitts_cas"]), t["muted"], pl["totals_n"])
        )
    out.append("</svg>\n")
    return "\n".join(out)


def expected_outputs(data):
    out = {}
    for theme in ("light", "dark"):
        out["bench-hero-%s.svg" % theme] = render_hero(data, theme)
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
