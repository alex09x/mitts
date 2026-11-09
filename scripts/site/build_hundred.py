# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Render site/the-hundred/index.html (The Hundred) from docs/data/benchmarks.json (made by aggregate.py).
The page is plain static HTML: every table and chart is rendered here; no data file is served."""
import math

import charts_hundred as ch
from shell import esc, page



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


def project_times(data):
    panels = "".join('<div class="panel" data-ty="%s">%s</div>' % (p["id"], ch.project_times(p)) for p in data["platforms"])
    return tabs("ty", [(p["id"], p["label"]) for p in data["platforms"]], "Machine") + panels


def absolute(p0, metric, arm):
    """The first machine's total for one tool, in the metric's own unit: '13 min', '90,961', '641 GB'."""
    return ch.FORMAT[metric](p0["totals"][metric][arm])


def dumbbells(data):
    out = [tabs("pl", [(p["id"], p["label"]) for p in data["platforms"]], "Machine"),
           tabs("mt", [("wall", "Time"), ("cpu", "CPU"), ("runs", "Compiler runs"), ("written", "Disk written"), ("disk", "Disk in use")], "Measure")]
    for p in data["platforms"]:
        for m in ("wall", "cpu", "runs", "written", "disk"):
            out.append('<div class="panel" data-pl="%s" data-mt="%s">%s</div>' % (p["id"], m, ch.project_bars(p, m)))
    return "".join(out)


def scenarios(data):
    panels = "".join('<div class="panel" data-sc="%s">%s</div>' % (p["id"], ch.scenario_times(p)) for p in data["platforms"])
    return tabs("sc", [(p["id"], p["label"]) for p in data["platforms"]], "Machine") + panels


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


def project_tables(data):
    """One static HTML table per machine, every project a row. No script and no data file needed."""
    def num(v, d):
        return "–" if v is None else "%.*f" % (d, v)

    out = []
    for p in data["platforms"]:
        rows = []
        for r in sorted(p["projects"], key=lambda r: r["name"]):
            sw, sc, ss = (r["swarm_s"].get(a) for a in ("mitts_cas", "cargo_bare", "cargo_sccache"))
            rows.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td></tr>" % (
                esc(r["name"]), "–" if r["units"] is None else r["units"], num(sw, 2), num(sc, 2), num(ss, 2),
                num(r["cold_s"].get("mitts_cas"), 2), num(r["cold_s"].get("cargo_bare"), 2), num(r["noop_ms"].get("mitts_cas"), 1),
                "steady" if r["status"] == "OK" else "wide spread"))
        out.append('<div class="panel tablewrap" data-ap="%s"><table class="ref nums"><thead><tr><th>Project</th><th>Units</th><th>Swarm Mitts</th><th>Swarm Cargo</th><th>Swarm sccache</th><th>Cold Mitts</th><th>Cold Cargo</th><th>No-op ms</th><th>Repeats</th></tr></thead><tbody>%s</tbody></table></div>' % (p["id"], "".join(rows)))
    return tabs("ap", [(p["id"], p["label"]) for p in data["platforms"]], "Machine") + "".join(out)


HEAD = """
<main class="shell bench">
<header class="doc-head"><p class="kicker"><span class="dot"></span>The Hundred &middot; Mitts %(ver)s</p>
<h1>The Hundred.<br><span class="accent">Cargo against Mitts, on 100 Rust projects.</span></h1>
<p class="lede">One hundred open-source Rust projects, each built the way parallel branches and coding agents build: <b>twelve worktrees of the same project at once</b>. Plain <code>cargo</code>, <code>cargo</code> with <code>sccache</code> and Mitts, on three machines. Time, CPU, compiler runs and disk, measured, not estimated. No measured project is dropped from the totals (runs the harness flagged as noisy stay in, marked), and where Mitts is not faster, this page says so.</p></header>

<section class="callouts four" aria-label="Headline results">
<div class="callout"><b>%(tm)s</b><span>with Mitts to build 12 worktrees of each of 100 projects, <b class="was">%(tc)s with Cargo</b></span></div>
<div class="callout"><b>%(cm)s</b><span>of processor time, what a laptop battery and a CI bill pay for, <b class="was">%(cc)s with Cargo</b></span></div>
<div class="callout"><b>%(rm)s</b><span>compiler runs, <b class="was">%(rc)s with Cargo</b>: Mitts compiles a unit once and restores it everywhere else</span></div>
<div class="callout"><b>%(wm)s</b><span>written to disk, <b class="was">%(wc)s with Cargo</b>: restored files are linked or cloned, not rewritten</span></div>
</section>
<p class="sub small">The big figures are the %(first)s machine, summed over its 100 projects. The other two machines are in the charts below, tool by tool.</p>

<nav class="local" aria-label="On this page"><a href="#scoreboard">Scoreboard</a><a href="#typical">How long</a><a href="#projects">Every project</a><a href="#method">Method</a><a href="#scenarios">Other scenarios</a><a href="#all">All projects</a><a href="#limits">Limits</a></nav>

<section id="scoreboard"><h2>The scoreboard</h2>
<p>Totals over every measured project on each machine (%(counts)s), each project building 12 worktrees at once. <b>The length of a bar is what it cost</b>, on a plain linear scale. Red is Mitts. The big number on the right is how much Mitts saved against plain Cargo.</p>
%(board)s
<p class="note">Time is wall-clock seconds of the 12-worktree build, summed over the measured projects. CPU is user plus system seconds of every process. Compiler runs counts <code>rustc</code> invocations. Disk written is what reached the block device. Disk in use is the peak size of all worktrees and the shared store, measured with <code>du</code>; <code>du</code> counts every copy-on-write clone as a separate file, so on XFS it drops far less than the bytes actually written (the Linux gain in disk in use is small because cloned files still count in full).</p></section>

<section id="typical"><h2>How long a project takes</h2>
<p>Minutes and seconds, not ratios. Each project builds its tests in 12 worktrees at once. The top row is a typical project (the median of the machine&rsquo;s projects, tool by tool); below it the slowest projects, where the difference is largest. Bars start at zero on a plain linear scale and the number is the measured time.</p>
%(times)s</section>

<section id="projects"><h2>Every project, one row</h2>
<p>Every measured project, slowest with Cargo first. Grey is plain Cargo, red is Mitts, both from zero on the same linear scale, and the measured value is written at the end of each row. Pick a machine and a measure. Short rows are projects that build in seconds; the numbers still tell them apart.</p>
%(dumb)s
<p class="note">Cargo with a warm <code>sccache</code> cuts CPU but wall time only a little: <code>sccache</code> still starts a wrapper for every <code>rustc</code> call, does not cache linking, and Cargo still runs every build script. Both are in the scoreboard and the project tables.</p></section>

<section id="method"><h2>What was tested, and how</h2>
%(flow)s
<dl class="params">
<dt>The Hundred</dt><dd>100 open-source Rust projects from crates.io and GitHub, fixed in <code>scripts/corpus_100.json</code>. Measured projects per machine: %(counts)s; every ratio is computed per machine over that machine&rsquo;s own set. <code>nushell</code> is excluded. Projects Cargo itself cannot build are replaced so the corpus stays at 100.</dd>
<dt>The scenario</dt><dd>Twelve git worktrees of one project, on 4 different commits, all build the project&rsquo;s tests at the same moment. That is what parallel branches, a review queue or a team of coding agents does. Test <i>execution</i> is the same for every tool and is not timed; the build uses <code>test --no-run</code>.</dd>
<dt>The tools</dt><dd><b>Plain Cargo</b>, <b>Cargo + sccache</b> (%(scc)s) and <b>Mitts</b> <code>%(ver)s</code> (commit <code>%(commit)s</code>, binary SHA-256 <code>%(sha)s</code>). Same toolchain, same machine, Cargo arms run <code>--offline</code>. A shared <code>CARGO_TARGET_DIR</code> was also run as a demonstration and is left out of the charts: it either missed the injected failure or leaked it into clean worktrees, so it is unsound.</dd>
<dt>The five readings</dt><dd>Wall-clock time of the swarm; CPU time (user + system of all processes); compiler runs; bytes written to the device; peak disk in use. Each is the <b>median of %(repeats)d repeats</b>; quartiles are in the raw data.</dd>
<dt>Soundness</dt><dd>Worktree 12 gets one deliberately failing test. A tool counts only if all 12 worktrees compile with <code>test --no-run</code> and produce the same number of test binaries as plain Cargo. After the timed build, an untimed smoke run filtered to that test checks that worktree 12 fails on it and that a clean worktree runs its test binaries with no failure. A shared target directory fails this check.</dd>
<dt>A quiet machine</dt><dd>Before each run the machine must be idle: CPU busy fraction and load under a threshold and no foreign compiler process. File systems are synced before disk readings.</dd>
<dt>Totals and typical projects</dt><dd>The scoreboard sums each reading over the projects of one machine. The &ldquo;typical project&rdquo; rows are the median over the projects, tool by tool. <b>Every measured project is in the scoreboard and in the medians.</b> The harness flagged %(wide)d project runs as noisy (the %(repeats)d repeats of a swarm differed by more than 15%%); they are marked &ldquo;wide spread&rdquo; in the project table and kept. A scenario row can cover fewer projects than the swarm: the edit probe fails to compile on crates that deny missing docs, and a few projects fail <code>check</code> or <code>clippy</code> on lints already present upstream, under every tool. Each row lists its project count.</dd>
<dt>Provenance</dt><dd>Every result file records the binary hash, the hash of each script, the commit, the toolchain, the file systems and the load before each run. The reduced data, with absolute paths and device names removed, is in the repository: <a href="https://github.com/alex09x/mitts/blob/main/docs/data/benchmarks.json">docs/data/benchmarks.json</a>.</dd>
</dl>
%(rig)s</section>

<section id="scenarios"><h2>The other scenarios</h2>
<p>The swarm is the case Mitts was built for. The same study also measured everyday builds. Each row is a typical project on the chosen machine (the median over the projects, tool by tool), in real time, from zero on a linear scale; every row has its own scale, so compare the three bars within a row, not rows with each other.</p>
%(scen)s
<h3>Where Mitts does not win</h3>
<p>A single cold build of one checkout, with an empty store, has nothing to reuse and also pays to hash the inputs and fill the store for later builds. So the cold row above is the one where plain Cargo is as fast or a little faster: plain Cargo finished first on %(coldshare)s of the projects, depending on the machine. Cargo + <code>sccache</code> starts cold with an empty cache too, and pays for its wrapper and cache writes, so Mitts is ahead of it there.</p></section>

<section id="all"><h2>All projects</h2>
<p>Seconds (milliseconds for the no-op column) for Mitts against the two baselines, one row per project, alphabetical. Pick a machine.</p>
%(ptables)s</section>

<section id="limits"><h2>What these numbers do and do not say</h2>
<ul>
<li><b>One version.</b> All three machines ran %(ver)s. Mitts has changed since; each series stands on its own and numbers from different versions are not compared.</li>
<li><b>Reuse is the point.</b> The swarm and second-worktree scenarios measure what a shared store buys when the same code is built many times. They are not the cost of a single first build, which the cold-build rows show.</li>
<li><b>Totals favour big projects.</b> A sum is dominated by the slowest projects. The per-project chart and the typical-project rows show the spread.</li>
<li><b>Three repeats.</b> Medians of three runs are stable for these workloads but not a confidence interval. The quartiles are in the data.</li>
<li><b>Desktop noise.</b> The Mac Studio carried resident agent sessions during its run. The idle check records and bounds this; it does not remove it.</li>
<li><b>No-op rebuild.</b> &ldquo;Build again, nothing changed&rdquo; is Mitts with its fast lock on, as a plain <code>mitts build</code> runs it. The harness passes an explicit <code>--cache-dir</code>, which turns the fast lock off; with it off the same step is about four times faster than Cargo instead of ten to nineteen (the figure is in the raw data as <code>noop_cache_dir_ms</code>).</li>
<li><b>Memory.</b> Memory is the peak resident size of the heaviest compiler or linker process in a worktree, and its sum over the 12 worktrees. A detached <code>sccache</code> server is outside the measured processes.</li>
<li><b>Disk figures.</b> Device bytes written are whole-device counters and compare tools within one run, not across machines.</li>
<li><b>Not covered.</b> Windows, cross-compilation and a remote or team cache are not supported yet and not measured.</li>
</ul></section>
</main>
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
      <p class="more"><a class="button" href="the-hundred/">Method, every project and the other scenarios</a></p>
    </section>""" % {"t": spread(data, "wall"), "r": spread(data, "runs"), "board": scoreboards(data), "counts": counts(data)}


def build(data):
    p0 = data["platforms"][0]
    par = p0["parameters"]
    wide = sum(len(p["noisy"]) for p in data["platforms"])
    body = HEAD % {
        "ver": esc(p0["mitts_version"].replace("mitts ", "")), "commit": esc(p0["commit"][:12]), "sha": esc(p0["binary_sha256"]),
        "scc": esc(p0.get("sccache") or "sccache"),
        "board": scoreboards(data), "n": p0["totals_n"],
        "dumb": dumbbells(data), "flow": ch.method_flow(), "times": project_times(data),
        "tm": absolute(p0, "wall", "mitts_cas"), "tc": absolute(p0, "wall", "cargo_bare"),
        "cm": absolute(p0, "cpu", "mitts_cas"), "cc": absolute(p0, "cpu", "cargo_bare"),
        "rm": absolute(p0, "runs", "mitts_cas"), "rc": absolute(p0, "runs", "cargo_bare"),
        "wm": absolute(p0, "written", "mitts_cas"), "wc": absolute(p0, "written", "cargo_bare"),
        "first": esc(p0["label"].split(" · ")[0]),
        "coldshare": share(data, "cold_s"), "counts": counts(data), "scen": scenarios(data),
        "repeats": par["repeats"], "wide": wide, "rig": rig_table(data),
        "ptables": project_tables(data),
    }
    return page("The Hundred: Mitts against Cargo on 100 Rust projects",
                "Mitts against plain Cargo and sccache on 100 Rust projects, three machines: time, CPU, compiler runs and disk for 12 worktrees at once.",
                body, 1, active="The Hundred")
