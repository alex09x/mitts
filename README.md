<p align="center">
  <img src="site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>Build work, accounted for.</b><br>
  A Rust-first build system with a shared, content-addressed artifact cache and explicit input tracking.
</p>

<p align="center">
  <a href="https://mittsbuild.com"><img src="https://img.shields.io/badge/site-mittsbuild.com-ff3b30" alt="Website"></a>
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-c9ced4" alt="License: MIT OR Apache-2.0">
  <img src="https://img.shields.io/badge/language-Rust-1b1d20?logo=rust" alt="Written in Rust">
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS-1b1d20" alt="Platforms: Linux, macOS">
  <img src="https://img.shields.io/badge/status-pre--release-ff3b30" alt="Status: pre-release">
</p>

---

Mitts reads your existing `Cargo.toml` and `Cargo.lock`, plans the compilation
itself, and invokes `rustc` directly. Every compile action gets a key derived
from all of its inputs. The output is stored once in a machine-wide immutable
store and restored into each workspace that needs it, across projects,
worktrees and branches.

If an input cannot be accounted for, the action is rebuilt, not guessed.

## Why

- **Rebuild only what changed.** Switching branches, reverting a commit or
  opening another worktree reuses every unit whose inputs match.
- **Store each artifact once.** Dependencies compiled for one project are
  reused by the next one with the same inputs.
- **Stay compatible with Cargo.** No new manifest, no second lockfile. A project
  built with Mitts still builds with `cargo build` in CI.
- **Correctness first.** A cache hit requires a complete, tracked set of
  inputs. Unknown inputs are treated as changed.

Performance numbers are published only with a reproducible methodology. The
benchmark study is in progress; until it is published, Mitts makes no
speedup or disk-saving claims.

## How it works

```
Cargo.toml + Cargo.lock
        │
        ▼
 resolve ─── reuse Cargo.lock, or resolve against the crates.io sparse index
        │
        ▼
 fetch ───── download .crate archives in parallel into the global source store
        │
        ▼
 plan ────── compilation units: lib / bin / build script / proc macro / test
        │
        ▼
 key ─────── SHA-256 over toolchain, target, profile, flags, features,
        │    source contents, dependency keys and tracked environment
        ▼
 execute ─── parallel DAG of direct rustc calls
             hit  → restore from the store (reflink or copy)
             miss → compile, verify, publish atomically to the store
```

The full protocol — key derivation, locking, build scripts, the no-op fast
path, storage layout and garbage collection — is described in
[docs/architecture.md](docs/architecture.md).

## Usage

Mitts is not published yet. Build it from source:

```sh
cargo install --path .
```

Then run it inside any Cargo workspace:

```sh
mitts build              # compile into target/mitts/
mitts build --release
mitts run                # build and run a binary
mitts check | clippy | fmt | test | coverage
mitts explain            # show the plan, units and cache eligibility
mitts fetch              # prefetch dependencies into the source store
mitts gc                 # prune the shared store within quotas
mitts clean              # remove workspace artifacts
```

Mitts writes only to `target/mitts/` in the workspace and to its shared cache
(`$MITTS_CACHE_DIR`, `$XDG_CACHE_HOME/mitts` or `~/.cache/mitts`). Ordinary
Cargo and rust-analyzer keep working side by side.

## Status

| Area | State |
| --- | --- |
| Cargo.toml / Cargo.lock reading, sparse-index resolution | done |
| Parallel fetcher and global source store | done |
| Native DAG runner, pipelined `rustc`, jobserver | done |
| Content-addressed store and action cache | done |
| Build script execution and caching | done |
| No-op fast path (`FastBuildLock`) | done |
| `check`, `clippy`, `fmt`, `test`, `coverage` | done |
| Conservative GC with quotas | done |
| C/C++ adapter on the same store | experimental |
| Path-independent keys for workspace crates | in progress |
| Reproducible benchmark study | in progress |
| Remote / team cache | planned |
| Windows | not supported |

Details and the full milestone history: [ROADMAP.md](ROADMAP.md).

## Design rules

- Keep build and cache orchestration language-neutral. Language semantics live
  in adapters; Rust is the first one.
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
