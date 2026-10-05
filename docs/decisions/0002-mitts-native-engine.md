# Decision 0002 — Mitts-native execution engine and Cargo.lock compatibility

Status: proposed and aligned with project owner. Decision owner: Alexander Panasenko.
Contributors: implementing resident and reviewer.

## Context and Problem Statement

In Milestone M0/M1 (Decision 0001), Option 1 was selected as the first slice: relying on Cargo to drive resolution and build planning while Mitts isolates build artifacts into `.mitts/target/` and prepares an immutable cache.

However, operational experience and architectural analysis reveal fundamental limitations with remaining an external wrapper around `cargo build`:
1. **Target directory pollution**: Cargo inherently requires staging all intermediate compilation units (`.rlib`, `.rmeta`, `.d`, `.fingerprint`) inside a workspace target directory (`target/debug/deps/`). Even with isolated `.mitts/target/`, gigabytes of intermediate files are written per workspace.
2. **Lock contention and target thrashing**: Cargo's internal lock semantics prevent concurrent builds from safely sharing build trees, forcing either duplicate local targets or risky lock fights.
3. **CoW Reflink vs Direct Store Reads**: While CoW reflinks protect immutable store files from mutable workspace overwrites, creating thousands of reflink entries in local directories remains an unnecessary overhead when modern compilers (`rustc`) can consume dependencies directly via `--extern crate_name=/path/to/store/...`.
4. **Lockfile Generation vs Reuse**: A build tool cannot assume `Cargo.lock` always pre-exists (e.g. freshly created projects or modified `Cargo.toml`).

## Architecture and Strategic Decisions

### 1. Two-phase lifecycle: Resolver vs DAG Builder

Mitts explicitly separates the build lifecycle into two decoupled phases:

```
[ Cargo.toml & Workspace Configuration ]
      │
      ▼
PHASE 1: RESOLVER
   - If Cargo.lock exists and is compatible with Cargo.toml and workspace settings -> read directly without re-resolution.
   - If Cargo.lock is missing or incompatible with manifest changes -> resolve dependencies via PubGrub against crates.io sparse index, evaluating features, profiles, and target conditions alongside lockfile state, and write a standard, valid Cargo.lock.
      │
      ▼
[ Cargo.lock ]  <-- Universal ecosystem compatibility contract
      │
      ▼
PHASE 2: BUILDER (Mitts-native parallel execution)
   - Fetch: Concurrent multithreaded download of missing .crate archives directly into global sources store (~/.cache/mitts/sources/).
   - DAG Builder: Topological dependency graph execution directly invoking rustc in parallel across all CPU cores.
   - Direct Store Reads: Feed dependency .rmeta and .rlib directly from the immutable Global Store (~/.cache/mitts/cas/) via rustc --extern flags.
   - Clean Workspace: The local project directory receives the final compiled binary (e.g. ./my-app), targeting zero intermediate target bloat.
```

### 2. Universal Ecosystem Compatibility via `Cargo.lock`

Mitts rejects inventing a proprietary `Mitts.lock`.
- **Single Source of Truth**: `Cargo.lock` remains the universal lockfile.
- **Interoperability**: Any project built with Mitts can still be built with standard `cargo build` in CI, and audited by tools like `cargo-audit` and `cargo-deny`.
- **Zero Friction**: Developers do not need to convert or reconfigure projects.

### 3. Global Immutable Store & Concurrency Safety

- **Sources**: `~/.cache/mitts/sources/{crate}-{version}/` (downloaded and unpacked once per system).
- **CAS (Content-Addressed Store)**: `~/.cache/mitts/cas/objects/{sha256}` (compiled `.rmeta` and `.rlib` artifacts, marked read-only).
- **Action Cache**: Maps `ActionKey = SHA256(rustc_version, flags, inputs_hash, deps_hashes)` to output artifact hashes.
- **Single-Flight Execution**: Concurrent builds of the same crate across different terminal sessions or workspaces coordinate via file locks (`flock`) on the action key; subsequent builders wait and immediately reuse the completed artifact.

### 4. Coexistence with `build.rs` and Native C Dependencies

For the majority of Rust crates, compilation is pure Rust (`rustc`). For crates with native build scripts (`build.rs`):
- `build.rs` is compiled and executed in a sandboxed staging directory.
- Captured build script instructions (`cargo:rustc-link-lib`, `cargo:rustc-cfg`, and `$OUT_DIR` outputs) are cached and fed to dependent `rustc` invocations.

### 5. Staged Delivery and Differential Oracle

- **Decoupled M2 Implementation**: Milestone M2 implements the Content-Addressed Store (CAS) and Action Cache (AC) using the verified M1 Cargo-driven build plan as input. This decouples storage fundamentals from the resolver.
- **Differential Oracle**: The M1 Cargo path serves as an ongoing differential correctness oracle. Native executions are validated against Cargo for diagnostic fidelity, exit status, and binary equivalence.
- **Pure Rust First Slice**: The initial native execution slice is strictly confined to pure Rust `rlib` and `bin` actions. Advanced features (build scripts, procedural macros, and cross-compilation) will be introduced incrementally with explicit input modeling.

## Decision Outcome

Mitts evolves from a passive `cargo build` wrapper into an autonomous Rust build engine and package runner, preserving Cargo.toml and Cargo.lock ecosystem compatibility with explicit staged goals:
- Eliminate intermediate dependency artifacts from workspace targets.
- Enable parallel fetching and DAG compilation.
- Direct-from-store compilation via `rustc --extern`.
- Shared, immutable, deduplicated artifact storage across all projects, with all speedup and disk-saving claims deferred to empirical measurement under M5.


