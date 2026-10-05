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

## M2 — Share compatible compiler results

- [ ] Introduce an immutable, content-addressed local store and versioned key format.
  Include compiler/backend, platform, flags/features, sources, dependency artifacts,
  and relevant tracked environment/native inputs; hashes of manifests alone are insufficient.
- [ ] Start with an explicitly eligible subset of Rust library actions. Keep
  unknown-input actions uncached; initially exclude build-script execution,
  procedural macros, and native-link work unless their input boundary is proven.
- [ ] Restore eligible results into a separate build directory per live workspace;
  keep mutable incremental state isolated. Record cache hits, misses, bypasses,
  corruption, and errors with explanations.
- [ ] Specify absolute-path handling before normalizing keys: preserve runtime,
  diagnostic, and debugger meaning. Do not merge different real inputs under a
  convenient shared path spelling.
- [ ] Publish entries atomically; verify format/content hashes on restore and keep
  partially written entries invisible. Establish same-key producer/reader races
  and interrupted-write correctness before any shared store is enabled.
- [ ] Provide a cache-off switch. Check coexistence with ordinary Cargo and
  rust-analyzer without overwriting their configuration or poisoning their artifacts.
- [ ] Test reuse across two checkouts, invalidation for each covered input class,
  truncated/corrupt entries, interrupted writes, and unchanged error behavior.

Gate: the same supported inputs reuse work, changed covered inputs do not produce
wrong hits, and incomplete/corrupt entries never become successful builds. If the
covered workload cannot reuse enough work to meet M0's goal, stop or revise scope.

## M3 — Save real disk space and survive concurrency

- [ ] Restore immutable artifacts using copy-on-write where supported; clearly
  report copy fallback and cross-volume limits. Do not share writable hardlinks.
- [ ] Coordinate duplicate in-flight actions without serializing unrelated builds;
  preserve each caller's cancellation and result delivery.
- [ ] Add storage budgets and conservative cleanup of validated, owned cache entries.
  Protect active restores/writes and never walk arbitrary projects deleting targets.
- [ ] Test concurrent producers/readers, cancellation, cleanup during active work,
  and immutable-store preservation after a restored file changes. M2's basic
  publication/race correctness is a prerequisite, not deferred to this milestone.

Gate: the supported filesystem demonstrates reduced physical allocation for
compatible repeated artifacts, not just smaller-looking directories; concurrency
and cleanup do not corrupt artifacts or break active builds.

## M4 — Expand verified Rust compatibility

- [ ] Publish a supported/bypassed/refused matrix and expand one category at a time:
  workspaces, path/registry/Git dependencies, features, profiles, cfg, and patches.
- [ ] Handle `build.rs`, proc macros, native libraries, and compile-time reads as
  explicit input/trust boundaries. An undeclared external input stays uncached
  unless a safe declared or isolated model covers it.
- [ ] Validate debugger/diagnostic paths, platform-specific artifacts, toolchain
  changes, and normal Cargo interoperability without timestamp-based freshness tricks.
- [ ] If Mitts owns dependency resolution/planning, deliver that work explicitly
  with lockfile/feature/build-script compatibility evidence. Otherwise document
  the remaining Cargo-owned boundary; do not claim a complete replacement.

Gate: each advertised compatibility category has directly affected regressions;
unsupported categories are still honest. No second independent lockfile by default.

## M5 — Run a bounded pilot, then prepare delivery

- [ ] Select one real project and agree on its exact command, revision, toolchain,
  profile, cache state, and platform. Reuse existing timing evidence when applicable.
- [ ] Obtain approval for a bounded cold/warm/changed-input comparison before running
  it. Change one causal variable; report build time, actual compiler work, physical
  disk allocation, failures, and cache overhead. Do not extrapolate foreign benchmarks.
- [ ] Make setup opt-in and reversible, with no silent global config changes or
  automatic target deletion. Test build behavior after disabling Mitts.
- [ ] Document installation, rollback, known limits, cache privacy/trust boundaries,
  and release readiness; publish/install globally only when separately authorized.

Gate: correctness is preserved and measured benefits justify the tradeoffs for
the agreed workload. Otherwise report no improvement and revisit the design.

## M6 — Add one other language

- [ ] Let the owner select the next language after the Rust slice is demonstrated.
- [ ] Write its adapter contract and respect its native manifest/lockfile semantics.
  Keep the shared action/cache core independent of Cargo-specific types.
- [ ] Deliver one build plus its cache-reuse/invalidation and failure boundaries,
  without starting implementations for every language simultaneously.

Gate: one real non-Rust workflow works through the shared core with its own evidence.

## Later, not part of the first implementation

- Remote/team caches and CI integration with authentication, trust separation,
  untrusted-writer handling, and controlled artifact publication. Entry: correct
  local reuse, a real multi-user/CI scenario, and an owner-approved trust contract.
- A Mitts-owned resolver/package manager if the initial plan delegates to Cargo.
  Entry: accepted compatibility scope and evidence that this ownership adds value;
  retain native Rust manifests/lockfile behavior unless a new format is explicitly chosen.
- Broader package-management commands and additional language adapters after
  the same ownership and compatibility criteria are met.
- Website/public release, alternative codegen backends, and remote execution.

## First task to pick up

M1 completed and cleared. Next task is **M2 — Share compatible compiler results**:
Introduce an immutable, content-addressed local store and versioned key format,
caching eligible Rust rlib actions and restoring results into isolated workspace
target directories with verified invalidation on input changes.
