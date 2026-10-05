# Decision 0001 — First Rust slice and build-plan ownership

Status: open. Decision owner: project owner. Contributors: implementing resident
and reviewer. This document records alternatives; none is selected yet.

## Goal

Choose the smallest useful path to correct reusable builds and lower physical
artifact duplication. Start with the [baseline scenario](../scenarios/0001-rust-baseline.md),
not an assumed speedup or an attempt to recreate every Cargo capability at once.

## Build, reuse, or contribute?

Compare native Cargo, existing caches such as Kache/sccache, and a Mitts-owned
solution against the same needs: correctness/trust boundaries, reusable inputs,
physical disk savings, parallel-worktree behavior, explanation of misses,
operational control, and maintenance cost. The first comparison is read-only
documentation research and must be labeled preliminary, not validated acceptance.
If a candidate's behavior needs empirical checking, propose an isolated trial with
its exact scope for separate approval; do not install or run it automatically.

Record the concrete unmet requirement before choosing a new implementation.
Reusing a tool or contributing a missing capability is a valid outcome; selecting
it must explicitly revise the roadmap rather than pretend a new engine shipped.

Order: choose the existing Cargo scenario, read/obtain approved baseline evidence,
compare alternatives, choose plan ownership, then finalize Mitts behavior and
acceptance. A documentation comparison can inform a provisional choice but cannot
establish tool correctness or measured benefit.

## Ownership alternatives

| Option | Mitts owns | Existing tooling owns | Consequence for M1 |
| --- | --- | --- | --- |
| Cargo-driven first slice | CLI, eligible artifact reuse, storage policy, explanations | Dependency resolution and the initial build plan | Smaller slice; package management is still Cargo-owned and must be described honestly |
| Mitts-owned Rust build plan | Supported manifest/resolver/planning semantics, compiler actions, store, CLI | rustc and platform tools | Larger implementation; break compatibility into explicit supported/bypassed/refused categories |
| Reuse/contribute first | Integration or missing operational capability | The chosen existing engine/cache | First prove the unmet need and ownership boundary; do not claim a Mitts replacement |

All options keep canonical Rust project inputs, respect `Cargo.lock` intent, and
avoid a second dependency source of truth. Unsupported behavior must fail clearly
or use a documented fallback without concealing which engine actually ran.

## Required decisions

- [ ] Select the scenario, supported platform/toolchain, and first representative fixture.
- [ ] Select the approach and state why existing tooling alone does not meet the goal.
- [ ] Name owners of manifest reading, dependency resolution, plan generation,
  compiler execution, lockfile updates, cache keys, and cleanup.
- [ ] Define eligible cached actions and unknown-input bypass/refusal behavior.
- [ ] Define operational configuration and any allowed interactions with Cargo settings.
- [ ] Choose the implementation stack and license before code or license grants.
- [ ] Approve the M0 correctness oracle and workload-specific success criteria.

## Correctness oracle

Compare effective dependency/toolchain/configuration semantics and observable
program results with ordinary Cargo for the supported scenario. Preserve failure
and diagnostic meaning. Record legitimate differences in output paths, path
mapping, signing, or nondeterminism; do not blindly require identical raw argv.

For the cache itself, verify that restored output bytes match the recorded hashes.
Test each covered invalidating input class and reject incomplete/corrupt entries.
An unsupported or undeclared input must never become a speculative successful hit.

## Outcome

Selected option: pending. Rationale, approved scope, and evidence: pending.
No implementation or measurement is authorized by this open decision document.
