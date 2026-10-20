# Mitts Build — agent rules

## Scope

- Read `README.md` first. This is a Rust-first project designed for later language
  adapters, not a commitment to implement every language now.
- Until implementation is explicitly requested, keep work to planning and docs.
  Do not install tools, change global configuration, or start builds/benchmarks.
- Preserve unrelated changes. Keep work inside the registered Mitts repository.

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
  `MITTS_*` dropped; fails if the real `~/.cache/mitts` changed) and prove the
  isolation with `scripts/probe_store_isolation.sh`, which runs every test target
  alone and fails on any that still reaches the default store.
  `CARGO_TEST_CMD=llvm-cov scripts/run_tests_isolated.sh --summary-only` gives coverage.
- State acceptance before implementation; cover correctness and cache invalidation.
  Distinguish cold builds, warm reuse, and changed-input builds in any approved study.
- Never claim speedups, disk savings, compatibility, or passing tests without evidence.
- Do not commit secrets or private infrastructure details. Preserve provenance and
  existing notices; follow the global source-header rule for eligible source edits.
  Do not invent a license or grant one before the owner selects it.
