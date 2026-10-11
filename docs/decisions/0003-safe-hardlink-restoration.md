# Decision 0003 — Safe Hardlink Restoration and CAS Immutability on Non-CoW Filesystems

Status: approved. Decision owner: Alexander Panasenko.
Contributors: implementing resident and reviewer.

## Context and Problem Statement

Mitts maintains an immutable Content-Addressed Store (CAS) at `~/.cache/mitts/cas/objects/{sha256}`.
On filesystems supporting Copy-on-Write (CoW) reflinks—such as APFS on macOS (`clonefile`),
Btrfs on Linux (`FICLONE`), and XFS on Linux (`reflink=1`)—restoring compilation artifacts
(`.rlib`, `.rmeta`, test binaries) into workspace target directories is zero-copy and protects
the store from accidental mutation: any write to a reflinked file triggers a copy-on-write
split at the block layer.

However, many Linux development and CI environments use `ext4`, where `ioctl(FICLONE)` fails
with `EOPNOTSUPP`. The default behavior on non-CoW filesystems is full copy fallback (`fs::copy`).
On large workspaces with extensive test suites, copying gigabytes of binaries into each
target directory incurs substantial I/O latency and physical disk duplication.

### Existing Project Invariants

The existing documentation strictly restricts hardlinks across two sections:
- **README**: *"Never hardlinked into a writable workspace."*
- **README**: *"No hardlinks to writable files, no timestamp tricks."*
- **AGENTS**: *"Do not share writable artifacts through unsafe hardlinks or manipulate timestamps to force hits."*

The core risk motivating these rules is **in-place mutation poisoning**: if an artifact in a
writable workspace target directory shares an inode with the global CAS via a raw, writable
hardlink, any compiler, linker, debugger, or script writing directly into that target file
modifies the shared inode, silently corrupting the CAS for all checkouts and workspaces on the host.

## Proposed Resolution and Policy Update

We propose an explicit, verified safe-hardlink restoration mode for same-filesystem non-CoW
environments, enabled via the environment flag `MITTS_CAS_HARDLINK=1`.

### Proposed Documentation Replacement

To resolve the contradiction between the original absolute prohibition and this controlled
opt-in mechanism, the invariants will be updated upon approval:

- **README replacement (first occurrence)**:
  > *"Artifacts are restored via CoW reflink where supported. On non-CoW filesystems (such as ext4), artifacts may be restored via safe read-only hardlinks (`MITTS_CAS_HARDLINK=1`) where permissions are restricted to `0444`/`0555` and workspace overwrites occur via atomic rename, or via isolated copies (default fallback). Direct in-place mutation of shared artifacts is strictly forbidden."*
- **README replacement (second occurrence)**:
  > *"No writable hardlinks into workspaces, no timestamp tricks; hardlinks are permitted only when strictly read-only (`0444`/`0555`) with atomic replacement."*
- **AGENTS replacement**:
  > *"Do not share writable artifacts through hardlinks and do not manipulate timestamps to force hits. Hardlinked artifacts must be read-only (0444/0555), promoted only by atomic rename, and are opt-in; otherwise use reflinks or isolated copies."*

## Architecture and Safety Mechanisms

### 1. Read-Only Permission Enforcement (`0o444` / `0o555`)

During artifact publication, `store_file` sets mode `0o444` (`r--r--r--`) on all stored objects.
During restoration of executable targets (tests, examples, binaries), Mitts marks the target
executable via `set_permissions(0o555)`. Because the hardlink shares the underlying inode,
this operation also modifies the mode of the CAS object itself to `0o555` (`r-xr-xr-x`).
Both states strictly forbid write access: standard file open modes (`O_WRONLY`, `O_RDWR`, `O_TRUNC`)
fail with `EACCES` (Permission Denied).

### 2. Atomic Target Promotion via `fs::rename`

Mitts execution does not compile directly into the destination target path. Compilers write
outputs into isolated staging directories under the CAS root (`create_staging_dir(ctx.cas.root(), ...)`).
Final artifacts are placed into their workspace destination via atomic `fs::rename`.

In POSIX semantics, `rename(2)` replaces the directory entry and unlinks the existing inode
at the destination rather than modifying data blocks in-place. If an artifact is recompiled,
a new inode is created in staging and atomically substituted at the target; the previous CAS
inode is unlinked from the workspace and remains unmodified.

### 3. Restoration Fallback Chain and Device Boundary Check (`st_dev`)

The restoration order in `restore_cas_object` is:
1. Attempt CoW reflink (`reflink_file`).
2. If reflink fails, check if hardlinks are enabled (`MITTS_CAS_HARDLINK=1`) and verify that
   the CAS store root and destination target directory reside on the exact same device
   (`st_dev` comparison). If both hold, link via `std::fs::hard_link`.
3. Otherwise, fall back to safe isolated copy (`fs::copy`).

Connecting runtime filesystem inspection (`fs_detect`) to selectively suggest or default this
behavior is tracked for future integration.

### 4. Proposed Hardlink Publishing (`store_file` via `link()`)

Currently, `store_file` publishes artifacts from staging to CAS via `reflink_or_copy`.
Because staging is created under the CAS root (`ctx.cas.root()`), staging and CAS reside on
the exact same filesystem by construction. We propose extending `store_file` so that when
`MITTS_CAS_HARDLINK=1` is active, staging artifacts are set to read-only and hardlinked into
`cas/objects/`, eliminating copy I/O during the publish phase as well.

### 5. Garbage Collection (GC) Semantics

If a CAS object is deleted during cache prune or garbage collection:
- Any existing hardlinks in active workspace target directories remain valid, accessible
  POSIX files. The underlying data blocks are only reclaimed by the OS once all directory
  entries (CAS and workspaces) are unlinked.
- Note on link counts: Inode link count (`st_nlink > 1`) is not a sufficient indicator of
  active workspace usage, because `named_artifact_path_with_disambiguator` (cas.rs:279) creates
  internal hardlinks within the CAS, and intra-workspace links may exist.

### 6. Integrity Verification and Writable Permission Rejection

During restoration, Mitts verifies the SHA-256 hash of the CAS object before linking or
copying. When restore is skipped because the target already matches the CAS object (Task 2),
skipping hash re-verification is safe because the object was already cryptographically
verified during its initial ingestion/restoration into the workspace.

Furthermore, the fast-path inode check in `TargetArtifactIndex::is_intact` actively inspects
permissions (`dest_meta.permissions().mode() & 0o222 == 0`). If an external tool, script,
or user attempts `chmod u+w` on a hardlinked target file, the writable bits cause `is_intact`
to immediately return `false`. The engine falls back to full `restore_cas_object`, which
cryptographically re-verifies the CAS object hash against corruption and re-establishes strict
read-only permissions (`0o444`/`0o555`).

## Residual Risks and Limitations

The safe hardlink strategy reduces risk via OS permissions and atomic replacement, but does
not offer hardware/kernel-enforced CoW block isolation. The following residual risks must be
explicitly recognized:

1. **Explicit Permission Changes (`chmod u+w`)**:
   If a user, build script, or external tool explicitly changes file permissions on a target
   file via `chmod(2)`, write access is restored and in-place mutation of the shared CAS inode
   becomes possible.
2. **In-place Permission Mutation**:
   Calling `set_permissions` on a hardlink modifies the inode metadata directly, mutating
   permissions on the CAS object as well (e.g. `0o444` becoming `0o555` on executable restore).
3. **Privileged Processes (root)**:
   Processes running with root / CAP_DAC_OVERRIDE capabilities bypass read-only permission bits
   and can mutate files in-place without triggering `EACCES`.
4. **Third-Party Build Tool Friction**:
   Any tool or script that opens an existing destination file path directly for writing
   (`open(O_WRONLY)`) without unlinking will receive `EACCES` on `0444`/`0555` files.

Because of these trade-offs, safe hardlinks are designated as an **experimental opt-in mode**
(`MITTS_CAS_HARDLINK=1`) rather than the default policy on non-CoW filesystems.

## Comparison of Storage Strategies

| Property | CoW Reflink (Btrfs / XFS / APFS) | Safe Read-Only Hardlink (ext4) | Full Copy Fallback |
| :--- | :--- | :--- | :--- |
| **I/O Latency** | Sub-second metadata clone | Sub-second directory link | Linear with byte size |
| **Disk Usage** | Shared physical blocks | Shared physical inodes | Fully duplicated per checkout |
| **Write Protection** | Hardware/kernel CoW split | Read-only permissions (`0444`) | Independent file copy |
| **Tool In-Place Writes**| Transparent CoW break | Blocked with `EACCES` | Allowed (isolated) |
| **Cross-Device** | Unsupported (copy fallback) | Unsupported (copy fallback) | Fully supported |
| **Default Policy** | Enabled by default | Experimental (`MITTS_CAS_HARDLINK=1`) | Safe default fallback |

Empirical benchmarks across these strategies are maintained in internal `benchmark-results/`
artifacts; public release of performance metrics remains subject to project owner approval.

## Implementation Roadmap

1. **Step 2 (Uncommitted Working-Tree Diff)**:
   - `src/core/fs_detect.rs`: Filesystem detection for Linux and macOS.
   - `src/core/reflink.rs`: Safe hardlink restore logic with read-only permission enforcement.
   - `src/core/cas.rs`: Delegation of `restore_object` to `restore_cas_object`.
   - `src/core/mod.rs`: Export of `fs_detect` module.
   - `src/cli.rs`: Documentation of `MITTS_CAS_HARDLINK=1` in `after_help`.
   - `docs/architecture.md`: Section 11 detailing filesystem detection and zero-copy strategy.
   - `tests/fs_detect_and_zero_copy_test.rs`: Unit and integration regression suite.
2. **Follow-Up Work**:
   - Enable `link()` in `store_file` for publish from staging.
   - Optimize restoration by skipping hash verification when target already matches CAS inode (Task 2).

## Decision

Status: Approved
Date: 2026-10-11
Owner Notes: Approved opt-in safe read-only hardlink restoration (`MITTS_CAS_HARDLINK=1`) with `0444`/`0555` mode enforcement and atomic replacement on same-device non-CoW filesystems (`ext4`).
