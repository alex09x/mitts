# Mitts Build roadmap

Rust comes first. Other languages follow through adapters on the same
language-neutral core. The protocol itself is described in
[docs/architecture.md](docs/architecture.md).

Legend: ✅ done · 🧪 experimental · 🚧 in progress · 📋 planned

## At a glance

| Milestone | Scope | State |
| --- | --- | --- |
| M0 | Contract, scenario, first-slice decision, license | ✅ |
| M1 | Correct uncached Rust builds, Cargo coexistence | ✅ |
| M2 | Content-addressed store and action cache | ✅ |
| M3 | Parallel fetcher, `Cargo.lock` reuse and resolution | ✅ |
| M4 | Native parallel DAG runner with direct `rustc` | ✅ |
| M5 | Pilot, deduplication, conservative GC | ✅ |
| M6 | Second adapter: C/C++ on the shared store | 🧪 |
| M7 | Hardening for real projects | 🚧 |
| M8 | Reproducible benchmark study | 🚧 |
| M9 | Remote and team cache | 📋 |

## Rules for this roadmap

- An unchecked item is planned, not implemented. A milestone closes only with
  its evidence and the exact revision.
- A cache miss may succeed through a real uncached build. A hit is never
  fabricated, and an unsupported build is never reported as success.
- No speedup or disk-saving claim without a published, reproducible
  measurement (see M8).
- Work follows the development validation policy in [AGENTS.md](AGENTS.md).

## Done

### M0 — First contract ✅

- [x] Rust baseline scenario on `fixtures/hello-rust`
  ([scenario](docs/scenarios/0001-rust-baseline.md)).
- [x] Alternatives compared and first slice chosen: Cargo owns resolution,
  Mitts owns keys, the store and restoration
  ([decision 0001](docs/decisions/0001-first-rust-slice.md)).
- [x] Correctness oracle: equivalent inputs, diagnostics, exit status and
  program behavior; restored bytes must match their hashes.
- [x] License selected: `MIT OR Apache-2.0`.

### M1 — Correct builds without caching ✅

- [x] Smallest supported Rust build that preserves lockfile intent, package,
  profile, features, diagnostics and exit status.
- [x] Build actions modeled apart from the Rust adapter.
- [x] Separate output location; Cargo and rust-analyzer are never disturbed.
- [x] Unsupported cases reported as unsupported.

### M2 — Content-addressed store and action cache ✅

- [x] Immutable store, objects published atomically and marked read-only.
- [x] Deterministic action keys over toolchain, target, profile, flags,
  features, sources and dependency keys.
- [x] Integrity check on every restore; corrupt objects trigger a rebuild.
- [x] Single-flight locking per action key; `--no-cache` bypass.

### M3 — Fetcher and lockfile ✅

- [x] Reuse a compatible `Cargo.lock` without re-resolution.
- [x] Resolve against the crates.io sparse index when the lockfile is missing
  or stale, and write a standard `Cargo.lock`
  ([decision 0002](docs/decisions/0002-mitts-native-engine.md)).
- [x] Parallel download into a global source store with checksum verification.

### M4 — Native DAG runner ✅

- [x] Topological unit graph executed in parallel with direct `rustc` calls.
- [x] Pipelined compilation from `.rmeta`.
- [x] Dependencies consumed straight from the store via `--extern` / `-L`.
- [x] Build scripts and proc macros compiled and run by Mitts.
- [x] Reflink restoration where the filesystem supports it, full copy elsewhere.

### M5 — Pilot and GC ✅

- [x] Pilot builds of real projects side by side with Cargo without corruption.
- [x] Dependencies shared across worktrees through one store.
- [x] `mitts gc` with age and size quotas that never touches active builds.

### M6 — C/C++ adapter 🧪

- [x] `CAdapter` with `Mitts.toml` and `cc` invocation.
- [x] Rust and C artifacts coexist in one store and one GC.
- [ ] Validation on real C/C++ projects before it leaves experimental status.

## In progress

### M7 — Hardening for real projects 🚧

Done:

- [x] Build script caching keyed by content hashes of `rerun-if-changed` paths
  and values of `rerun-if-env-changed` variables.
- [x] Variables read through `env!` / `option_env!` discovered and keyed per unit.
- [x] Dep-info inputs (`include_str!`, nested modules) included in workspace keys.
- [x] Cooperative scheduler: contested units are skipped and revisited instead
  of blocking a worker.
- [x] Machine-wide jobserver shared by all Mitts processes; inherited jobserver
  respected.
- [x] No-op fast path (`FastBuildLock`) with file and directory snapshots.
- [x] `check`, `clippy`, `fmt`, `test`, `coverage` / `llvm-cov` commands.
- [x] Outputs under `target/mitts/` with `CACHEDIR.TAG`.
- [x] Path-independent keys for workspace crates via `--remap-path-prefix` and
  selective `CARGO_MANIFEST_DIR` detection, enabling cross-worktree and cross-clone cache reuse.

Open:

- [ ] Cross-compilation beyond the host target.
- [ ] Clear diagnostics when a build script reads undeclared inputs.
- [ ] Windows support (locks and jobserver are POSIX-only today).

### M8 — Reproducible benchmark study 🚧

- [ ] Baselines: Cargo, Cargo with a shared target directory, Cargo with
  sccache. Ratios are reported against the best baseline.
- [ ] Scenarios: cold build, no-op, replay of a commit history, branch switch
  and revert, many worktrees at once.
- [ ] Each scenario documents its starting cache state; cold runs assert empty
  caches. Repeated runs with median and spread, pinned toolchain, offline
  sources, physical disk usage measured with `df`.
- [ ] Results published with raw data and scripts before any number appears in
  the README or on the site.

## Planned

### M9 — Remote and team cache 📋

- [ ] Second store tier over S3-compatible storage or HTTP.
- [ ] Writes only from trusted CI; developers read.
- [ ] Immutable objects may sit behind a CDN; the action cache stays authoritative.
- [ ] Requires path-independent keys from M7 first.

### Later

- Remote execution (REAPI).
- IDE and rust-analyzer integration.
- More language adapters once the Rust path is stable.
