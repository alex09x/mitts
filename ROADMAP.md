# Mitts Build roadmap

Status: planning only. This document defines work; it does not start implementation,
queue agents, approve benchmarks, or claim that any planned capability exists.
The first supported language is Rust. Other languages follow through adapters.

## Already done

- [x] Create the private repository and register the Mitts Build workspace.
- [x] Record the product idea, boundaries, and development rules in README/AGENTS.

## Execution rules

Work in milestone order. Start one bounded vertical slice at a time; record its
allowed paths, observable acceptance, and directly affected tests before editing.
Follow the global validation policy: no per-edit build/format/lint loop, no
automatic broad suites, and no replay of unchanged successful checks.

An unchecked item is planned, not implemented. A milestone closes only with its
actual evidence and exact revision. A failed gate means fix, narrow the declared
scope, or stop. A cache miss/bypass may succeed through a real uncached build;
never fabricate a hit or conceal a failed/unsupported build as success.

## M0 — Decide the first contract

Follow this order; later choices use evidence from earlier steps.

1. [x] Fill the [Rust baseline scenario](docs/scenarios/0001-rust-baseline.md): choose
   one existing Cargo workflow, platform/toolchain, and representative small fixture (`fixtures/hello-rust`).
2. [x] Read existing baseline evidence or obtain approval for one bounded ordinary
   Cargo measurement. Record compilation units and storage categories: dependency
   artifacts, own artifacts, incremental state, binaries, and debug data.
3. [x] Complete the alternatives section of the open
   [first-slice decision](docs/decisions/0001-first-rust-slice.md): compare Mitts with
   reusing/contributing to Kache, Cargo, or other existing tools. Identify the concrete unmet user need.
4. [x] Choose the initial resolver/build-plan owner and implementation stack. Name
   owners of resolution, compilation actions, lockfile updates, and cache keys.
   Selected Option 1 (Cargo-driven first slice): Cargo owns dependency resolution and base plan,
   Mitts owns cache keys, artifact store, CoW restoration, and explanations.
5. [x] Now define planned `mitts build` / `mitts explain` behavior, unsupported cases,
   and workload-specific success criteria. Define the correctness oracle: equivalent
   effective inputs/settings, diagnostics/exit behavior, and program behavior;
   record permitted path/signing differences. Restored cache bytes must match their hashes.
6. [x] Record the owner-selected licensing decision and approval of the narrow first
   implementation scope: Rust stack, `MIT OR Apache-2.0` dual license.

Gate: cleared. The owner approved the concrete scenario (`fixtures/hello-rust`),
Cargo-driven first slice ownership, `MIT OR Apache-2.0` license, and M1 acceptance scope.

## M1 — Build correctly without caching

- [x] Implement the smallest supported Rust build from the M0 contract; preserve
  lockfile intent, selected package/profile/features, diagnostics, and exit status.
- [x] Execute build scripts and proc macros without caching their runs when the
  chosen workflow requires them. A deliberately smaller first fixture must not
  be advertised as supporting the whole chosen project.
- [x] Model build actions separately from the Rust adapter: inputs, outputs,
  toolchain identity, and dependencies. Keep the store free of Cargo-specific
  semantics, but defer a general plugin API until another language actually needs it.
- [x] Make unsupported cases visible. Do not silently alter configuration or report
  an unsupported build as successful.
- [x] Define separate mutable output/build locations for Mitts. Verify ordinary
  Cargo and rust-analyzer can coexist without overwriting their configuration,
  deleting their targets, or sharing mutable build state with Mitts.
- [x] Add focused success and failure regressions for the agreed fixture, including
  changed source, a compile error, and an unsupported-case boundary.
- [x] Compare with the M0 oracle and expose the effective plan. Output directories
  or documented path mappings may differ; unexplained semantic differences may not.

Branch explicitly: with a Cargo-owned plan, M1 is a clean orchestration slice.

Gate: cleared. The supported program has the exact same observable behavior as its ordinary Cargo
build (`Hello from Mitts fixture: 42`, exit 0); failures preserve diagnostics and exit codes.
No speedup claim.

## M2 — Content-Addressed Store (CAS) & Action Cache (AC)

- [x] Take the verified M1 Cargo-driven build plan as input, decoupling storage fundamentals from the independent resolver.
- [x] Introduce an immutable, content-addressed local store (CAS) at `~/.cache/mitts/cas/` and versioned Action Cache (AC).
- [x] Implement deterministic Action Key derivation:
  `Key = SHA256(rustc_version + target_triple + flags + features + source_hashes + dep_artifact_hashes)`.
- [x] Atomically publish artifacts into CAS (`chmod 444`, `rename` staging); guarantee corrupted/interrupted entries never appear.
- [x] Single-flight execution: file-based locking (`flock`) on active Action Keys to eliminate duplicate compilation across concurrent workspaces/terminals.
- [x] Initial eligible subset: pure Rust rlib and binary compile actions with verified input tracking.
- [x] Test CAS hash integrity, action cache hit/miss semantics, concurrent locks, and cache-off mode.

Gate: cleared. Compatible action inputs reuse cached artifacts without re-execution (`cached: true`, exit 0); corrupted CAS objects are detected via SHA-256 integrity and trigger clean recompilation; concurrent builds coordinate safely on the same action key via single-flight `flock`; `--no-cache` mode bypasses cache cleanly. 20 workspace tests passing.

## M3 — Fast parallel fetcher & Cargo.lock resolution

- [ ] Implement two-phase dependency management based on standard `Cargo.lock` (Decision 0002):
  - Fast-path: read existing `Cargo.lock` directly without re-resolution, validating compatibility with `Cargo.toml` and workspace settings.
  - Sync-path: when `Cargo.lock` is missing or incompatible with manifest/workspace changes, resolve dependencies using PubGrub against crates.io sparse index (evaluating feature, target, and dependency constraints; treating compilation profiles as builder inputs) and generate standard, valid `Cargo.lock`.
- [ ] Concurrent multithreaded downloader: fetch `.crate` archives in parallel directly into global sources store (`~/.cache/mitts/sources/`), verifying SHA-256 checksums from `Cargo.lock`.
- [ ] Zero local source pollution: dependencies are fetched and unpacked once globally per machine, not duplicated per workspace.
- [ ] Test lockfile generation equivalence with Cargo, missing lockfile handling, offline cache hits, and concurrent downloads.

Gate: `Cargo.lock` demonstrates format and resolution compatibility with ordinary Cargo; packages are fetched concurrently into the global store without invoking `cargo`; checksums match `Cargo.lock`.

## M4 — Mitts-native parallel DAG runner (Zero target pollution)

- [ ] Implement the autonomous compilation DAG runner:
  - Construct topological dependency graph from `Cargo.lock`.
  - Compile leaf crates in parallel across all CPU cores directly invoking `rustc`.
  - Pipelined compilation: feed `.rmeta` outputs directly from Global Store to dependent crates via `--extern crate_name=/path/to/cas/...`.
- [ ] Zero intermediate target pollution: dependency `.rmeta` and `.rlib` live solely in Global Store; local workspace receives only the final compiled binary (`./my-app`).
- [ ] CoW reflink fallback for tools requiring local file staging (e.g. legacy IDE workflows or specific linkers).
- [ ] Handle `build.rs` and procedural macros with explicit sandboxed execution and environment tracking.
- [ ] Test multi-crate DAG parallel compilation, compiler diagnostic preservation, and clean workspace behavior.

Gate: full build succeeds via direct `rustc` invocations without running `cargo build`; local workspace contains only the requested output binary; speedup and disk-saving claims are deferred to M5 empirical measurements.

## M5 — Real-world pilot, benchmarks, and coexistence validation

- [ ] Benchmark cold build, warm build (100% hit), and single-file incremental rebuild against standard Cargo on real-world projects.
- [ ] Measure physical disk allocation across multiple worktrees sharing dependencies (demonstrating true 1:1 deduplication).
- [ ] Validate coexistence: running ordinary `cargo build` and `mitts build` side-by-side does not corrupt either toolchain or invalidate caches.
- [ ] Conservative GC and storage quotas: LRU pruning of old CAS objects without touching active builds.

Gate: physical disk savings and build speedup are verified with reproducible empirical evidence; coexistence with Cargo and IDEs is rock-solid.

## M6 — Add one other language adapter

- [ ] Select next language adapter (e.g. C/C++ or Go) building upon the language-neutral CAS and DAG engine.
- [ ] Implement native manifest parsing and toolchain invocation for the target language.
- [ ] Verify multi-language artifact reuse in the shared CAS core.

Gate: one non-Rust project builds correctly through the shared core with verified caching and store reuse.

## Later, not part of the first implementation

- Remote/team distributed cache (HTTP/gRPC S3-compatible backend).
- Remote execution engine (REAPI).
- IDE daemon integration and language server direct indexing.

## First task to pick up

With M2 completed and validated, proceed to **M3 — Fast parallel fetcher & Cargo.lock resolution**:
Implement the two-phase dependency resolver (fast-path lockfile validation vs sync-path PubGrub resolution against crates.io sparse index) and parallel multi-threaded crate fetcher.
