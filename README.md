<p align="center">
  <img src="site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>Build work, accounted for.</b><br>
  A Rust-first build system with a machine-wide, content-addressed artifact cache
  and explicit input tracking. It reads your existing <code>Cargo.toml</code> and <code>Cargo.lock</code>.
</p>

<p align="center">
  <a href="https://mittsbuild.com"><img src="https://img.shields.io/badge/site-mittsbuild.com-ff3b30" alt="Website"></a>
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-c9ced4" alt="License: MIT OR Apache-2.0">
  <img src="https://img.shields.io/badge/language-Rust-1b1d20?logo=rust" alt="Written in Rust">
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS-1b1d20" alt="Platforms: Linux, macOS">
  <img src="https://img.shields.io/badge/status-pre--release-ff3b30" alt="Status: pre-release">
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="docs/architecture.md">Protocol</a> ·
  <a href="#limitations">Limitations</a> ·
  <a href="ROADMAP.md">Roadmap</a>
</p>

---

Mitts plans a Cargo workspace itself and invokes `rustc` directly. Every
compile action gets a key derived from all of its inputs. The output is stored
once in a machine-wide immutable store and restored into every workspace that
needs it, across projects, worktrees and branches.

If an input cannot be accounted for, the action is rebuilt, not guessed.

## The problem

Cargo's build cache lives in each workspace's `target/` directory. Anything
that creates a new `target/` starts from zero:

- a second checkout or `git worktree` of the same repository,
- switching to a branch that changes `Cargo.lock`, then switching back,
- another project that depends on the same crates with the same features,
- parallel coding agents, each working in its own worktree.

The same `serde`, `syn` or `tokio` gets compiled again and again, and every copy
takes disk space.

The usual workarounds have sharp edges:

- **A shared `CARGO_TARGET_DIR`** serializes builds behind one lock, and
  projects with different feature sets overwrite each other's artifacts.
- **A compiler wrapper such as sccache** sees one `rustc` call at a time. Its
  key depends on absolute paths unless they are normalized, and crates that
  invoke the linker (`bin`, `proc-macro`, `dylib`) are not cached. Cargo still
  plans the build and runs every build script.

Mitts takes a different route: it replaces Cargo's executor, not the compiler.
It sees the whole unit graph, so it can key each unit by its dependencies' keys,
cache linking units and build script runs, and let several builds share one
store safely.

## Highlights

- **Reuse across workspaces.** One store per machine. A dependency compiled for
  one checkout is restored into the next one with the same inputs.
- **Complete keys.** Toolchain, target, profile, flags, features, source
  contents, dependency keys and tracked environment. Unknown inputs mean a
  rebuild.
- **Cargo stays the contract.** No new manifest, no second lockfile. The same
  project still builds with `cargo build` in CI, and Cargo and rust-analyzer
  keep working next to Mitts.
- **Linking units and build scripts are cached.** Binaries, tests, proc macros
  and build-script executions are keyed and stored, not only `rlib`s.
- **Concurrent builds cooperate.** Per-key locks: two terminals or worktrees
  that need the same dependency compile it once. One jobserver bounds
  parallelism for all Mitts processes on the machine.
- **Safe sharing.** Store objects are immutable: published atomically, stored
  read-only and restored by reflink or copy. `MITTS_VERIFY_RESTORE=1` re-hashes every
  object before it is restored (the default trusts the store, which saves one read of
  every restored byte). Where the filesystem cannot reflink, a restore hardlinks the
  read-only store object into `target/` instead of copying its bytes: the shared inode
  stays `0o444` (`0o555` for executables), the target index rejects a link that gained
  write bits, and Mitts only ever replaces workspace files by rename. Never a writable
  link to a store object. `MITTS_STORE_HARDLINKS=0` keeps private copies for workspaces
  that other tools write into in place.
- **Zero-copy workspace staging.** Intra-workspace library aliases, secondary binaries,
  and atomic unit staging use intra-workspace hardlinks and rename promotion without
  copying bytes. When `MITTS_CACHE_DIR` and the workspace `target/` reside on the same
  filesystem volume, staging promotions operate without copying bytes; across separate
  mounts, they transparently fall back to safe copying.
- **A no-op fast path.** An unchanged workspace skips planning and keying.
- **One CLI.** `build`, `run`, `check`, `clippy`, `fmt`, `test` (with doctests),
  `coverage`, `explain`, `fetch`, `gc`, `clean`.

## Quick start

Install Mitts from crates.io with a recent stable Rust:

```sh
cargo install mitts --locked
```

Or build from source:

```sh
git clone https://github.com/alex09x/mitts
cd mitts
cargo install --path . --locked
```

Then run it inside any Cargo workspace:

```sh
mitts build                  # compile into target/mitts/
mitts build --release
mitts run                    # build and run a binary
mitts test                   # unit, integration and doc tests
mitts test --tests -p foo    # Cargo-style target selection
mitts check | clippy | fmt | coverage
mitts explain                # the plan, units and why each is cached or not
mitts fetch                  # prefetch dependencies into the source store
mitts gc                     # prune the shared store within quotas
mitts clean                  # remove this workspace's Mitts artifacts
```

Mitts writes only to `target/mitts/` in the workspace and to its shared cache
(`--cache-dir`, else `$MITTS_CACHE_DIR`, else `$XDG_CACHE_HOME/mitts`, else
`~/.cache/mitts`). The artifact store, action cache, locks, jobserver, source
store and index cache all live under that one root.

## How it works

```
Cargo.toml + Cargo.lock
        │
        ▼
 resolve ─── reuse Cargo.lock, or resolve against the crates.io sparse index
        │
        ▼
 fetch ───── download .crate archives in parallel, verify, unpack once per machine
        │
        ▼
 plan ────── compilation units: lib · bin · test · build script · proc macro
        │    with features, targets and dependency edges (Cargo's resolver rules)
        ▼
 key ─────── SHA-256 over toolchain, target, profile, flags, features,
        │    source contents, dependency keys and tracked environment
        ▼
 execute ─── parallel DAG of direct rustc calls, pipelined on .rmeta
             hit  → verify and restore from the store (reflink or copy)
             miss → lock the key, compile, publish atomically to the store
```

A unit's key includes its dependencies' **keys**, not their output bytes, so a
change anywhere below a unit changes its key. Build scripts run in a staging
directory; their directives (`rustc-cfg`, `rustc-link-*`, `rustc-env`, …) enter
the key of the unit they configure, and their `rerun-if-*` declarations decide
when a cached run can be reused.

After a successful build Mitts records a snapshot of the workspace. If nothing
in it changed, the next build skips straight to the result. Any mismatch falls
back to the full keyed build, so the fast path never skips work the protocol
would do.

The full protocol — key derivation, locking, build scripts, the fast path,
store layout and garbage collection — is in
[docs/architecture.md](docs/architecture.md).

## Architecture

Mitts is a language-neutral core with language adapters on top. The core knows
nothing about Cargo; the Rust adapter turns a Cargo workspace into keyed
actions.

```
                 ┌──────────────────────────────────────────────┐
  mitts CLI ───▶ │ Rust adapter                                 │
  (src/cli)      │ manifest · resolver · fetcher · features     │
                 │ planner · keys · DAG runner · build scripts  │
                 ├──────────────────────────────────────────────┤
                 │ C/C++ adapter (experimental, Mitts.toml)     │
                 └──────────────────────┬───────────────────────┘
                                        │ actions + keys
                 ┌──────────────────────▼───────────────────────┐
                 │ Core                                         │
                 │ content-addressed store · action cache       │
                 │ key locks · jobserver · fast path · GC       │
                 └──────────────────────────────────────────────┘
```

| Path | What lives there |
| --- | --- |
| `src/cli/` | Command handlers: `build`, `test`, `clippy`, `explain`, … |
| `src/adapter/rust/` | Cargo semantics: manifests, lockfile, sparse index, feature resolution, unit planning, action keys, the DAG runner, build scripts, doctests |
| `src/adapter/c/` | Experimental C/C++ adapter on the same store |
| `src/core/` | Language-neutral store (`cas.rs`), action cache, locks, jobserver, fast path (`build_lock*.rs`), GC, reflinks |
| `docs/` | [Protocol](docs/architecture.md), [design decisions](docs/decisions), [scenarios](docs/scenarios) |
| `tests/` | Integration tests, including Cargo-parity and cache-invalidation tests |
| `scripts/` | Benchmark and audit harness |
| `fixtures/` | Minimal Rust and C projects used by tests |

## What invalidates the cache

A cache hit requires every input below to match. If Mitts cannot see an
input, it does not reuse the result.

| Input | Tracked as |
| --- | --- |
| Toolchain | `rustc -vV` output |
| Target and profile | target triple, profile settings, `RUSTFLAGS` and Cargo config rustflags |
| Features | the unit's resolved feature set (resolver 1 or 2, as Cargo would) |
| Sources | content hashes of the package files and every file rustc reports reading (`include_str!`, `include_bytes!`, modules) |
| Dependencies | the action keys of all dependency units |
| Environment | `CARGO_*`, `RUST_*`, wrappers, and every variable read through `env!` / `option_env!`, with `<unset>` recorded explicitly |
| Cargo configuration | the `.cargo/config(.toml)` files Cargo would read |
| Build scripts | their directives, `OUT_DIR`, and the content of `rerun-if-changed` paths and values of `rerun-if-env-changed` variables |

Some proc macros read files from your package while they expand: `sqlx` checks
queries against `.sqlx/`, `askama` compiles `templates/`, `rust-embed` and
`include_dir` embed whole directories. rustc only learns about such reads when
the macro registers them, so declare these crates in the root `Cargo.toml`:

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

Crates that depend on a listed macro are keyed on every file in their package
and on their absolute path: an edited template rebuilds them, and another
checkout never reuses them. Every other macro is treated as pure, and its
dependents are shared across worktrees. Name the crate you depend on; the
macro crate it pulls in (`sqlx-macros`) works too.

## Comparison

A qualitative comparison of how each tool reuses work. It is not a
performance claim.

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| Reads `Cargo.toml` / `Cargo.lock` as is | yes | yes | yes |
| Reuse across workspaces and worktrees | no (per `target/`) | yes, per `rustc` call | yes, per unit |
| Key depends on the checkout path | — | yes, unless normalized | no for registry crates; for workspace crates only when not eligible |
| Caches `bin`, test and `proc-macro` units | — | no | yes |
| Caches build-script runs | no | no | yes |
| Concurrent builds of one dependency | wait on the `target/` lock | both may compile on a miss | one compiles, the other reuses |
| Remote / team cache | no | yes (S3, GCS, Redis, …) | planned |
| Windows | yes | yes | no |

Bazel and Buck2 solve a larger problem — hermetic, multi-language builds with
remote execution — but require their own build files instead of `Cargo.toml`.

## Limitations

Mitts is pre-release. Know these before you rely on it:

- **Linux and macOS only.** Locks and the jobserver are POSIX.
- **Host target only.** Cross-compilation is not supported yet.
- **No remote cache yet.** The store is per machine.
- **Some units are never cached:** crates of type `cdylib`, `dylib` or
  `staticlib`, and crates whose `env!` reads cannot be tracked. They always
  build; they are reported by `mitts explain`.
- **Workspace crates across checkouts.** Your own crates are reused across
  worktrees only when their key can be made path-independent (no
  `CARGO_MANIFEST_DIR` reads, no build script, no declared impure proc macro).
  Otherwise the key includes the absolute path.
- **Macros that read your files are yours to declare.** A proc macro is treated
  as pure unless the project lists it (see above). A listed macro's dependents
  follow every file in their package; an unlisted macro that reads a file
  without registering it with rustc can produce a stale result, exactly as
  with Cargo.
- **Build scripts are not sandboxed.** A script that reads files or variables it
  did not declare with `rerun-if-*` can produce a stale result — exactly as with
  Cargo.
- **Unsupported Cargo features fail loudly.** For example `--profile`, globs
  with wildcards in intermediate directories in `workspace.members`. Mitts
  reports them instead of guessing.
- **Same-filesystem zero-copy.** Zero-copy unit promotion from staging requires
  `MITTS_CACHE_DIR` and the workspace `target/` directory to reside on the same
  filesystem mount. Crossing filesystem boundaries safely falls back to standard copy.

**Don't use Mitts yet** if you need Windows, cross-compilation, a team cache, or
a stable release. Cargo, optionally with sccache, is the safe choice there.

## Performance

Performance numbers are published only with a reproducible methodology: fixed
hardware, cold, warm and changed-input runs, several worktrees, the stronger of
plain Cargo and sccache as the baseline, and parity checks on diagnostics,
exit codes and test results. The study is in progress
([ROADMAP M8](ROADMAP.md)). Until it is published, Mitts makes no speedup or
disk-saving claims.

## Status

| Area | State |
| --- | --- |
| `Cargo.toml` / `Cargo.lock` reading, sparse-index resolution | done |
| Feature resolution (resolver 1 and 2) | done |
| Parallel fetcher and global source store | done |
| Native DAG runner, pipelined `rustc`, jobserver | done |
| Content-addressed store and action cache | done |
| Build script execution and caching | done |
| No-op fast path | done |
| `check`, `clippy`, `fmt`, `test` (incl. doctests), `coverage` | done |
| Conservative GC with quotas | done |
| C/C++ adapter on the same store | experimental |
| Path-independent keys for workspace crates | partial |
| Reproducible benchmark study | in progress |
| Remote / team cache | planned |
| Windows, cross-compilation | not supported |

Milestones and evidence: [ROADMAP.md](ROADMAP.md).

## FAQ

**Does Mitts replace Cargo?**
It replaces Cargo's build executor. Manifests, lockfiles and the registry stay
Cargo's. You can switch back to `cargo build` at any time.

**Can I use it in CI?**
You can, but the store is local to the machine today. CI benefits once the
remote cache lands. Until then, keep CI on Cargo.

**Will it fight with rust-analyzer or `cargo build`?**
No. Mitts never writes into `target/debug` or `target/release`; everything goes
to `target/mitts/`.

**Where is the cache and how do I clear it?**
`--cache-dir`, else `$MITTS_CACHE_DIR`, `$XDG_CACHE_HOME/mitts` or
`~/.cache/mitts`; everything Mitts caches lives under that one root. Use
`mitts gc` to prune within quotas; deleting the directory is also safe when no
build is running.

**Why was a unit not cached?**
Run `mitts explain`. It lists every unit and the reason it is or is not
eligible for the cache.

## Design rules

- Keep build and cache orchestration language-neutral. Language semantics live
  in adapters; Rust is the first one.
- A cache hit needs complete, tracked inputs. Unknown inputs are treated as
  changed.
- Shared artifacts are immutable and verified before use. Mutable state stays
  in each workspace. No hardlinks to writable files, no timestamp tricks.
- Cleanup never touches artifacts of an active build.

Contributor and agent rules: [AGENTS.md](AGENTS.md).
Architecture decisions: [docs/decisions](docs/decisions).

## Author

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes ·
[mittsbuild.com](https://mittsbuild.com)

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), at your option.
