# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""The guide page site/worktrees/index.html: sharing Cargo build work between git worktrees."""
from shell import page

TITLE = "How to share a Cargo build cache between git worktrees"
DESCRIPTION = ("Every git worktree of a Rust project gets its own target/ and compiles the same crates again. "
               "Three ways to share the work: a shared CARGO_TARGET_DIR, sccache, and Mitts, with their limits.")

# The demo run below is one recording on the release named in it, not a benchmark.
BODY = """
<main class="shell docs guide">
<article class="doc">
<header class="doc-head"><p class="kicker"><span class="dot"></span>Guide &middot; Rust &middot; git worktrees</p><h1>How to share a Cargo build cache between git worktrees</h1>
<p class="lede">Each <code>git worktree</code> of a Rust project has its own <code>target/</code>, so Cargo compiles every dependency again in each one. That is the price of a second branch, a review checkout or a coding agent in its own worktree. Here are the three usual ways to share that work, what each one shares, and where each one stops.</p></header>

<section id="problem"><h2>Why every worktree builds from zero</h2>
<p>Cargo keeps its build cache in the workspace's <code>target/</code> directory. A new worktree is a new directory with an empty <code>target/</code>, so the first build there compiles <code>serde</code>, <code>syn</code>, <code>tokio</code> and the rest again, and stores another copy of each on disk. The same happens for a second clone, and for a branch switch that changes <code>Cargo.lock</code> and back.</p>
<pre class="term"><code>git worktree add ../myproject-review main
cd ../myproject-review
cargo build        # compiles every dependency again</code></pre>
</section>

<section id="shared-target"><h2>Option 1: one shared <code>CARGO_TARGET_DIR</code></h2>
<pre class="term"><code>export CARGO_TARGET_DIR=$HOME/.cache/cargo-target   # or build.target-dir in .cargo/config.toml</code></pre>
<p>Every worktree now writes to one directory, and a dependency compiled for one is found by the next. It needs nothing beyond Cargo.</p>
<ul>
<li><b>One build at a time.</b> Cargo locks the target directory for a build, so worktrees that build at the same moment wait for each other.</li>
<li><b>Mutable outputs are shared too.</b> <code>target/debug/your-binary</code> and the test binaries belong to whichever worktree built last, and worktrees on different commits replace each other's crates, so switching between them rebuilds.</li>
<li>In <a href="../the-hundred/">The Hundred</a> study (measured with Mitts 0.3.17), a shared-target configuration failed the correctness check: a failing test injected into one worktree was missed, or leaked into a clean one. It was left out of the scores.</li>
</ul></section>

<section id="sccache"><h2>Option 2: sccache</h2>
<pre class="term"><code>cargo install sccache --locked
export RUSTC_WRAPPER=sccache</code></pre>
<p>sccache wraps each <code>rustc</code> call and keeps compiled results in a cache outside <code>target/</code>, locally or in remote storage, so worktrees and CI machines can share them.</p>
<ul>
<li>It sees one <code>rustc</code> call at a time. Its key depends on absolute paths unless they are normalized, so the same crate compiled from two checkout paths can miss.</li>
<li>Crates that invoke the linker (<code>bin</code>, <code>proc-macro</code>, <code>dylib</code>) are not cached.</li>
<li>Cargo still plans every build and runs every build script in each worktree.</li>
<li>It is the mature choice when you need a cache shared between machines.</li>
</ul></section>

<section id="mitts"><h2>Option 3: Mitts, a machine-wide store</h2>
<p><a href="../">Mitts</a> reads the same <code>Cargo.toml</code> and <code>Cargo.lock</code>, plans the build itself and runs <code>rustc</code> directly. Every unit gets a key from all of its inputs (toolchain, target, profile, flags, features, source contents, dependency keys, tracked environment). The output is stored once per machine, read-only, and restored into every worktree that needs the same key. An input it cannot account for means a rebuild, not a guess.</p>
<pre class="term"><code>curl -fsSL https://mittsbuild.com/install.sh | sh      # Windows: irm https://mittsbuild.com/install.ps1 | iex
cd myproject && mitts build
git worktree add ../myproject-review HEAD
cd ../myproject-review && mitts build                  # dependencies come from the store</code></pre>
<ul>
<li><b>Shared:</b> registry and git dependencies, including build-script runs, proc macros and linked units, whenever their keys match. Builds in several worktrees at once take per-key locks, so a dependency two of them need is compiled once.</li>
<li><b>Not shared by default:</b> your own crates (workspace members and path dependencies). They stay in each worktree's <code>target/</code>: an unchanged rebuild restores them, an edit replaces them, a second worktree compiles them again. <code>MITTS_SHARE_WORKSPACE_CRATES=1</code> shares them too, when their key can be made path-independent (no <code>CARGO_MANIFEST_DIR</code> read, no build script, no proc macro declared impure).</li>
<li><b>Never cached:</b> <code>cdylib</code>, <code>dylib</code> and <code>staticlib</code> crates, and crates whose <code>env!</code> reads cannot be tracked. <code>mitts explain</code> lists every unit and why.</li>
<li><b>Per machine only.</b> There is no remote or team cache yet.</li>
<li><b>Not a sandbox.</b> A build script or proc macro that reads files it does not declare can give a stale result, as with Cargo.</li>
<li><b>Pre-release.</b> Mitts is young software; Cargo, optionally with sccache, is the safe choice where you need a stable release.</li>
</ul></section>

<section id="example"><h2>One run, shown as it came out</h2>
<p>ripgrep 14.1.1, dev profile, on Mitts 0.5.9 and rustc 1.99.1, an Apple M2 Ultra (24 cores, macOS 15.7.8, APFS), with an empty Mitts cache and sources fetched beforehand. One run on a desktop that was not otherwise idle: it shows what happens, it is not a benchmark.</p>
<div class="scroll"><table class="ref">
<thead><tr><th>Step</th><th>Wall time</th><th>Units</th></tr></thead>
<tbody>
<tr><td>First build, empty cache</td><td>6.5&nbsp;s</td><td>33 compiled</td></tr>
<tr><td>Same build again, nothing changed</td><td>0.016&nbsp;s</td><td>nothing to do</td></tr>
<tr><td>Second worktree, same commit, empty <code>target/</code></td><td>5.5&nbsp;s</td><td>9 compiled, 24 from the store</td></tr>
</tbody></table></div>
<p>The 24 restored units are ripgrep's dependencies. The 9 compiled ones are ripgrep's own crates, which a second worktree builds again by default. ripgrep's own crates are most of its build, so here the second worktree saves little; the more of a build goes into dependencies, the more a second worktree saves.</p>
<p>Repeat it:</p>
<pre class="term"><code>git clone --depth 1 --branch 14.1.1 https://github.com/BurntSushi/ripgrep.git rg-a
cd rg-a
export MITTS_CACHE_DIR=$PWD/../cache       # a fresh, empty cache
mitts fetch
mitts build                                # first build
mitts build                                # nothing changed
git worktree add --detach ../rg-b HEAD && cd ../rg-b
mitts build                                # second worktree</code></pre>
<p>For many projects and parallel worktrees, measured against Cargo and sccache on three machines, see <a href="../the-hundred/">The Hundred</a>. It was measured with Mitts 0.3.17, which also shared the projects' own crates by default.</p>
</section>

<section id="choose"><h2>Which one to use</h2>
<ul>
<li>One worktree at a time, no extra tools: a shared <code>CARGO_TARGET_DIR</code>, accepting the lock and the shared binaries.</li>
<li>A cache shared between machines or with CI: sccache.</li>
<li>Several worktrees building on one machine at the same time, for branches, reviews or coding agents: Mitts keeps each worktree's <code>target/</code> separate and shares the dependencies between them.</li>
</ul>
<p>More: <a href="../docs/#worktrees">what a second checkout reuses</a>, <a href="../docs/#action-keys">what decides a cache hit</a>, and the source on <a href="https://github.com/alex09x/mitts">GitHub</a>.</p>
</section>
</article>
</main>"""


def build():
    return page(TITLE, DESCRIPTION, BODY, 1, active="Worktrees", path="worktrees/", kind="TechArticle")
