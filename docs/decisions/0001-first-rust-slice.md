# Decision 0001 — First Rust slice and build-plan ownership

Status: decided and approved by project owner. Decision owner: Alexander Panasenko.
Contributors: implementing resident and reviewer.

## Goal

Choose the smallest useful path to correct reusable builds and lower physical
artifact duplication. Start with the [baseline scenario](../scenarios/0001-rust-baseline.md),
not an assumed speedup or an attempt to recreate every Cargo capability at once.

## Build, reuse, or contribute?

Comparison of existing tooling and options against project requirements:
- Native Cargo: No cross-workspace artifact cache. Each checkout or worktree rebuilds
  dependencies into its private `target/`, duplicating gigabytes across checkouts.
  Cargo's experimental shared target directory is not safe across concurrent builds
  and leads to target directory thrashing and lock contention.
- sccache: Caches individual `rustc` compiler invocations via a wrapper, but does
  not orchestrate workspace target directory assembly, does not utilize CoW reflinks
  for entire crate artifact bundles (rlib + rmeta), and does not provide language-neutral
  store primitives.
- Mitts: Introduces an immutable content-addressed artifact store with CoW reflink
  restoration into isolated workspace target directories (`.mitts/target/`). Operates
  with explicit cache key derivation (compiler identity, target triple, flags, dependencies,
  sources), explains cache hits/misses via `mitts explain`, and cleanly isolates
  operational settings from package manifests.

Conclusion: Option 1 (Cargo-driven first slice) meets the concrete user need while
minimizing initial implementation risk.

## Ownership alternatives

| Option | Mitts owns | Existing tooling owns | Consequence for M1 |
| --- | --- | --- | --- |
| **Cargo-driven first slice (Selected)** | CLI, eligible artifact reuse, storage policy, explanations | Dependency resolution and the initial build plan | Smaller slice; package management is still Cargo-owned and described honestly |
| Mitts-owned Rust build plan | Supported manifest/resolver/planning semantics, compiler actions, store, CLI | rustc and platform tools | Larger implementation; break compatibility into explicit supported/bypassed/refused categories |
| Reuse/contribute first | Integration or missing operational capability | The chosen existing engine/cache | First prove the unmet need and ownership boundary; do not claim a Mitts replacement |

All options keep canonical Rust project inputs, respect `Cargo.lock` intent, and
avoid a second dependency source of truth. Unsupported behavior must fail clearly
or use a documented fallback without concealing which engine actually ran.

## Required decisions

- [x] **Scenario and fixture**: `fixtures/hello-rust` (pure binary + `itoa` dependency),
  macOS Darwin `aarch64` (APFS) / Linux `x86_64` (Btrfs/XFS for CoW reflink, ext4 for copy fallback), Rust stable (`>= 1.85.0`, host `1.97.1`).
- [x] **Approach selection**: Option 1 (Cargo-driven first slice). Reuses Cargo resolution/planning
  while Mitts owns artifact store, cache keying, and CoW reflink target restoration.
- [x] **Ownership allocation**:
  - Manifest reading / dependency resolution / lockfile updates: Cargo (`Cargo.toml` / `Cargo.lock`).
  - Plan generation: Cargo execution captured by Mitts adapter.
  - Compiler execution: Cargo / rustc with tracked parameters.
  - Cache keys, immutable store, CoW restoration: Mitts core engine.
  - Cleanup and storage quotas: Mitts store manager.
- [x] **Eligible cached actions and bypass boundary**:
  - Eligible: pure Rust rlib and binary compile actions with fully tracked inputs, flags, and dependencies.
  - Bypassed / uncached: build scripts (`build.rs`), proc macro expansions (initially), and untracked environment/host reads.
- [x] **Operational configuration**: `.mitts/config.toml` (store path, size quota, CoW reflink toggle),
  strictly isolated from Cargo package manifests.
- [x] **Implementation stack and licensing**:
  - Implementation stack: Rust.
  - License: `MIT OR Apache-2.0` (dual license, canonical for Alexander Panasenko Rust tooling).
- [x] **Correctness oracle and success criteria**:
  - Observably identical executable output and exit code.
  - Restored artifact SHA-256 byte hashes match stored artifact hashes.
  - Target directory isolation: `.mitts/target/` coexists cleanly with Cargo `target/` and rust-analyzer.

## Correctness oracle

Compare effective dependency/toolchain/configuration semantics and observable
program results with ordinary Cargo for the supported scenario. Preserve failure
and diagnostic meaning. Record legitimate differences in output paths, path
mapping, signing, or nondeterminism; do not blindly require identical raw argv.

For the cache itself, verify that restored output bytes match the recorded hashes.
Test each covered invalidating input class and reject incomplete/corrupt entries.
An unsupported or undeclared input must never become a speculative successful hit.

## Outcome

Selected option: **Cargo-driven first slice (Option 1)**.
Rationale: Proven Cargo dependency resolution ensures 100% manifest and lockfile
fidelity, while Mitts delivers the missing shared immutable artifact store,
fast CoW reflink restoration, cache transparency (`mitts explain`), and
multi-workspace isolation.
Approved scope: M0 gate cleared; proceeding to M1 uncached build implementation.
