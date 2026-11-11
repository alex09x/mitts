# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Render site/docs/index.html: install, quick start, the cache mechanism, every command and option, environment, config."""
import html
import subprocess

import docs_commands as C
import docs_how
import helpparse
from shell import esc, page

GROUP_ORDER = ["Build", "Test", "Inspect", "Release", "Cache"]


def pre(text):
    return "<pre class=\"term\"><code>%s</code></pre>" % html.escape(text)


def command_section(binary, name):
    group, purpose, examples, extra = C.COMMANDS[name]
    summary, usage, opts, trailer = helpparse.parse(helpparse.run_help(binary, name))
    rows = []
    for o in opts:
        if o["long"] == "--help":
            continue
        flag = ("<code>%s</code>, " % o["short"] if o["short"] else "") + "<code>%s%s</code>" % (o["long"], html.escape(" " + o["value"] if o["value"].startswith("<") else o["value"]))
        note = C.FLAG_NOTES.get(o["long"])
        text = html.escape(o["text"]).replace("`", "")
        if note:
            text += "<p class=\"more\">%s</p>" % note
        rows.append("<tr><td class=\"flag\">%s</td><td>%s</td></tr>" % (flag, text))
    ex = "".join("<li>%s<span>%s</span></li>" % ("<code>%s</code>" % html.escape(c), d) for c, d in examples)
    tail = "".join("<p class=\"hint\">%s</p>" % html.escape(t) for t in trailer if not t.startswith("mitts ") and not t.startswith("Examples"))
    return (name, """
<section class="cmd" id="cmd-%(n)s">
<h3><code>mitts %(n)s</code> <span class="pill">%(g)s</span></h3>
<p>%(p)s</p>
<p class="usage"><code>%(u)s</code></p>
<h4>Examples</h4><ul class="examples">%(ex)s</ul>
<h4>Options</h4>
<table class="ref opts"><thead><tr><th>Option</th><th>What it does</th></tr></thead><tbody>%(rows)s</tbody></table>
%(tail)s%(extra)s
</section>""" % {"n": name, "g": group, "p": purpose, "u": html.escape(usage), "ex": ex, "rows": "".join(rows), "tail": tail, "extra": extra})


def env_table():
    rows = "".join("<tr><td class=\"flag\"><code>%s</code></td><td><code>%s</code></td><td>%s<p class=\"more\"><code>%s</code></p></td></tr>" % (n, html.escape(v), d, html.escape(e))
                   for n, v, d, e in C.ENV)
    return "<table class=\"ref\"><thead><tr><th>Variable</th><th>Value</th><th>Meaning and example</th></tr></thead><tbody>%s</tbody></table>" % rows


INTRO = """
<section id="install"><h2>Install</h2>
<p>Mitts is one binary for macOS (Apple Silicon) and Linux (x86_64 and aarch64). The installer downloads the release for your platform and verifies its checksum. If GitHub cannot be reached, or your platform has no prebuilt binary (Intel macOS, other architectures), and Cargo is installed, it builds the same version from crates.io instead; set <code>MITTS_NO_SOURCE_FALLBACK=1</code> to fail rather than compile. See the <a href="#environment">environment variables</a>.</p>""" + pre("curl -fsSL https://mittsbuild.com/install.sh | sh\nmitts --version") + """
<p>From source, or through crates.io, once you already have a Rust toolchain:</p>""" + pre("cargo install mitts --locked        # from crates.io\ncargo install --path .              # from a clone of the repository") + """
<p>Mitts reads your existing <code>Cargo.toml</code> and <code>Cargo.lock</code>. There is nothing to configure and no second manifest. Outputs go to Cargo's usual <code>target/debug</code> and <code>target/release</code> (or the root chosen by <code>CARGO_TARGET_DIR</code> or <code>--target-dir</code>); Mitts keeps its own build lock and file index under <code>target/mitts/</code>.</p></section>

<section id="quick-start"><h2>Quick start</h2>
<ol class="steps">
<li><b>Build.</b> From any Cargo workspace:""" + pre("cd your-workspace\nmitts build") + """The first build compiles and stores each unit. The progress line shows <i>compiled</i> and <i>cached</i> units.</li>
<li><b>Build again.</b> Nothing changed, so Mitts returns almost immediately (the <a href="#fast-path">no-op fast path</a>).</li>
<li><b>Make a second checkout and build it.</b>""" + pre("git worktree add ../your-workspace-2 HEAD\ncd ../your-workspace-2\nmitts build") + """Every dependency, and every crate of yours whose key is path-independent, is restored from the machine-wide store instead of compiled. See <a href="#worktrees">why a second checkout reuses everything</a>.</li>
<li><b>Ask why.</b>""" + pre("mitts explain               # the plan and every unit's cache eligibility\nmitts explain --keys k.json  # each unit's action key, for diffing between commits") + """</li>
<li><b>Run the tests, fast.</b>""" + pre("mitts test -c                # every test in its own process, all cores") + """</li>
</ol></section>
"""

OUTPUT = """
<section id="output-formats"><h2>Output formats</h2>
<p><code>--message-format</code> on <code>build</code>, <code>check</code>, <code>clippy</code>, <code>test</code>, <code>bench</code> and <code>run</code> selects how diagnostics are printed. Cached runs replay diagnostics in exactly the requested format; each format has its own cache entries.</p>
<table class="ref"><thead><tr><th>Value</th><th>Output</th></tr></thead><tbody>
<tr><td class="flag"><code>human</code> (default)</td><td>Progress, banners and rendered multi-line diagnostics.</td></tr>
<tr><td class="flag"><code>short</code></td><td>One line per diagnostic, <code>file:line:col: level: message</code>, on stderr; no banners.</td></tr>
<tr><td class="flag"><code>json</code></td><td>Cargo-compatible JSON lines on stdout. Nothing else is printed there while compiling.</td></tr>
<tr><td class="flag"><code>json-diagnostic-short</code></td><td>JSON lines whose <code>message.rendered</code> is the short single-line text.</td></tr>
<tr><td class="flag"><code>json-diagnostic-rendered-ansi</code></td><td>JSON lines whose <code>message.rendered</code> keeps ANSI colour escapes.</td></tr>
<tr><td class="flag"><code>json-render-diagnostics</code></td><td>No <code>compiler-message</code> records on stdout; diagnostics render to stderr; <code>build-finished</code> is still emitted.</td></tr>
</tbody></table>
<p>Cargo's comma-separated modifiers work (<code>json,json-diagnostic-short</code>). Conflicting combinations such as <code>human,json</code>, or unknown values, are rejected with exit code 2.</p>
<h4>The JSON stream</h4>
<p>Each stdout line is one object with a <code>reason</code>. <code>compiler-message</code> appears once per diagnostic, in the order units finish; <code>build-finished</code> closes the stream with <code>success</code> false if any unit failed. The exit code matches human mode.</p>""" + pre('{"reason":"compiler-message","package_id":"demo@0.1.0","target":{"kind":["lib"],"name":"demo"},\n "message":{"level":"error","code":{"code":"E0308"},"rendered":"..."}}\n{"reason":"build-finished","success":false}') + """
<h4>Quiet mode</h4>
<p><code>-q</code> / <code>--quiet</code> removes Mitts' banners, progress bars and stat lines. In <code>mitts test -q</code> libtest receives <code>-q</code>, failures and the final <code>test result</code> line remain. In <code>mitts test -c -q</code> the <code>PASS</code> lines go, <code>FAIL</code> output and the <code>Summary</code> line stay.</p>
</section>
"""

CONFIG = """
<section id="configuration"><h2>Configuration</h2>
<p>Mitts has no configuration file of its own. It follows Cargo's, and has one project-level table.</p>
<dl class="params">
<dt>Cache location</dt><dd><code>--cache-dir</code>, then <code>MITTS_CACHE_DIR</code>, then <code>$XDG_CACHE_HOME/mitts</code>, then <code>~/.cache/mitts</code>.</dd>
<dt>Impure proc macros</dt><dd>Proc macros are pure by default. List the ones that read package files or the package path while expanding, in the root manifest:""" + pre('[workspace.metadata.mitts]\nimpure-proc-macros = ["sqlx-macros", "askama_derive", "rust-embed-impl"]') + """Their direct dependents are then keyed on the whole package tree and on the absolute path, so an edited template invalidates them and a second checkout rebuilds them.</dd>
<dt>Registries and mirrors</dt><dd><code>[registries.&lt;name&gt;] index</code>, <code>[source.&lt;name&gt;] registry | directory | git</code> and <code>replace-with</code> are read from Cargo's config chain, as are vendored directories and <code>[patch]</code>. Tokens come from Cargo's credential providers: <code>cargo:token</code> (<code>CARGO_REGISTRIES_&lt;NAME&gt;_TOKEN</code> or <code>credentials.toml</code>), <code>cargo:macos-keychain</code>, <code>cargo:libsecret</code>, <code>cargo:token-from-stdout</code>, and plugins.</dd>
<dt><code>[env]</code> tables</dt><dd>Merged over the config chain (the closer file wins). <code>force = false</code> keeps a variable the environment already has; <code>relative = true</code> joins the value to the parent of the config's <code>.cargo</code>. Applied to compiler runs, build scripts and programs started by <code>mitts run</code> and <code>mitts test</code>. Config files are part of the package inputs, so a change invalidates the units that read it.</dd>
<dt>Git dependencies</dt><dd>Cloned with the system <code>git</code> (<code>MITTS_GIT</code>) into <code>&lt;cache&gt;/git</code>: a bare mirror under <code>db/</code> and one checkout per locked commit. Authentication is git's own (SSH agent, credential helpers).</dd>
<dt>Compiler wrappers</dt><dd>See <a href="#wrappers">Compiler wrappers</a>: any wrapper except <code>sccache</code> disables caching by default, unless <code>MITTS_RUSTC_WRAPPER_CACHE=1</code> is set.</dd>
</dl></section>
"""


CI = """
<section id="github-actions"><h2>GitHub Actions</h2>
<p>Two small composite actions install a verified release and keep the build cache between runs.</p>""" + pre("steps:\n  - uses: actions/checkout@v4\n  - uses: dtolnay/rust-toolchain@stable\n  - uses: alex09x/mitts@v{VERSION}\n  - run: mitts test\n  - uses: alex09x/mitts/save@v{VERSION}\n    if: always()") + """
<p>Pin the action to a release tag or a full commit SHA. The setup action installs the release you ask for with the installer that ships in the same ref, and the installer checks the archive against the published SHA-256 sums.</p>
<h4>What gets cached</h4>
<p>The whole Mitts cache directory (store and action cache, by default <code>~/.cache/mitts</code>), and nothing else: the Cargo registry and <code>target/</code> do not need restoring. The key is <code>mitts-v1</code>, the runner OS and architecture, the Mitts version, your <code>cache-key</code> text and a hash of every <code>Cargo.lock</code>. On a miss the newest cache with the same prefix is restored, so a changed dependency set rebuilds only what changed.</p>
<h4>Setup action: <code>alex09x/mitts</code></h4>
<table class="ref"><thead><tr><th>Input</th><th>Default</th><th>Meaning</th></tr></thead><tbody>
<tr><td class="flag"><code>version</code></td><td>latest stable</td><td>Release to install, <code>vX.Y.Z</code> or <code>X.Y.Z</code>.</td></tr>
<tr><td class="flag"><code>cache</code></td><td><code>true</code></td><td>Restore the cache. <code>false</code> only installs.</td></tr>
<tr><td class="flag"><code>cache-key</code></td><td>empty</td><td>Extra text in the key, for example a job name, when jobs build different things.</td></tr>
<tr><td class="flag"><code>cache-dir</code></td><td>platform default</td><td>Cache root. When empty: <code>MITTS_CACHE_DIR</code>, <code>XDG_CACHE_HOME/mitts</code>, <code>~/.cache/mitts</code>.</td></tr>
</tbody></table>
<p>Outputs: <code>version</code>, <code>cache-dir</code> (also exported as <code>MITTS_CACHE_DIR</code> for the rest of the job) and <code>cache-hit</code> (<code>true</code> when the exact key was restored).</p>
<h4>Save action: <code>alex09x/mitts/save</code></h4>
<p>Runs <code>mitts gc</code> to bound the store, then saves it; nothing is saved after an exact hit. <code>max-bytes</code> (default 2&nbsp;GiB) is the size the store is pruned to first, oldest objects first. Use <code>if: always()</code> to keep the work of a failed build, and run it after the setup action in the same job.</p>
<p class="hint">Runners: Linux x86-64 and ARM64, macOS on Apple Silicon. A Rust toolchain and a native linker must already be present. GitHub keeps 10&nbsp;GB of caches per repository; lower <code>max-bytes</code> when workflows share that budget.</p>
</section>
"""


def build(binary, version):
    how = docs_how.sections()
    cmds = [command_section(binary, n) for n in C.COMMANDS]
    toc = [("Start", [("install", "Install"), ("quick-start", "Quick start")]),
           ("How it works", [("how-it-works", "Overview")] + [(i, t) for i, t, _ in how]),
           ("Commands", [("commands", "Overview")] + [("cmd-" + n, "mitts " + n) for n in C.COMMANDS]),
           ("Reference", [("environment", "Environment variables"), ("output-formats", "Output formats"), ("configuration", "Configuration"), ("github-actions", "GitHub Actions")])]
    side = "".join("<p class=\"toc-h\">%s</p><ul>%s</ul>" % (g, "".join("<li><a href=\"#%s\">%s</a></li>" % (i, esc(t)) for i, t in items)) for g, items in toc)
    how_html = "".join("<section id=\"%s\"><h3>%s</h3>%s</section>" % (i, esc(t), h) for i, t, h in how)
    by_group = ""
    for g in GROUP_ORDER:
        by_group += "<h3 class=\"group\">%s</h3>" % g + "".join(h for n, h in cmds if C.COMMANDS[n][0] == g)
    body = """
<main class="shell docs">
<aside class="toc" aria-label="Documentation contents">%(side)s</aside>
<article class="doc">
<header class="doc-head"><p class="kicker"><span class="dot"></span>Documentation &middot; mitts %(v)s</p><h1>Mitts documentation</h1>
<p class="lede">How to install and use Mitts, how the machine-wide cache decides what to rebuild, and every command, option and environment variable. The option tables are generated from the installed binary, so they match this version.</p></header>
%(intro)s
<section id="how-it-works"><h2>How it works: the global cache</h2>
<p>This section describes the build and cache protocol as implemented. Where the implementation has a known gap, the gap is stated next to the rule it weakens.</p>
%(how)s</section>
<section id="commands"><h2>Commands</h2>
<p>Every command accepts <code>-h</code> / <code>--help</code>. Options marked in the tables are generated from the binary; the paragraph under each option adds what the one-line help cannot say. Exit codes: <code>0</code> success, <code>101</code> a test failed, <code>2</code> a usage error.</p>
%(cmds)s</section>
<section id="environment"><h2>Environment variables</h2>
<p>Flags always win over variables. The diagnostic variables change how the work is done, never what is cached.</p>%(env)s</section>
%(out)s%(conf)s%(ci)s
</article>
</main>""" % {"side": side, "v": version, "intro": INTRO, "how": how_html, "cmds": by_group, "env": env_table(), "out": OUTPUT, "conf": CONFIG, "ci": CI.replace("{VERSION}", version)}
    return page("Mitts documentation", "Install Mitts, understand the machine-wide build cache, and look up every command, option and environment variable.", body, 1, active="Docs")
