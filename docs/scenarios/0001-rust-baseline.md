# Scenario 0001 — Rust baseline and first-slice acceptance

Status: approved by project owner. Measurement approval: baseline fixture and M0 scope.

## Specification

| Field | Value |
| --- | --- |
| Project and representative small fixture | `fixtures/hello-rust` (binary crate with single pure dependency `itoa = "1.0"`) |
| Exact source revision | Head of main at initial fixture introduction |
| Exact Cargo command; package/target/features/profile | `cargo build --package hello-rust --profile dev` and `--profile release` |
| Generalized OS, architecture, filesystem, and node class | macOS Darwin `aarch64` (APFS) / Linux `x86_64` (Btrfs/XFS for CoW reflink, ext4 for copy fallback); local workstation / dev node |
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

Empirically measured baseline for `fixtures/hello-rust` under standard Cargo:
- **Command**: `cargo build --message-format=json` (dev profile)
- **Source revision**: `b7aff36`
- **Toolchain**: `rustc 1.97.1` (8bab26f4f 2026-07-14) / `cargo 1.97.1` (c980f4866 2026-06-30)
- **Measured host platform**: Linux `x86_64` (kernel 6.6); macOS Darwin `aarch64` remains supported target environment
- **Exit status**: 0
- **Elapsed time**: 0.09s compilation (0.1s wall time)
- **Compilation units**: 2 (`itoa` v1.0.18 rlib, `hello-rust` v0.1.0 bin)
- **Artifact storage breakdown**:
  - `target/debug/deps/libitoa-*.rlib`: 201 KB
  - `target/debug/deps/libitoa-*.rmeta`: 60 KB
  - `target/debug/hello-rust` executable: 4.4 MB
  - Total `target/debug` directory allocation: 5.7 MB uncached
- **Program execution oracle**:
  - Command: `./target/debug/hello-rust`
  - Output: `Hello from Mitts fixture: 42`
  - Exit status: 0
- **Filesystem deduplication oracle & measurement method**:
  - macOS APFS: clonefile copy-on-write extent sharing; measured via allocated blocks (`stat -f "%b * %B"`).
  - Linux Btrfs / XFS: `ioctl(FICLONE)` reflink extent sharing; measured via allocated blocks (`stat -c "%b * %B"`) and extent mapping.
  - Linux ext4 & non-reflink filesystems: verified standard file copy fallback (validates build correctness without claiming CoW deduplication).
  - Verification: identical SHA-256 content hashes between store and restored target; modifying workspace target file must not alter the immutable cache entry.

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

| Run ID | Revision | Command | Platform | Toolchain | Exit | Elapsed | Storage | Oracle Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `base-0001` | `b7aff36` | `cargo build` | Linux x86_64 | rustc 1.97.1 | 0 | 0.09s | 5.7 MB (4.4 MB bin, 261 KB itoa) | PASS (`Hello from Mitts fixture: 42`) |

Baseline evidence recorded and verified under M0.
