# Mitts Build — agent rules

## Scope

- Read `README.md` first. This is a Rust-first project designed for later language
  adapters, not a commitment to implement every language now.
- Until implementation is explicitly requested, keep work to planning and docs.
  Do not install tools, change global configuration, or start builds/benchmarks.
- Preserve unrelated changes. Keep work inside the registered Mitts repository.

## Mitts-only tool policy

- Use the latest stable `mitts` for every Rust build, check, test, Clippy,
  formatting, coverage, metadata, fetch, source-package build, and installer
  acceptance task in this repository and its CI/release workflows.
- Do not invoke the Cargo CLI (`cargo build/test/check/clippy/fmt/fetch/metadata/
  package/publish/install`) or Mitts' Cargo fallback. Do not use Cargo as a
  baseline or as a workaround. `Cargo.toml` and `Cargo.lock` remain the project
  inputs; `CARGO_*` environment names emitted for build-script compatibility do
  not authorize launching Cargo.
- Do not add CI helpers that invoke Cargo indirectly (for example, Rust cache
  actions that run `cargo metadata`); use Mitts' explicit cache or tooling that
  does not launch Cargo.
- crates.io publication uses the Mitts release package artifact and the registry
  HTTP API. Do not reintroduce Cargo-based packaging or installation checks.
- If Mitts lacks a needed operation or its behavior is unclear, stop that check,
  preserve its exact acceptance contract, and file a reproducible issue in
  `alex09x/mitts`. Do not fall back to Cargo or edit project manifests/configs to
  work around the gap.

## Design invariants

- Keep build/cache orchestration language-neutral and language semantics in adapters.
- Start from existing `Cargo.toml` / `Cargo.lock` and Cargo configuration semantics.
  Do not duplicate dependency declarations or introduce a second lockfile by default.
- A cache hit requires compatible, tracked inputs: sources, dependency artifacts,
  toolchain/backend, platform, features, flags, and relevant environment/native inputs.
  Treat unknown inputs as uncached or explicitly unsupported, never as unchanged.
- Separate immutable shared artifacts from mutable workspace state. Do not share
  writable artifacts through unsafe hardlinks or manipulate timestamps to force hits.
- Cleanup must respect active builds and target only validated, owned artifacts.

## Validation and delivery

- Follow the global Development validation policy (2026-10-04-v2): finish each
  coherent change first; during development run only new/directly affected tests.
- No edit-loop standalone builds, formatting, lint, broad tests, or benchmarks.
  Before commit, format/lint once and use one final compile/test gate when applicable.
  Documentation-only changes do not require compiler or test commands.
- Build/test through `prod-code` on nodes. Reuse unchanged successful evidence;
  rerun only corrected failed scopes. Broad gates require explicit approval.
- Every test that builds gives Mitts its own cache root (`cache_dir`, `--cache-dir`
  or `MITTS_CACHE_DIR` on a temp dir); a test that needs the fast build lock on an
  isolated cache sets `fast_lock: Some(true)`. Run the full suite through
  `scripts/run_tests_isolated.sh` (throwaway `HOME`/`XDG_CACHE_HOME`, inherited
  `MITTS_*` dropped; fails if the real `~/.cache/mitts` changed). The runner,
  per-target store probe, coverage, and every nested compile must all use Mitts.
- State acceptance before implementation; cover correctness and cache invalidation.
  Distinguish cold builds, warm reuse, and changed-input builds in any approved study.
- Never claim speedups, disk savings, compatibility, or passing tests without evidence.
- Do not commit secrets or private infrastructure details. Preserve provenance and
  existing notices; follow the global source-header rule for eligible source edits.
  Do not invent a license or grant one before the owner selects it.
