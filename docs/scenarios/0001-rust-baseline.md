# Scenario 0001 — Rust baseline and first-slice acceptance

Status: pending owner selection. Measurement approval: not granted by this document.

## Fill before execution

| Field | Value |
| --- | --- |
| Project and representative small fixture | Pending |
| Exact source revision | Pending |
| Exact Cargo command; package/target/features/profile | Pending |
| Generalized OS, architecture, filesystem, and node class | Pending |
| Exact execution location / sensitive evidence | Approved private pointer only; pending |
| Rust/Cargo toolchain and relevant native tools | Pending |
| Existing timing/storage evidence, if available | Pending |
| Approved measurement scope and cold/warm/changed-input definitions | Pending |
| Workload-specific correctness and benefit criteria | Pending |
| Planned Mitts behavior and ownership boundary | Pending; fill after the M0 ownership decision |
| Decision/approval owner | Project owner |

Do not paste credentials, private infrastructure details, or sensitive source into
reports. Use approved private evidence pointers when needed.

## Baseline evidence

Read existing logs first. If new evidence is needed, propose one bounded Cargo
measurement for approval; do not automatically run a broad benchmark matrix.
`--timings` can expose compilation units and the dependency critical path, but
does not by itself separate every internal compiler/linker cost.

Record total time and actual compiler work, plus storage categories: dependencies,
workspace outputs, incremental state, final/test binaries, and debug data.
Define how physical allocated/shared blocks are measured on the chosen filesystem;
apparent `target` size alone is not a disk-saving oracle.

Do not delete an active or user-owned target to manufacture a cold run. Any
approved cold scenario must use an isolated, owned build location.

## First-slice acceptance

### M1 — Uncached supported build

- [ ] Ordinary Cargo and uncached Mitts build the supported fixture with equivalent
  effective inputs and observable program behavior; documented differences are explained.
- [ ] Source/configuration errors keep meaningful diagnostics and a failing exit status.
- [ ] Unsupported cases are explicitly refused or use the declared fallback.
- [ ] Required build scripts/proc macros execute correctly before their caching is attempted.
- [ ] Mitts has isolated mutable build outputs and coexists with ordinary Cargo
  and rust-analyzer without altering their configuration or targets.

### M2 — Eligible cache reuse

- [ ] Eligible results are restored byte-for-byte; each covered input change invalidates reuse.
- [ ] Interrupted/corrupt publications cannot produce a successful cache hit.
- [ ] Ordinary Cargo, rust-analyzer, and cache-disabled Mitts can still use the workspace.

These are separate milestone gates; M1 does not require an implemented cache.

## Evidence record

For any later approved check, record exact command, source revision, platform,
toolchain, cache state, exit status, counts, and artifact/evidence locations.
Reuse matching successful evidence. A missing or skipped result remains pending.

No checks have been executed for this scenario.
