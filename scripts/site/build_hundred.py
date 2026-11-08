# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Render site/hundred/index.html (The Hundred) from site/data/benchmarks.json (made by aggregate.py)."""
import json
import math

import charts
import charts_hundred as ch
from shell import esc, page

HEADLINE = [("swarm_s", "12 worktrees build in parallel"), ("other_worktree_s", "Same code, second worktree"),
            ("hop_s", "Switch branch, build"), ("check_s", "12 worktrees: check"), ("clippy_s", "12 worktrees: clippy"),
            ("noop_ms", "Build again, nothing changed"), ("edit_s", "Edit one file, rebuild"),
            ("history_s", "Replay 10 commits"), ("cold_s", "One cold build")]
METRIC_UNITS = {"wall": "s", "cpu": "s", "runs": "", "written": "MB", "disk": "MB"}


def tabs(key, items, label):
    """Buttons that the page script wires to panels carrying data-<key>. Hidden until JS runs."""
    return '<div class="tabs" role="group" aria-label="%s" data-key="%s">%s</div>' % (
        esc(label), key, "".join('<button type="button" data-v="%s"%s>%s</button>' % (v, ' class="on"' if i == 0 else "", esc(t)) for i, (v, t) in enumerate(items)))


def rng(values, unit="×", digits=1):
    """'3.5–6.1×' or '1.4×' for a list of ratios, truncated (not rounded) so nothing is claimed above what was measured."""
    k = 10 ** digits
    lo, hi = math.floor(min(values) * k + 1e-9) / k, math.floor(max(values) * k + 1e-9) / k
    a, b = "%.*f" % (digits, lo), "%.*f" % (digits, hi)
    return a + unit if a == b else "%s–%s%s" % (a, b, unit)


def counts(data):
    """'100 on macOS and Linux aarch64, 90 on Linux x86_64' from the measured project counts."""
    by = {}
    for p in data["platforms"]:
        by.setdefault(p["projects_ok"], []).append(p["label"].split(" · ")[0] + (" " + p["label"].split(" · ")[1].split()[0] if p["label"].startswith("Linux") else ""))
    return ", ".join("%d on %s" % (n, " and ".join(v)) for n, v in sorted(by.items(), reverse=True))


def share(data, scenario, base="cargo_bare"):
    """Range over the machines of the percentage of projects where the baseline was faster than Mitts."""
    xs = [100.0 * p["summary"][scenario][base]["slower"] / p["summary"][scenario][base]["n"] for p in data["platforms"]]
    return "%d–%d%%" % (min(xs), max(xs)) if int(min(xs)) != int(max(xs)) else "%d%%" % min(xs)


def scoreboards(data):
    panels = "".join('<div class="panel" data-sb="%s">%s</div>' % (p["id"], ch.scoreboard(p)) for p in data["platforms"])
    return tabs("sb", [(p["id"], p["label"]) for p in data["platforms"]], "Machine") + panels


def dumbbells(data):
    out = [tabs("pl", [(p["id"], p["label"]) for p in data["platforms"]], "Machine"),
           tabs("mt", [("wall", "Time"), ("cpu", "CPU"), ("runs", "Compiler runs"), ("written", "Disk written"), ("disk", "Disk in use")], "Measure")]
    for p in data["platforms"]:
        for m in ("wall", "cpu", "runs", "written", "disk"):
            out.append('<div class="panel" data-pl="%s" data-mt="%s">%s</div>' % (p["id"], m, ch.dumbbell(p, m, METRIC_UNITS[m])))
    return "".join(out)


def table_rows(data):
    out = []
    for p in data["platforms"]:
        out.append('<h4>%s</h4><table class="ref nums"><thead><tr><th>Scenario</th><th>Projects</th><th>vs sccache (geo. mean)</th><th>Faster / slower</th><th>vs plain Cargo (geo. mean)</th><th>Faster / slower</th></tr></thead><tbody>' % esc(p["label"]))
        for key, label in HEADLINE:
            s, c = p["summary"].get(key, {}).get("cargo_sccache"), p["summary"].get(key, {}).get("cargo_bare")
            if not s or not c:
                continue
            out.append("<tr><td>%s</td><td>%d</td><td>%s</td><td>%d / %d</td><td>%s</td><td>%d / %d</td></tr>" % (
                esc(label), c["n"], charts.fmt_x(s["geomean"]), s["faster"], s["slower"], charts.fmt_x(c["geomean"]), c["faster"], c["slower"]))
        out.append("</tbody></table>")
    return "".join(out)


def rig_table(data):
    rows = []
    for p in data["platforms"]:
        r = p["rig"]
        rows.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td><code>%s</code></td></tr>" % (
            esc(p["label"]), esc("%s logical CPUs" % r["logical_cpus"] if r.get("logical_cpus") else "n/a"),
            esc(("%s GB" % r["ram_gb"]) if r.get("ram_gb") else "n/a"), esc(r.get("os") or "Linux"),
            "%d" % p["projects_ok"], esc(p["timestamp"][:10] if p.get("timestamp") else "")))
    return '<table class="ref"><thead><tr><th>Machine</th><th>CPUs</th><th>RAM</th><th>OS</th><th>Projects measured</th><th>Measured</th></tr></thead><tbody>%s</tbody></table>' % "".join(rows)


def spread(data, metric):
    """Lowest and highest total gain over plain Cargo across the machines."""
    xs = [p["totals"][metric]["cargo_bare"] / p["totals"][metric]["mitts_cas"] for p in data["platforms"] if p["totals"][metric]["mitts_cas"]]
    return rng(xs)


def slim(data):
    """Only what the project table needs, so the page stays small."""
    out = []
    for p in data["platforms"]:
        rows = [{"n": r["name"], "u": r["units"], "s": r["status"], "sw": r["swarm_s"].get("mitts_cas"), "ss": r["swarm_s"].get("cargo_sccache"), "sc": r["swarm_s"].get("cargo_bare"),
                 "cm": r["cold_s"].get("mitts_cas"), "cc": r["cold_s"].get("cargo_bare"), "no": r["noop_ms"].get("mitts_cas")}
                for r in p["projects"]]
        out.append({"label": p["label"], "rows": rows})
    return out


HEAD = """
<main class="shell bench">
<header class="doc-head"><p class="kicker"><span class="dot"></span>The Hundred &middot; Mitts %(ver)s</p>
<h1>The Hundred.<br><span class="accent">Cargo against Mitts, on 100 Rust projects.</span></h1>
<p class="lede">One hundred open-source Rust projects, each built the way parallel branches and coding agents build: <b>twelve worktrees of the same project at once</b>. Plain <code>cargo</code>, <code>cargo</code> with <code>sccache</code> and Mitts, on three machines. Time, CPU, compiler runs and disk, measured, not estimated. No measured project is dropped from the totals (runs the harness flagged as noisy stay in, marked), and where Mitts is not faster, this page says so.</p></header>

<section class="callouts four" aria-label="Headline results">
<div class="callout"><b>%(t)s</b><span>less time to build 12 worktrees of a project, summed over the projects</span></div>
<div class="callout"><b>%(c)s</b><span>less CPU: processor time is what a laptop battery and a CI bill pay for</span></div>
<div class="callout"><b>%(r)s</b><span>fewer compiler runs: Mitts compiles a unit once and restores it everywhere else</span></div>
<div class="callout"><b>%(w)s</b><span>less written to disk: restored files are linked or cloned, not recompiled and rewritten</span></div>
</section>
<p class="sub small">Against plain Cargo, range over the three machines. %(sc)s</p>

<nav class="local" aria-label="On this page"><a href="#scoreboard">Scoreboard</a><a href="#why">What happens</a><a href="#projects">Every project</a><a href="#method">Method</a><a href="#scenarios">Other scenarios</a><a href="#all">All projects</a><a href="#limits">Limits</a></nav>

<section id="scoreboard"><h2>The scoreboard</h2>
<p>Totals over every measured project on each machine (%(counts)s), each project building 12 worktrees at once. <b>The length of a bar is what it cost</b>, on a plain linear scale. Red is Mitts. The number on the right is how many times less Mitts used.</p>
%(board)s
<p class="note">Time is wall-clock seconds of the 12-worktree build, summed over the measured projects. CPU is user plus system seconds of every process. Compiler runs counts <code>rustc</code> invocations. Disk written is what reached the block device. Disk in use is the peak size of all worktrees and the shared store, measured with <code>du</code>; <code>du</code> counts every copy-on-write clone as a separate file, so on XFS it drops far less than the bytes actually written (the Linux gain in disk in use is small because cloned files still count in full).</p></section>

<section id="why"><h2>What actually happens</h2>
<p>This is an illustration of one mechanism, not a measurement; the two averages in it are measured. Cargo keeps a <code>target/</code> directory per worktree, so a second and a twelfth worktree of the same code compile everything again. Mitts keys every compile action by its inputs and keeps one copy in a machine-wide store, so worktrees two to twelve restore what the first one built.</p>
%(picture)s</section>

<section id="projects"><h2>Every project, one row</h2>
<p>All %(n)d projects, sorted by how much Mitts saved. The grey dot is plain Cargo, the red dot is Mitts, the line between them is the saving. The axis is logarithmic, so equal distances mean equal ratios. Hover a row for the numbers.</p>
%(dumb)s
<p class="note">The grey dot is plain Cargo. Cargo with a warm <code>sccache</code> cuts CPU (plain Cargo needs %(sccc)s as much) but wall time only a little (plain Cargo needs %(scct)s as much): <code>sccache</code> still starts a wrapper for every <code>rustc</code> call, does not cache linking, and Cargo still runs every build script. Both are in the scoreboard and the tables.</p></section>

<section id="method"><h2>What was tested, and how</h2>
%(flow)s
<dl class="params">
<dt>The Hundred</dt><dd>100 open-source Rust projects from crates.io and GitHub, fixed in <code>scripts/corpus_100.json</code>. Measured projects per machine: %(counts)s; every ratio is computed per machine over that machine&rsquo;s own set. <code>nushell</code> is excluded. Projects Cargo itself cannot build are replaced so the corpus stays at 100.</dd>
<dt>The scenario</dt><dd>Twelve git worktrees of one project, on 4 different commits, all build the project&rsquo;s tests at the same moment. That is what parallel branches, a review queue or a team of coding agents does. Test <i>execution</i> is the same for every tool and is not timed; the build uses <code>test --no-run</code>.</dd>
<dt>The tools</dt><dd><b>Plain Cargo</b>, <b>Cargo + sccache</b> (%(scc)s) and <b>Mitts</b> <code>%(ver)s</code> (commit <code>%(commit)s</code>, binary SHA-256 <code>%(sha)s</code>). Same toolchain, same machine, Cargo arms run <code>--offline</code>. A shared <code>CARGO_TARGET_DIR</code> was also run as a demonstration and is left out of the charts: it either missed the injected failure or leaked it into clean worktrees, so it is unsound.</dd>
<dt>The five readings</dt><dd>Wall-clock time of the swarm; CPU time (user + system of all processes); compiler runs; bytes written to the device; peak disk in use. Each is the <b>median of %(repeats)d repeats</b>; quartiles are in the raw data.</dd>
<dt>Soundness</dt><dd>Worktree 12 gets one deliberately failing test. A tool counts only if all 12 worktrees compile with <code>test --no-run</code> and produce the same number of test binaries as plain Cargo. After the timed build, an untimed smoke run filtered to that test checks that worktree 12 fails on it and that a clean worktree runs its test binaries with no failure. A shared target directory fails this check.</dd>
<dt>A quiet machine</dt><dd>Before each run the machine must be idle: CPU busy fraction and load under a threshold and no foreign compiler process. File systems are synced before disk readings.</dd>
<dt>Totals and ratios</dt><dd>The scoreboard sums each reading over the projects of one machine; the ratio is the sum for Cargo divided by the sum for Mitts. The tables further down use the geometric mean of per-project ratios, because ratios multiply. <b>Every measured project is in the scoreboard and in the geometric means.</b> The harness flagged %(wide)d project runs as noisy (the %(repeats)d repeats of a swarm differed by more than 15%%); they are marked &ldquo;wide spread&rdquo; in the project table and kept. A scenario row can cover fewer projects than the swarm: the edit probe fails to compile on crates that deny missing docs, and a few projects fail <code>check</code> or <code>clippy</code> on lints already present upstream, under every tool. Each row lists its project count.</dd>
<dt>Provenance</dt><dd>Every result file records the binary hash, the hash of each script, the commit, the toolchain, the file systems and the load before each run. The reduced data, with absolute paths and device names removed, is published as <a href="../data/benchmarks.json">benchmarks.json</a>.</dd>
</dl>
%(rig)s</section>

<section id="scenarios"><h2>The other scenarios</h2>
<p>The swarm is the case Mitts was built for. The same study also measured everyday builds. Each bar is the geometric-mean speed-up of Mitts over a baseline across the projects of one machine. The axis is logarithmic: the vertical line is 1&times; (equal), bars to the right mean Mitts finished sooner, bars to the left mean the baseline did.</p>
<h3>Against <code>sccache</code></h3><p class="sub">The strongest sound baseline: it caches compiler results and does not change them.</p>
%(leg)s%(bars_s)s
<h3>Against plain Cargo</h3>%(bars_c)s
<h3>Where Mitts does not win</h3>
<p>A single cold build of one checkout, with an empty store, has nothing to reuse and also pays to hash the inputs and fill the store for later builds. Against plain Cargo Mitts is slightly slower: the geometric mean of the per-project ratio is %(coldc)s over the three machines, and plain Cargo is faster on %(coldshare)s of the projects. Against cold Cargo + <code>sccache</code>, which pays for its own wrapper and cache writes, Mitts is %(colds)s.</p>
%(tables)s</section>

<section id="all"><h2>All projects</h2>
<p>Seconds (milliseconds for the no-op column) for Mitts against the two baselines. Pick a machine, sort a column, filter by name.</p>
<div class="controls"><label>Machine <select id="plat"></select></label><label>Filter <input id="flt" type="search" placeholder="project name"></label></div>
<div class="tablewrap"><table class="ref nums" id="ptable"><thead><tr><th data-k="name">Project</th><th data-k="units">Units</th><th data-k="swarm">Swarm Mitts</th><th data-k="swarm_c">Swarm Cargo</th><th data-k="swarm_sc">Swarm sccache</th><th data-k="swarm_xc">vs Cargo</th><th data-k="swarm_x">vs sccache</th><th data-k="cold">Cold Mitts</th><th data-k="cold_c">Cold Cargo</th><th data-k="noop">No-op ms</th><th data-k="status">Repeats</th></tr></thead><tbody></tbody></table></div>
<noscript><p>The table needs JavaScript. The same data is in <a href="../data/benchmarks.json">benchmarks.json</a>.</p></noscript></section>

<section id="limits"><h2>What these numbers do and do not say</h2>
<ul>
<li><b>One version.</b> All three machines ran %(ver)s. Mitts has changed since; each series stands on its own and numbers from different versions are not compared.</li>
<li><b>Reuse is the point.</b> The swarm and second-worktree scenarios measure what a shared store buys when the same code is built many times. They are not the cost of a single first build, which the cold-build rows show.</li>
<li><b>Totals favour big projects.</b> A sum is dominated by the slowest projects. The per-project chart and the geometric means show the spread.</li>
<li><b>Three repeats.</b> Medians of three runs are stable for these workloads but not a confidence interval. The quartiles are in the data.</li>
<li><b>Desktop noise.</b> The Mac Studio carried resident agent sessions during its run. The idle check records and bounds this; it does not remove it.</li>
<li><b>No-op rebuild.</b> &ldquo;Build again, nothing changed&rdquo; is Mitts with its fast lock on, as a plain <code>mitts build</code> runs it. The harness passes an explicit <code>--cache-dir</code>, which turns the fast lock off; with it off the same step is about four times faster than Cargo instead of ten to nineteen (the figure is in the raw data as <code>noop_cache_dir_ms</code>).</li>
<li><b>Memory.</b> Memory is the peak resident size of the heaviest compiler or linker process in a worktree, and its sum over the 12 worktrees. A detached <code>sccache</code> server is outside the measured processes.</li>
<li><b>Disk figures.</b> Device bytes written are whole-device counters and compare tools within one run, not across machines.</li>
<li><b>Not covered.</b> Windows, cross-compilation and a remote or team cache are not supported yet and not measured.</li>
</ul></section>
</main>
<script id="bench-data" type="application/json">%(json)s</script>
<script src="../hundred.js"></script>
"""


def home_block(data):
    """The section of the landing page: the scoreboard, a headline and a link."""
    return """    <section class="shell section hundred-home" id="hundred" aria-labelledby="hundred-title">
      <header class="section-head">
        <span class="num">&#9733;</span>
        <div>
          <h2 id="hundred-title">The Hundred: %(t)s less time, %(r)s fewer compiler runs.</h2>
          <p>100 open-source Rust projects (%(counts)s measured), each built as 12 worktrees at once, on three machines. Plain Cargo against Cargo with sccache against Mitts: time, CPU, compiler runs and disk. Totals over every project measured, none dropped for variance.</p>
        </div>
      </header>
      %(board)s
      <p class="more"><a class="button" href="hundred/">Method, every project and the other scenarios</a></p>
    </section>""" % {"t": spread(data, "wall"), "r": spread(data, "runs"), "board": scoreboards(data), "counts": counts(data)}


def build(data):
    p0 = data["platforms"][0]
    par = p0["parameters"]
    t0 = p0["totals"]
    n0 = max(p0["totals_n"], 1)
    wide = sum(len(p["noisy"]) for p in data["platforms"])
    body = HEAD % {
        "ver": esc(p0["mitts_version"].replace("mitts ", "")), "commit": esc(p0["commit"][:12]), "sha": esc(p0["binary_sha256"]),
        "scc": esc(p0.get("sccache") or "sccache"), "t": spread(data, "wall"), "c": spread(data, "cpu"), "r": spread(data, "runs"),
        "w": spread(data, "written"), "sc": "Against Cargo + sccache the time gain is %s." % rng([p["totals"]["wall"]["cargo_sccache"] / p["totals"]["wall"]["mitts_cas"] for p in data["platforms"]], "×"),
        "board": scoreboards(data), "n": p0["totals_n"],
        "picture": ch.swarm_picture(t0["runs"]["cargo_bare"] / n0, t0["runs"]["mitts_cas"] / n0),
        "dumb": dumbbells(data), "flow": ch.method_flow(),
        "scct": rng([p["totals"]["wall"]["cargo_bare"] / p["totals"]["wall"]["cargo_sccache"] for p in data["platforms"]], "×", 2),
        "sccc": rng([p["totals"]["cpu"]["cargo_bare"] / p["totals"]["cpu"]["cargo_sccache"] for p in data["platforms"]], "×"),
        "coldc": rng([p["summary"]["cold_s"]["cargo_bare"]["geomean"] for p in data["platforms"]], "×", 2),
        "colds": rng([p["summary"]["cold_s"]["cargo_sccache"]["geomean"] for p in data["platforms"]], "× faster", 2),
        "coldshare": share(data, "cold_s"), "counts": counts(data),
        "repeats": par["repeats"], "wide": wide, "rig": rig_table(data), "tables": table_rows(data),
        "leg": charts.legend(data), "bars_s": charts.ratio_bars(data, "cargo_sccache", HEADLINE, "Speed-up of Mitts over sccache"),
        "bars_c": charts.ratio_bars(data, "cargo_bare", HEADLINE, "Speed-up of Mitts over plain Cargo"),
        "json": json.dumps(slim(data), separators=(",", ":")).replace("</", "<\\/"),
    }
    return page("The Hundred: Mitts against Cargo on 100 Rust projects",
                "Mitts against plain Cargo and sccache on 100 Rust projects, three machines: time, CPU, compiler runs and disk for 12 worktrees at once.",
                body, 1, active="The Hundred")
