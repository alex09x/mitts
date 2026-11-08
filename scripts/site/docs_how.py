# Mitts Build — a Rust-first build system with a machine-wide, content-addressed artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Documentation: the mechanism of the machine-wide cache. Facts follow docs/architecture.md and docs/cache-limits.md."""

SHARED_STORE_SVG = """<svg class="diagram" viewBox="0 0 860 330" role="img" aria-label="Three workspaces restore objects from one machine-wide store">
<rect class="d-box" x="20" y="20" width="220" height="80" rx="6"/><text class="d-t" x="130" y="52" text-anchor="middle">project-a/target</text><text class="d-s" x="130" y="76" text-anchor="middle">workspace (mutable)</text>
<rect class="d-box" x="20" y="125" width="220" height="80" rx="6"/><text class="d-t" x="130" y="157" text-anchor="middle">project-a worktree 2</text><text class="d-s" x="130" y="181" text-anchor="middle">another checkout, same code</text>
<rect class="d-box" x="20" y="230" width="220" height="80" rx="6"/><text class="d-t" x="130" y="262" text-anchor="middle">project-b/target</text><text class="d-s" x="130" y="286" text-anchor="middle">other project, same serde</text>
<rect class="d-store" x="420" y="20" width="420" height="290" rx="8"/>
<text class="d-t" x="630" y="50" text-anchor="middle">~/.cache/mitts (one per machine)</text>
<rect class="d-box" x="445" y="68" width="370" height="64" rx="5"/><text class="d-t" x="630" y="94" text-anchor="middle">ac/manifests</text><text class="d-s" x="630" y="116" text-anchor="middle">action key → list of output objects</text>
<rect class="d-box" x="445" y="146" width="370" height="64" rx="5"/><text class="d-t" x="630" y="172" text-anchor="middle">cas/objects/&lt;sha256&gt;</text><text class="d-s" x="630" y="194" text-anchor="middle">immutable, read-only (0444)</text>
<rect class="d-box" x="445" y="224" width="370" height="64" rx="5"/><text class="d-t" x="630" y="250" text-anchor="middle">sources/ &middot; index/ &middot; locks/ &middot; jobserver/</text><text class="d-s" x="630" y="272" text-anchor="middle">unpacked crates, registry index, single-flight locks</text>
<path class="d-arrow" d="M240 60 H420"/><path class="d-arrow" d="M240 165 H420"/><path class="d-arrow" d="M240 270 H420"/>
<text class="d-s" x="330" y="52" text-anchor="middle">restore</text><text class="d-s" x="330" y="157" text-anchor="middle">restore</text><text class="d-s" x="330" y="262" text-anchor="middle">restore</text>
</svg>"""

KEY_SVG = """<svg class="diagram" viewBox="0 0 860 300" role="img" aria-label="Inputs of a compilation unit are hashed into one action key">
<g class="d-in"><rect x="20" y="14" width="300" height="34" rx="4"/><text x="34" y="36">toolchain: rustc -vV</text>
<rect x="20" y="56" width="300" height="34" rx="4"/><text x="34" y="78">target triple · profile</text>
<rect x="20" y="98" width="300" height="34" rx="4"/><text x="34" y="120">flags (in invocation order)</text>
<rect x="20" y="140" width="300" height="34" rx="4"/><text x="34" y="162">features (sorted)</text>
<rect x="20" y="182" width="300" height="34" rx="4"/><text x="34" y="204">sources: (path, content hash)</text>
<rect x="20" y="224" width="300" height="34" rx="4"/><text x="34" y="246">dependency action keys</text>
<rect x="20" y="266" width="300" height="28" rx="4"/><text x="34" y="285">tracked env: CARGO_*, RUSTFLAGS, env!()</text></g>
<path class="d-arrow" d="M320 154 H430"/>
<rect class="d-store" x="430" y="104" width="170" height="100" rx="8"/><text class="d-t" x="515" y="148" text-anchor="middle">SHA-256</text><text class="d-s" x="515" y="172" text-anchor="middle">canonical text</text>
<path class="d-arrow" d="M600 154 H680"/>
<rect class="d-box" x="680" y="114" width="160" height="80" rx="6"/><text class="d-t" x="760" y="148" text-anchor="middle">action key</text><text class="d-s" x="760" y="172" text-anchor="middle">one per unit</text>
</svg>"""

FLOW_SVG = """<svg class="diagram" viewBox="0 0 860 300" role="img" aria-label="Unit execution: lookup, hit restore, or miss lock, compile, publish">
<rect class="d-box" x="20" y="120" width="150" height="60" rx="6"/><text class="d-t" x="95" y="156" text-anchor="middle">look up key</text>
<path class="d-arrow" d="M170 150 H250"/>
<rect class="d-hit" x="250" y="30" width="250" height="70" rx="6"/><text class="d-t" x="375" y="58" text-anchor="middle">hit</text><text class="d-s" x="375" y="82" text-anchor="middle">verify, restore to target/</text>
<rect class="d-box" x="250" y="200" width="250" height="70" rx="6"/><text class="d-t" x="375" y="228" text-anchor="middle">miss</text><text class="d-s" x="375" y="252" text-anchor="middle">take the key lock, look again</text>
<path class="d-arrow" d="M210 140 L250 65"/><path class="d-arrow" d="M210 160 L250 235"/>
<path class="d-arrow" d="M500 235 H560"/>
<rect class="d-box" x="560" y="200" width="130" height="70" rx="6"/><text class="d-t" x="625" y="240" text-anchor="middle">rustc</text>
<path class="d-arrow" d="M690 235 H730"/>
<rect class="d-box" x="730" y="200" width="110" height="70" rx="6"/><text class="d-t" x="785" y="228" text-anchor="middle">publish</text><text class="d-s" x="785" y="250" text-anchor="middle">tmp, 0444, rename</text>
</svg>"""


def sections():
    """Return a list of (id, title, html)."""
    S = []
    S.append(("principles", "Principles", """
<p>The cache follows four rules. Everything else on this page is their consequence.</p>
<ol>
<li><b>A cache hit needs a complete key.</b> Every input that can change an output is part of the action key. An input Mitts cannot see is a reason to rebuild, never a reason to reuse.</li>
<li><b>Shared objects are immutable.</b> A published object is read-only and is verified by hash before it is used. Workspaces get their own copy, reflink or read-only hardlink, never a writable alias.</li>
<li><b>The language core is neutral.</b> The store, the action cache, the locks and the garbage collector know nothing about Cargo. A Rust adapter turns a Cargo workspace into actions.</li>
<li><b>Cargo stays the contract.</b> <code>Cargo.toml</code> and <code>Cargo.lock</code> are read as they are. Mitts never writes a lockfile of its own. Outputs go to Cargo's usual <code>target/debug</code> and <code>target/release</code>, and Mitts records the files it put there so <code>mitts clean</code> removes only those.</li>
</ol>"""))
    S.append(("global-cache", "The global cache in one picture", """
<p>Cargo keeps its cache in each workspace's <code>target/</code>, so every new clone, worktree or project starts empty. Mitts keeps compiled work in <b>one store per machine</b> and restores it into whichever workspace asks for it.</p>
""" + SHARED_STORE_SVG + """
<ul>
<li>The <b>action cache</b> (<code>ac/manifests</code>) maps an action key to the list of objects that unit produced.</li>
<li>The <b>content-addressed store</b> (<code>cas/objects/&lt;sha256&gt;</code>) holds those objects. Two units that produce identical bytes share one object.</li>
<li>A <b>workspace</b> (<code>target/</code>) is a mutable view. Deleting it loses nothing: the next build restores from the store.</li>
</ul>
<p>Two clones, two worktrees, two projects that use the same crate at the same version with the same inputs compile it once, on this machine, and every later build restores it.</p>"""))
    S.append(("pipeline", "The pipeline", """
<p>A build runs six phases. A warm build skips most of them (see the <a href="#fast-path">no-op fast path</a>).</p>
<table class="ref"><thead><tr><th>Phase</th><th>What happens</th></tr></thead><tbody>
<tr><td><code>resolve</code></td><td>Reuse <code>Cargo.lock</code> when it matches the manifests. Otherwise resolve against the crates.io sparse index and write a standard <code>Cargo.lock</code>. Registries, mirrors, vendored directories, git dependencies and <code>[patch]</code> follow Cargo's configuration.</td></tr>
<tr><td><code>fetch</code></td><td>Download missing <code>.crate</code> archives in parallel, verify each SHA-256 against <code>Cargo.lock</code>, unpack once per machine into <code>sources/</code>.</td></tr>
<tr><td><code>plan</code></td><td>Build the compilation units (library, binary, test, build script, proc macro) with features, targets and dependency edges, using Cargo's resolver rules.</td></tr>
<tr><td><code>key</code></td><td>Derive one action key per unit, in topological order.</td></tr>
<tr><td><code>execute</code></td><td>Run the unit graph in parallel: restore hits, compile misses.</td></tr>
<tr><td><code>publish</code></td><td>Store each new output in the CAS, then record the action cache entry.</td></tr>
</tbody></table>
<p><code>mitts explain</code> prints the plan and the key of every unit without compiling anything. <code>mitts fetch</code> runs only the first two phases.</p>"""))
    S.append(("action-keys", "Action keys: what decides a hit", """
<p>An action key is the SHA-256 of a canonical text made of everything that can change the output of a unit.</p>
""" + KEY_SVG + """
<pre><code>toolchain: &lt;rustc -vV&gt;
target:    &lt;target triple&gt;
profile:   &lt;dev | release | ...&gt;
flags:     &lt;unit flags, in invocation order&gt;
features:  &lt;sorted feature list&gt;
sources:   &lt;sorted (path, content hash) pairs&gt;
deps:      &lt;sorted (dependency unit, dependency action key) pairs&gt;</code></pre>
<dl class="params">
<dt>Sources</dt><dd>For a registry dependency: the package files that came with the download. For a workspace unit, tracking is per unit. <i>Package inputs</i> are manifests, lockfiles, applicable <code>.cargo/config.toml</code> and <code>clippy.toml</code>, and root files including <code>build.rs</code>. <i>Unit inputs</i> are the <code>src/</code> tree for libraries and binaries, and only the declared <code>src_path</code> for integration tests, benchmarks and examples, so editing one integration test never invalidates its siblings or the library. <i>Extra inputs</i> (<code>include_str!</code>, <code>include_bytes!</code>, <code>#[path]</code> targets, <code>tests/support/</code>) are read from rustc's dep-info after the first compile and recorded in the manifest.</dd>
<dt>Environment</dt><dd><code>CARGO_*</code>, <code>RUST_*</code>, <code>RUSTFLAGS</code>, <code>RUSTDOCFLAGS</code>, <code>RUSTC_WRAPPER</code>, <code>RUSTC_WORKSPACE_WRAPPER</code>, <code>TARGET</code>, <code>HOST</code> and <code>PROFILE</code> enter the flags. Variables a crate reads through <code>env!</code> or <code>option_env!</code> are discovered per unit and keyed with their values; an unset variable is recorded as <code>&lt;unset&gt;</code>.</dd>
<dt>Dependencies</dt><dd>A unit's key contains its dependencies' <i>keys</i>, not their output bytes. A change anywhere below a unit therefore changes every key above it, and nothing else.</dd>
<dt>Anything unknown</dt><dd>An input Mitts cannot account for makes the unit uncached or explicitly unsupported. It is never treated as unchanged.</dd>
</dl>"""))
    S.append(("worktrees", "Why a second checkout reuses everything", """
<p>An absolute path in a key would make every clone a miss. Mitts compiles workspace crates with <code>--remap-path-prefix=&lt;workspace_root&gt;/=</code>, so diagnostics, <code>file!()</code>, panic locations and debug info use relative paths such as <code>src/main.rs</code>. A unit that qualifies receives a <b>path-independent key</b> and hits across independent checkouts, clones and agent worktrees.</p>
<p>A unit qualifies when it:</p>
<ul>
<li>does not read <code>CARGO_MANIFEST_DIR</code> at compile time through <code>env!</code> or <code>option_env!</code>;</li>
<li>has no custom <code>build.rs</code>;</li>
<li>does not depend on a proc macro the project declared impure;</li>
<li>lives inside the workspace root (not <code>../shared</code>).</li>
</ul>
<p>Units that do not qualify keep the local absolute path in their key and are rebuilt per checkout. That is slower, not wrong.</p>
<p><b>Proc macros are pure by default.</b> A project lists the ones that read package files or the package path while expanding (<code>sqlx</code>, <code>askama</code>, <code>rust-embed</code>, a local derive) under <code>[workspace.metadata.mitts] impure-proc-macros</code> in the root <code>Cargo.toml</code>. Their direct dependents are then keyed on the whole package tree and on the absolute path. There is no built-in list.</p>
<p><b>Defence in depth.</b> Before publishing, Mitts scans every generated artifact for the bytes of the workspace root path. If it finds them, the artifact is never published under the path-independent key.</p>
<p><b>macOS linking.</b> ld64 records each linked object's absolute path in the output's debug stabs, where <code>--remap-path-prefix</code> does not reach. Mitts passes <code>-Wl,-oso_prefix,&lt;workspace root&gt;/</code> so the stabs read <code>target/debug/deps/lib....rlib</code> in every checkout, and a debugger started in the workspace still resolves them.</p>"""))
    S.append(("store-layout", "Store layout", """
<p>One resolver picks the root of every store, in this order: <code>--cache-dir</code>, <code>$MITTS_CACHE_DIR</code>, <code>$XDG_CACHE_HOME/mitts</code>, <code>~/.cache/mitts</code>. Setting it relocates all of them together; nothing falls back to the default on its own.</p>
<pre><code>&lt;cache root&gt;/
&boxvr;&boxh;&boxh; cas/
&boxv;   &boxvr;&boxh;&boxh; objects/&lt;sha256&gt;          immutable objects, mode 0444
&boxv;   &boxvr;&boxh;&boxh; artifacts/                named views of objects for rustc -L / --extern
&boxv;   &boxvr;&boxh;&boxh; build_scripts/&lt;base&gt;/&lt;exec&gt;/{meta.toml, out/}
&boxv;   &boxur;&boxh;&boxh; tmp/                      staging for atomic publish
&boxvr;&boxh;&boxh; ac/manifests/                 action cache entries
&boxvr;&boxh;&boxh; locks/                        single-flight lock per action key
&boxvr;&boxh;&boxh; jobserver/                    managed token pool
&boxvr;&boxh;&boxh; sources/&lt;crate&gt;-&lt;version&gt;/    unpacked registry sources
&boxvr;&boxh;&boxh; git/                          bare mirrors and checkouts of git dependencies
&boxur;&boxh;&boxh; index/                        cached sparse-index entries</code></pre>
<p>The <b>workspace output</b> is Cargo's <code>target/</code>, or <code>$CARGO_TARGET_DIR</code>, or <code>--target-dir</code> (which wins). Mitts creates <code>CACHEDIR.TAG</code> and a <code>.gitignore</code> there. <code>mitts clean</code> removes only the indexed outputs that still match their recorded metadata, so unrelated files in a shared target directory are left alone.</p>"""))
    S.append(("executing", "Executing a unit: hit, miss, publish", """
""" + FLOW_SVG + """
<ol>
<li><b>Look up</b> the action key in the action cache.</li>
<li><b>Hit.</b> Verify the objects, then restore them into the workspace: a reflink where the filesystem supports it (APFS, XFS with reflink, Btrfs), otherwise a read-only hardlink to the immutable object, otherwise a copy. A corrupt object is discarded and the unit is rebuilt.</li>
<li><b>Miss.</b> Take the per-key lock, then look the key up <i>again</i>. Another process may have finished the unit while this one waited (double-checked locking). If it is still missing, compile.</li>
<li><b>Publish.</b> Copy each output into <code>cas/tmp/</code>, mark it read-only, then <code>rename</code> it into <code>objects/</code>. The action cache entry is written only after every output is stored, so an interrupted build never leaves a partial object or an entry pointing at one.</li>
</ol>
<p>By default a restore trusts objects that were published atomically and read-only, and does not re-read their bytes. <code>MITTS_VERIFY_RESTORE=1</code> re-hashes every object before restoring it, for audits or suspect disks.</p>"""))
    S.append(("concurrency", "Concurrency: one compile for many builds", """
<ul>
<li>Units run in waves over a work queue. A worker first tries the key lock without blocking. If another build holds it, the unit is marked <i>contested</i> and the worker moves on to other ready units, returning later.</li>
<li>Two terminals, worktrees or agents building the same dependency therefore compile it <b>once</b>. The second build waits for, then restores, the first one's result.</li>
<li><code>rustc</code> is pipelined: dependents start from the <code>.rmeta</code> as soon as metadata is ready, before the <code>.rlib</code> is finished.</li>
<li>Parallelism is bounded by a <b>jobserver shared by all Mitts processes on the machine</b>, so ten simultaneous builds do not start ten times the cores. An inherited <code>--jobserver-auth</code> from a parent build is respected. <code>-j N</code> or <code>MITTS_JOBS=N</code> lowers the limit.</li>
</ul>"""))
    S.append(("build-scripts", "Build scripts", """
<p>Build scripts are compiled and run by Mitts, and their results are cached too.</p>
<dl class="params">
<dt>Base key</dt><dd>The script's sources, package, features, target, profile, tracked environment and build-dependency keys.</dd>
<dt>Execution entry</dt><dd>The directives it printed (<code>rustc-cfg</code>, <code>rustc-link-lib</code>, <code>rustc-env</code>, ...), its <code>OUT_DIR</code>, and the evidence it declared: content hashes of <code>rerun-if-changed</code> paths and values of <code>rerun-if-env-changed</code> variables.</dd>
<dt>Reuse rule</dt><dd>A cached run is reused only if every declared path has the same content hash and every declared variable the same value. Otherwise the script runs again.</dd>
<dt>Dynamic key</dt><dd>The script runs <i>first</i>, in an isolated staging directory, before the unit's lock. The compile-affecting directives are canonicalised and hashed into the unit's key (<code>&lt;base_key&gt;:bd:&lt;directives_hash&gt;</code>), so two builds that obtain different directives never share a compile.</dd>
<dt>Limit</dt><dd>The staging directory is not an OS sandbox. A script that reads files or variables it did not declare can still cause a stale hit. As with Cargo, declaring inputs is the script author's job.</dd>
</dl>"""))
    S.append(("fast-path", "The no-op fast path", """
<p>After a successful build Mitts writes <code>target/mitts/build-&lt;profile&gt;.lock</code>: the toolchain, the package selection, the features, and stat snapshots (path, size, mtime) of <code>Cargo.lock</code>, the manifests, the source files and directories, and the outputs.</p>
<p>On the next build, if every snapshot still matches, Mitts skips planning and keying and returns at once. Directory snapshots catch added and removed files. Any mismatch falls through to the normal keyed build, so the fast path can only skip work the full protocol would also skip. It is local to one workspace and never shared. With an explicit <code>--cache-dir</code> it is off unless <code>MITTS_FAST_LOCK=1</code>.</p>"""))
    S.append(("gc", "Size limit and garbage collection", """
<p>The store is bounded by default, with no cron job. After a command that uses the cache (<code>build</code>, <code>check</code>, <code>clippy</code>, <code>test</code>, <code>bench</code>, <code>coverage</code>, <code>llvm-cov</code>, <code>run</code>, <code>rustc</code>), Mitts checks the size of the store and, over the limit, prunes the oldest objects until it is under it.</p>
<ul>
<li>Objects of a running build are never pruned; pruning waits until no build holds the cache.</li>
<li>Pruning goes by file modification time, oldest first. A pruned object costs one recompile, nothing else.</li>
<li><code>MITTS_CACHE_MAX_BYTES</code> sets the limit (<code>500M</code>, <code>2G</code>, or <code>off</code> / <code>0</code>); the default is 100&nbsp;GiB. <code>MITTS_GC_INTERVAL_SECS</code> is the least time between checks (default 3600).</li>
<li>Build-script entries are removed by an atomic rename to <code>.gc.*</code> and then deleted, so a concurrent reader sees the whole entry or none of it. Leftover <code>tmp</code> files older than one hour go too.</li>
<li><code>mitts gc --max-bytes N --max-age-secs S</code> prunes on demand.</li>
<li>A build with <code>--no-cache</code> uses a private <code>cas-ephemeral-&lt;id&gt;</code> store, removed by the next build or by <code>mitts gc</code> after the process has exited, and adds nothing to the shared one.</li>
</ul>"""))
    S.append(("wrappers", "Compiler wrappers", """
<p>A <code>rustc-wrapper</code> in the Cargo config, or <code>RUSTC_WRAPPER</code>, <code>CARGO_BUILD_RUSTC_WRAPPER</code>, <code>RUSTC_WORKSPACE_WRAPPER</code> or <code>CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER</code>, is an input Mitts cannot know: it may change what the compiler produces. By default a build with a wrapper is therefore <b>not cached</b>, and Mitts prints a warning that names the wrapper and how to clear it. Its <code>-C metadata</code> and <code>-C extra-filename</code> stay stable from run to run.</p>
<p><b>Opt in.</b> If your wrapper is deterministic (a pass-through, a launcher, an analysis tool), set <code>MITTS_RUSTC_WRAPPER_CACHE=1</code>. The wrapper's path, canonical path and executable SHA-256 then go into every key, together with <code>MITTS_RUSTC_WRAPPER_SALT</code> if you set one, so a different or changed wrapper never reuses entries made by another. If the wrapper cannot be resolved or hashed, the build is not cached (it fails closed).</p>
<p><code>sccache</code> is the one exception. It caches compiler results without changing them, so Mitts leaves it out, compiles without it, and reuses units from the Mitts store.</p>"""))
    S.append(("cargo-compat", "Compatibility with Cargo", """
<ul>
<li>Mitts writes into the same <code>target/debug</code> and <code>target/release</code> layout as Cargo (honouring <code>CARGO_TARGET_DIR</code> and <code>--target-dir</code>); its own lock and file index live under <code>target/mitts/</code>.</li>
<li><code>mitts build --cargo</code> delegates execution to Cargo. It is the differential oracle Mitts uses to compare diagnostics, exit codes and program behaviour.</li>
<li>Unsupported cases are reported as unsupported, not as a successful build.</li>
</ul>"""))
    S.append(("limits", "Not supported yet", """
<ul>
<li>A remote or team cache. The planned design is a second store tier over S3 or HTTP, written only by trusted CI.</li>
<li>Cross-compilation beyond the host target.</li>
<li>Windows (locks and the jobserver are POSIX-only today).</li>
<li>Empty or omitted <code>workspace.members</code> means the root package only; path dependencies outside it must be listed. Wildcards in intermediate directories (<code>crates/*/sub</code>) and character-class globs fail closed.</li>
</ul>"""))
    return S
