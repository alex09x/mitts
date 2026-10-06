# Mitts Build

Rust-first build and package tooling with a shared artifact cache.

Project domain: [mittsbuild.com](https://mittsbuild.com).

## Idea

Build compatible inputs once and reuse the results across projects, worktrees,
developers, and CI. Reduce unnecessary compilation and duplicate artifact storage
without sacrificing build correctness.

Start with **Rust**, then extend the same build/cache core through adapters for
other languages. Future language support is a direction, not a shipped feature.

## Initial direction

- Read existing `Cargo.toml` and `Cargo.lock`; do not require a new package manifest.
- Respect Cargo features, profiles, workspace rules, and configuration semantics.
  Reading the same TOML is not, by itself, proof of compatibility.
- Keep package/dependency declarations separate from Mitts operational settings
  such as cache location, storage limits, and remote cache policy.
- Design a shared immutable artifact store with complete build-input identities.
  Keep mutable incremental state isolated between workspaces and concurrent builds.
- Prefer copy-on-write restoration where supported. Measure physical disk usage,
  not just apparent file sizes; copying artifacts alone is not deduplication.
- Keep the core language-neutral: adapters describe inputs, outputs, toolchains,
  and build actions. Rust-specific behavior belongs in the Rust adapter.

## Boundaries

Correctness comes before cache hits. Unknown or untracked inputs must not produce
a speculative cache hit. Build scripts, procedural macros, native libraries,
environment values, and compiler settings need explicit treatment.

Mitts is not a new Rust compiler. Do not promise Go-like cold-build times or any
speedup without measurements. Preserve normal build behavior for unsupported
cases or report an explicit unsupported case; never silently claim success.

## Status
 
Milestones M0 through M6 completed and verified across 65 passing workspace tests:
- M0: Contract & architecture approved
- M1: Uncached build correctness & Cargo capture
- M2: Content-Addressed Store (CAS) & Action Cache (AC) with single-flight locking
- M3: Fast parallel fetcher & Cargo.lock resolution
- M4: Mitts-native parallel DAG runner (zero target pollution, direct `rustc`)
- M5: Real-world pilot, benchmarks, 1:1 disk deduplication & conservative GC
- M6: Second language adapter (C/C++) with multi-language CAS reuse

Dual-licensed under MIT OR Apache-2.0. This repository is private.

Roadmap: [ROADMAP.md](ROADMAP.md). Agent rules: [AGENTS.md](AGENTS.md).

