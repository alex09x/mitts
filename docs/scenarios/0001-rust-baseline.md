# Scenario 0001 — Rust baseline and first-slice acceptance

Status: approved by project owner. Measurement approval: baseline fixture and M0 scope.

## Specification

| Field | Value |
| --- | --- |
| Project and representative small fixture | `fixtures/hello-rust` (binary crate with single pure dependency `itoa = "1.0"`) |
| Exact source revision | Head of main at initial fixture introduction |
| Exact Cargo command; package/target/features/profile | `cargo build --package hello-rust --profile dev` and `--profile release` |
| Generalized OS, architecture, filesystem, and node class | macOS Darwin `aarch64` (APFS) / Linux `x86_64` (ext4/btrfs); local workstation / dev node |
| Exact execution location / sensitive evidence | Clean relative workspace path: `fixtures/hello-rust`; no sensitive or private paths |
| Rust/Cargo toolchain and relevant native tools | Stable Rust (`>= 1.85.0`, current host `1.97.1`), standard Cargo toolchain |
| Existing timing/storage evidence, if available | Baseline established upon initial fixture build: 2 compilation units (`itoa`, `hello-rust`) |
| Approved measurement scope and cold/warm/changed-input definitions | Cold: clean `.mitts/target/` and clean cache store; Warm: full cache hit; Changed-input: edited `main.rs` |
| Workload-specific correctness and benefit criteria | Observably identical output and exit status; CoW reflink restoration to isolated target |
| Planned Mitts behavior and ownership boundary | Cargo owns dependency resolution and base execution plan; Mitts owns CLI, cache keys, store, CoW restore, `mitts explain` |
| Decision/approval owner | Project owner (Alexander Panasenko) |

Do not paste credentials, private infrastructure details, or sensitive source into
reports. Use approved private evidence pointers when needed.

## Baseline evidence

Initial baseline measurements for `fixtures/hello-rust` under standard Cargo:
- Compilation units: 2 (`itoa` v1.0.14 rlib, `hello-rust` bin).
- Target storage categories:
  - Dependency rlib: `libitoa-*.rlib` + metadata `libitoa-*.rmeta`.
  - Application binary: executable `hello-rust` + debug info (`.dSYM` on macOS / DWARF on Linux).
  - Incremental state: isolated inside build target directory.
- Filesystem deduplication oracle:
  - macOS APFS: clonefile / copy-on-write extent sharing.
  - Linux: `ioctl(FICLONE)` reflink extent sharing.
  - Fallback: standard file copy when cross-device or unsupported.

Do not delete an active or user-owned target to manufacture a cold run. Any
approved cold scenario must use an isolated, owned build location (`.mitts/target/`).

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

Baseline definition completed and approved under M0.
