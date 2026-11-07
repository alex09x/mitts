# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Render site/benchmarks/index.html from site/data/benchmarks.json (made by aggregate.py)."""
import json

import charts
from shell import esc, page

HEADLINE = [("swarm_s", "12 worktrees build in parallel"), ("other_worktree_s", "Same code, second worktree"),
            ("hop_s", "Switch branch, build"), ("check_s", "12 worktrees: check"), ("clippy_s", "12 worktrees: clippy"),
            ("noop_ms", "Build again, nothing changed"), ("edit_s", "Edit one file, rebuild"),
            ("history_s", "Replay 10 commits"), ("cold_s", "One cold build")]


def table_rows(data):
    out = []
    for p in data["platforms"]:
        out.append('<h4>%s</h4><table class="ref nums"><thead><tr><th>Scenario</th><th>vs sccache (geo. mean)</th><th>Faster / slower</th><th>vs plain Cargo (geo. mean)</th><th>Faster / slower</th></tr></thead><tbody>' % esc(p["label"]))
        for key, label in HEADLINE:
            s, c = p["summary"].get(key, {}).get("cargo_sccache"), p["summary"].get(key, {}).get("cargo_bare")
            if not s or not c:
                continue
            out.append("<tr><td>%s</td><td>%s</td><td>%d / %d</td><td>%s</td><td>%d / %d</td></tr>" % (
                esc(label), charts.fmt_x(s["geomean"]), s["faster"], s["slower"], charts.fmt_x(c["geomean"]), c["faster"], c["slower"]))
        out.append("</tbody></table>")
    return "".join(out)


def rig_table(data):
    rows = []
    for p in data["platforms"]:
        r = p["rig"]
        rows.append("<tr><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td>%s</td><td><code>%s</code></td></tr>" % (
            esc(p["label"]), esc("%s logical CPUs" % r["logical_cpus"] if r.get("logical_cpus") else "n/a"),
            esc(("%s GB" % r["ram_gb"]) if r.get("ram_gb") else "n/a"), esc(r.get("os") or "Linux"),
            "%d of %d" % (p["projects_ok"], p["projects_total"]), esc(p["timestamp"][:10] if p.get("timestamp") else "")))
    return '<table class="ref"><thead><tr><th>Platform</th><th>CPUs</th><th>RAM</th><th>OS</th><th>Projects in the aggregates</th><th>Measured</th></tr></thead><tbody>%s</tbody></table>' % "".join(rows)


def build(data):
    p0 = data["platforms"][0]
    ver = p0["mitts_version"]
    par = p0["parameters"]
    best = max((s["geomean"], k) for k, v in p0["summary"].items() for b, s in v.items() if b == "cargo_sccache" and k == "swarm_s")
    scatter = "".join('<figure><figcaption>%s</figcaption>%s</figure>' % (esc(p["label"]), charts.scatter(p, "swarm_s", "cargo_sccache", "s")) for p in data["platforms"])
    strips = "".join('<figure><figcaption>%s</figcaption>%s</figure>' % (esc(p["label"]), charts.strip_hist(p, "swarm_s", "cargo_sccache")) for p in data["platforms"])
    cold = "".join('<figure><figcaption>%s</figcaption>%s</figure>' % (esc(p["label"]), charts.strip_hist(p, "cold_s", "cargo_bare")) for p in data["platforms"])
    body = """
<main class="shell bench">
<header class="doc-head"><p class="kicker"><span class="dot"></span>Benchmarks &middot; The Hundred &middot; %(ver)s</p>
<h1>One hundred Rust projects, measured.</h1>
<p class="lede">Mitts and the two baselines a Rust developer actually has &mdash; plain <code>cargo</code> and <code>cargo</code> with <code>sccache</code> &mdash; run over the same open-source projects (100 on two machines, 90 on the third) on three machines. Every number below is a median of repeated runs, summarised across projects as a geometric mean. Where Mitts is not faster, the page says so.</p></header>

<section class="callouts" aria-label="Headline results">
<div class="callout"><b>%(swarm)s</b><span>faster than <code>sccache</code> when 12 worktrees of one project build at once (geometric mean over %(n)d projects, Apple M2 Ultra)</span></div>
<div class="callout"><b>%(other)s</b><span>faster for the same code in a second worktree, because the first one already built it</span></div>
<div class="callout neutral"><b>&asymp;1&times;</b><span>on one cold build against plain Cargo. That case has nothing to reuse, so there is nothing to win; the numbers are below.</span></div>
</section>

<nav class="local" aria-label="On this page"><a href="#results">Results</a><a href="#swarm">The swarm</a><a href="#cold">Cold builds</a><a href="#tables">Tables</a><a href="#projects">All projects</a><a href="#method">Method</a><a href="#limits">Limits</a></nav>

<section id="results"><h2>Results by scenario</h2>
<p>Each bar is the geometric-mean speed-up of Mitts over a baseline across the projects of one machine. The axis is logarithmic: the vertical line is 1&times; (equal), bars to the right mean Mitts finished sooner, bars to the left mean the baseline did.</p>
<h3>Against <code>sccache</code></h3><p class="sub">The best sound baseline: it caches compiler results and does not change them.</p>
%(leg)s%(bars_s)s
<h3>Against plain Cargo</h3>%(bars_c)s
<p class="note">A shared <code>CARGO_TARGET_DIR</code> is the other obvious shortcut. It is left out of the charts because it is unsound: in the recorded swarms it either missed the injected test failure or let it leak into clean worktrees. It is kept in the raw data as a demonstration.</p></section>

<section id="swarm"><h2>The swarm: 12 worktrees at once</h2>
<p>The scenario Mitts was built for. Twelve worktrees of one project, on four different commits, all build the project's tests at the same moment, the way parallel branches or coding agents do. Worktree 12 carries a deliberately failing test; an arm only counts as sound if it reports that failure and no other.</p>
<h3>Every project, one point</h3>
<p>Horizontal: seconds for <code>sccache</code>. Vertical: seconds for Mitts. A point under the diagonal means Mitts was faster on that project; hover for the name.</p>
<div class="trio">%(scatter)s</div>
<h3>How the speed-up is spread</h3><p>One tick per project on the same log axis; red ticks are projects where Mitts was slower.</p>%(strips)s</section>

<section id="cold"><h2>Cold builds: where Mitts does not win</h2>
<p>A single build of one checkout with empty caches has nothing to reuse. Against plain Cargo it is a tie by design, and on some projects Cargo is a little faster. Against <code>sccache</code> Mitts is about 1.4&times; faster on all three machines (the geometric means are in the tables). Speed-up of Mitts over plain Cargo, one tick per project:</p>%(cold)s</section>

<section id="tables"><h2>Summary tables</h2>%(tables)s</section>

<section id="projects"><h2>All projects</h2>
<p>Seconds (milliseconds for the no-op column) for Mitts against the two baselines. Pick a machine, sort a column, filter by name.</p>
<div class="controls"><label>Machine <select id="plat"></select></label><label>Filter <input id="flt" type="search" placeholder="project name"></label></div>
<div class="tablewrap"><table class="ref nums" id="ptable"><thead><tr><th data-k="name">Project</th><th data-k="units">Units</th><th data-k="swarm">Swarm Mitts</th><th data-k="swarm_sc">Swarm sccache</th><th data-k="swarm_x">Speed-up</th><th data-k="cold">Cold Mitts</th><th data-k="cold_c">Cold Cargo</th><th data-k="noop">No-op ms</th><th data-k="status">Status</th></tr></thead><tbody></tbody></table></div>
<noscript><p>The table needs JavaScript. The same data is in <a href="../data/benchmarks.json">benchmarks.json</a>.</p></noscript></section>

<section id="method"><h2>Method</h2>
<dl class="params">
<dt>The Hundred</dt><dd>100 open-source Rust projects from crates.io and GitHub, fixed in <code>scripts/corpus_100.json</code>. <code>nushell</code> is excluded from this run. Projects Cargo itself cannot build are replaced so the corpus stays at 100.</dd>
<dt>Binary under test</dt><dd><code>%(ver)s</code>, commit <code>%(commit)s</code>, release binary SHA-256 <code>%(sha)s</code>. The latest release is newer; these numbers were not re-measured on it.</dd>
<dt>Baselines</dt><dd><b>Plain Cargo</b>, <b>Cargo + sccache</b> (%(scc)s), and a <b>shared target directory</b> demonstration. Cargo arms run <code>--offline</code>.</dd>
<dt>Scenarios</dt><dd>Fetch and plan (T0, T1); one cold build, no-op, edit one file, same code in a second worktree (T2); replay of %(commits)d consecutive commits and branch switching (T3); check, clippy and reflink-versus-copy disk writes (T4); the 12-worktree swarm (T5).</dd>
<dt>Repeats</dt><dd>%(repeats)d repeats per measurement; the median is used, with the quartiles kept in the raw data. Swarms use %(wt)d worktrees on 4 commits.</dd>
<dt>Soundness</dt><dd>An arm's swarm counts only if every worktree builds, all arms produce the same number of test binaries, and the injected failing test in worktree 12 is reported. A swarm whose total time varies by more than 15%% between repeats is flagged NOISY and kept out of every aggregate (%(noisy)s).</dd>
<dt>Preflight</dt><dd>Before each run the machine must be quiet: CPU busy fraction and load below a threshold and no foreign compiler process, with the wait recorded. File systems are settled with <code>sync</code> before disk measurements.</dd>
<dt>Aggregation</dt><dd>Speed-up is baseline time divided by Mitts time per project; the summary is the geometric mean, with the count of projects where Mitts was faster and slower. The geometric mean is used because ratios multiply.</dd>
<dt>Provenance</dt><dd>Every result file records the binary hash, the hash of each script, the commit, the toolchain, the file systems and the load before each run. The raw reports, with absolute paths and device names removed, are published as <a href="../data/benchmarks.json">benchmarks.json</a>.</dd>
</dl>
%(rig)s</section>

<section id="limits"><h2>What these numbers do and do not say</h2>
<ul>
<li><b>One version.</b> All three machines ran %(ver)s. Mitts has changed since; each series stands on its own and numbers from different versions are not compared.</li>
<li><b>Warm caches favour Mitts by design.</b> The swarm and second-worktree scenarios measure reuse. They show what a shared store buys when the same code is built many times, not what a single first build costs.</li>
<li><b>Build only.</b> Test <i>execution</i> time belongs to the tests and is identical for every arm, so the swarm runs <code>test --no-run</code>. Soundness is checked with two short executions that never enter the metric.</li>
<li><b>Three repeats.</b> Medians of three runs are stable for these workloads but not a confidence interval. The quartiles are in the data.</li>
<li><b>Desktop noise.</b> The Mac Studio carried resident agent sessions during its run. The gate records and bounds this; it does not remove it.</li>
<li><b>Disk figures.</b> Device bytes written are whole-device counters and compare arms within one run, not across machines.</li>
<li><b>Not covered.</b> Windows, cross-compilation and a remote or team cache are not supported yet and not measured.</li>
</ul></section>
</main>
<script id="bench-data" type="application/json">%(json)s</script>
<script src="bench.js"></script>
""" % {
        "ver": esc(ver), "commit": esc(p0["commit"][:12]), "sha": esc(p0["binary_sha256"]), "scc": esc(p0.get("sccache") or "sccache"),
        "swarm": charts.fmt_x(p0["summary"]["swarm_s"]["cargo_sccache"]["geomean"]), "n": p0["summary"]["swarm_s"]["cargo_sccache"]["n"],
        "other": charts.fmt_x(p0["summary"]["other_worktree_s"]["cargo_sccache"]["geomean"]),
        "leg": charts.legend(data), "bars_s": charts.ratio_bars(data, "cargo_sccache", HEADLINE, "Speed-up of Mitts over sccache"),
        "bars_c": charts.ratio_bars(data, "cargo_bare", HEADLINE, "Speed-up of Mitts over plain Cargo"),
        "scatter": scatter, "strips": strips, "cold": cold, "tables": table_rows(data), "rig": rig_table(data),
        "commits": par["commits"], "repeats": par["repeats"], "wt": par["worktrees"],
        "noisy": esc(", ".join(n for p in data["platforms"] for n in p["noisy"]) or "none"),
        "json": json.dumps(slim(data), separators=(",", ":")).replace("</", "<\\/"),
    }
    return page("Mitts benchmarks: the Hundred", "Mitts against plain Cargo and sccache on 100 Rust projects across macOS and Linux: results, method and limits.", body, 1, active="Benchmarks")


def slim(data):
    """Only what the project table needs, so the page stays small."""
    out = []
    for p in data["platforms"]:
        rows = []
        for r in p["projects"]:
            rows.append({"n": r["name"], "u": r["units"], "s": r["status"],
                         "sw": r["swarm_s"].get("mitts_cas"), "ss": r["swarm_s"].get("cargo_sccache"),
                         "cm": r["cold_s"].get("mitts_cas"), "cc": r["cold_s"].get("cargo_bare"),
                         "no": r["noop_ms"].get("mitts_cas")})
        out.append({"label": p["label"], "rows": rows})
    return out
