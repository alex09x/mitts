# How Mitts works

This document describes the build and cache protocol as implemented. Each
section names the module that owns the behavior. Where the implementation has
a known gap, the gap is stated next to the rule it weakens.

## 1. Principles

1. **A cache hit needs a complete key.** Every input that can change an output
   must be part of the action key. An input Mitts cannot see is a reason to
   rebuild, never a reason to reuse.
2. **Shared objects are immutable.** Published store objects are read-only and
   verified by hash before use. Workspaces get their own copies or reflinks.
3. **The language core is neutral.** The store, action cache, locks and GC know
   nothing about Cargo. The Rust adapter turns a Cargo workspace into actions;
   the C adapter does the same for `Mitts.toml`.
4. **Cargo stays the contract.** `Cargo.toml` and `Cargo.lock` are read as is.
   Mitts never writes a lockfile of its own.

## 2. Pipeline

```
resolve → fetch → plan → key → execute → publish → record fast path
```

| Phase | What happens | Module |
| --- | --- | --- |
| Resolve | Reuse `Cargo.lock` when it matches the manifests; otherwise resolve against the crates.io sparse index and write a standard `Cargo.lock`. | `adapter/rust/resolver.rs`, `sparse_index.rs`, `lockfile.rs` |
| Fetch | Download missing `.crate` archives in parallel, verify their SHA-256 from `Cargo.lock`, unpack once per machine. | `adapter/rust/fetcher.rs`, `core/source_store.rs` |
| Plan | Build compilation units (lib, bin, test, build script, proc macro) with features, targets and dependency edges. | `adapter/rust/planner.rs`, `features.rs`, `target_filter.rs` |
| Key | Derive one action key per unit in topological order. | `adapter/rust/key.rs`, `core/action_cache.rs` |
| Execute | Run the unit DAG in parallel; restore hits, compile misses. | `adapter/rust/dag_runner.rs`, `dag_scheduler.rs`, `dag_unit.rs` |
| Publish | Store outputs in the CAS and record the action cache entry. | `core/cas.rs`, `core/action_cache.rs` |

## 3. Action keys

An action key is `SHA-256` over a canonical text payload:

```
toolchain: <rustc -vV>
target:    <target triple>
profile:   <dev | release | …>
flags:     <unit flags, in invocation order>
features:  <sorted feature list>
sources:   <sorted (path, content hash) pairs>
deps:      <sorted (dependency unit, dependency action key) pairs>
```

- **Sources.** For a registry dependency, the package files downloaded with the
  package. For workspace units, input tracking is isolated at the unit level:
  - *Package common inputs*: manifests (`Cargo.toml`), lockfiles (`Cargo.lock`),
    applicable configs (`.cargo/config.toml`, `clippy.toml`), and package root files
    (including `build.rs`).
  - *Unit coarse inputs*: library and binary units track the `src/` directory;
    integration tests, benchmarks, and examples track only their declared `src_path`
    (or `<dir>/**` for multi-file runner suites). Editing an integration test never
    cascades invalidations to sibling tests or the library crate.
  - *Extra inputs*: files read by the compiler outside the unit's coarse set
    (such as `tests/support/`, `#[path]` targets, `include_str!`, `include_bytes!`)
    are extracted from `rustc` dep-info (`.d`) and recorded in the action manifest,
    ensuring precise invalidation without over-invalidating unrelated units.
- **Environment.** `CARGO_*`, `RUST_*`, `RUSTFLAGS`, `RUSTDOCFLAGS`,
  `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `TARGET`, `HOST` and `PROFILE`
  enter the flags. Variables read through `env!` / `option_env!` are discovered
  per unit and keyed with their values, with `<unset>` recorded explicitly.
- **Dependencies.** A unit's key includes its dependencies' keys, not their
  output bytes. A change anywhere below a unit changes its key.

Workspace crates compile with `--remap-path-prefix=<workspace_root>/=` to
stabilize compiler diagnostics, `file!()`, panic locations, and debuginfo to relative
paths (e.g. `src/main.rs`), maintaining full Cargo parity and terminal/IDE clickability.
Units that do not read `CARGO_MANIFEST_DIR` at compile time, do not have a custom `build.rs`,
do not depend on a proc macro the project declared impure, and reside within the
workspace root receive path-independent action keys with `REMAP_PATH_PREFIX=relative`
and `MANIFEST_DIR_REL=<rel>`, enabling 100% cache hits across independent checkouts,
clones, and agent worktrees.
Crates that read `CARGO_MANIFEST_DIR` via `env!` / `option_env!`, define `build.rs`,
depend on a declared impure proc macro, or reside outside the workspace root
(`../shared`) conservatively retain the local absolute path in their action key.
Proc macros are pure by default. A project lists the ones that read package files or
the package path while expanding (`sqlx`, `askama`, `rust-embed`, a local derive) under
`[workspace.metadata.mitts] impure-proc-macros` in the root `Cargo.toml`
(`adapter/rust/project_config.rs`); their direct dependents are then keyed on the whole
package tree, so an edited template invalidates them, and on the absolute path.
There is no built-in list: a macro Mitts does not know is treated like any other.
As an additional defense-in-depth safeguard before publishing to the CAS, Mitts scans
all generated artifacts for the byte sequence of the workspace root path; if found, the artifact
is never published under the path-independent key.
For debuggers (`lldb`/`gdb`), relative source paths resolve naturally when running from
the workspace directory; if debugging from an external directory, target source mapping
can be configured via `settings set target.source-map "" <workspace_root>`.

## 4. Store layout

```
--cache-dir | $MITTS_CACHE_DIR | $XDG_CACHE_HOME/mitts | ~/.cache/mitts
├── cas/
│   ├── objects/<sha256>          immutable objects, mode 0444
│   ├── artifacts/                named views of objects for rustc -L / --extern
│   ├── build_scripts/<base>/<exec>/{meta.toml, out/}
│   └── tmp/                      staging for atomic publish
├── ac/manifests/                 action cache entries
├── locks/                        single-flight locks per action key
├── jobserver/                    managed token pool
├── sources/<crate>-<version>/    unpacked registry sources
└── index/                        cached sparse-index entries
```

One resolver (`core::cache_root`) picks that root for every store, so an explicit
`--cache-dir` (or `RustBuildOptions::cache_dir`) relocates all of them together;
nothing falls back to `~/.cache/mitts` on its own.

The workspace sees only `target/mitts/` (or `$CARGO_TARGET_DIR/mitts`), which
holds restored outputs, `build.log` and the fast-path snapshots. Mitts creates
`CACHEDIR.TAG` and a `.gitignore` there.

## 5. Executing a unit

For every unit whose dependencies are ready:

1. **Look up** the action key in the action cache.
2. **Hit:** verify every referenced object's hash, then restore it into the
   workspace with a reflink (APFS, Btrfs, XFS) or a full copy. A corrupt object
   is discarded and the unit is rebuilt.
3. **Miss:** take the per-key lock and look up the key again (double-checked
   locking: another process may have finished it while we waited). If it is
   still missing, compile.
4. **Publish:** copy each output into `cas/tmp/`, mark it read-only, then
   `rename` it into `objects/`. An interrupted build never leaves a partial
   object. The action cache entry is written only after all outputs are stored.

### Scheduling and concurrency

- Units run in waves over a work queue. A worker first tries the key lock
  without blocking. If another build holds it, the unit is marked *contested*
  and the worker moves to other ready units, returning to it later.
- Two terminals or worktrees building the same dependency therefore compile it
  once; the second one reuses the result.
- `rustc` invocations are pipelined: dependents start from `.rmeta` as soon as
  metadata is ready, before the `.rlib` is finished.
- Parallelism is bounded by a jobserver shared by all Mitts processes on the
  machine (`core/jobserver.rs`). An inherited `--jobserver-auth` from a parent
  build is respected.

## 6. Build scripts

Build scripts are compiled and executed by Mitts and their results are cached.

- **Base key:** the build script's sources, package, features, target, profile,
  tracked environment and build-dependency keys.
- **Execution entry:** the directives it printed (`rustc-cfg`, `rustc-link-lib`,
  `rustc-env`, …), its `OUT_DIR`, and the freshness evidence it declared:
  content hashes of `rerun-if-changed` paths and values of
  `rerun-if-env-changed` variables.
- A cached execution is reused only if every declared path still has the same
  content hash and every declared variable has the same value. Otherwise the
  script runs again.
- The script runs in its own staging directory with Cargo's standard
  environment (`OUT_DIR`, `TARGET`, `HOST`, `PROFILE`, `CARGO_FEATURE_*`, …).
  It is not an OS-level sandbox: a script that reads undeclared files or
  variables can still produce a stale hit. Declaring inputs is the script
  author's responsibility, as it is with Cargo.
- **Upfront Execution & Dynamic Keying:** For units declaring build scripts, the
  script runs upfront in an isolated staging directory before acquiring the action
  lock or checking the compilation action cache. Compile-affecting directives
  (`rustc-cfg`, `rustc-check-cfg`, `rustc-flags`, `rustc-link-lib`, `rustc-link-search`,
  `rustc-link-arg*`, `rustc-env`) are canonicalized and hashed into the unit's dynamic
  action key (`<base_key>:bd:<directives_hash>`). Concurrent builds of the same unit
  both execute their build script in separate staging directories (restoring from CAS
  on a cache hit; harmlessly executing on a cold cache) and serialize only when locking
  the resulting directive-aware compilation key.

## 7. No-op fast path

After a successful build Mitts writes `target/mitts/build-<profile>.lock`. It
records the toolchain, package selection, features and stat snapshots (path,
size, mtime) of `Cargo.lock`, manifests, source files, source directories and
outputs.

On the next build, if every snapshot still matches, Mitts skips planning and
keying entirely. Directory snapshots catch files that were added or removed.
Any mismatch falls through to the normal keyed build, so the fast path can
only skip work that the full protocol would also skip. It is local to one
workspace and never shared.

## 8. Garbage collection

`mitts gc` prunes the store by age and by total size (oldest first).

- Objects referenced by an active build are never removed; GC respects the
  same locks as builds.
- Build-script entries are removed by an atomic rename to `.gc.*` and then
  deleted, so a concurrent reader sees either the whole entry or none of it.
  Access is tracked with a `.last_used` marker.
- Leftover `tmp` files older than one hour are removed.

## 9. Compatibility with Cargo

- Mitts never writes into Cargo's `target/debug` or `target/release`.
  `cargo build`, rust-analyzer and Mitts can run side by side.
- `mitts build --cargo` delegates execution to Cargo; it is the differential
  oracle used to check diagnostics, exit codes and program behavior.
- Unsupported cases are reported as unsupported, not as a successful build.

## 10. Not yet supported

- Path-independent keys for workspace crates (cross-worktree reuse of your own
  code).
- A remote or team cache. The planned design is a second store tier over
  S3/HTTP, written only by trusted CI, with the action cache kept separate from
  any CDN.
- Cross-compilation beyond the host target.
- Windows (locks and the jobserver are POSIX-only today).
- Empty or omitted `workspace.members` defaults strictly to the root package (`.`); implicit path-dependencies outside the root package are not auto-promoted to members and must be listed in `workspace.members`.
- Wildcards in intermediate directories (`crates/*/sub`) and character-class globs (`?`, `[...]`) in `workspace.members` are unsupported and fail closed.

## 11. Diagnostic Environment Variables

- `MITTS_TIMINGS=1`: Enables sub-millisecond phase timing breakdowns (`[mitts::phases]`) and I/O byte and file counters (`[mitts::io]`) on stderr.
- `MITTS_NO_REFLINK=1`: Diagnostic flag to disable copy-on-write block cloning (`FICLONE` on Linux, `clonefile` on macOS), forcing fallback to byte copying for isolation studies and benchmarking.
- `MITTS_VERIFY_RESTORE=1`: Re-hash every CAS object before restoring it. Objects are published atomically and stored read-only, so by default a restore trusts them and does not read their bytes; this flag restores the full integrity check (one read of every restored byte) for audits or suspect disks.
- `MITTS_CACHE_IO_BUDGET=<n>`: Number of DAG workers allowed in the cache-read phase at once (`0` = unbounded). Default: `1` on macOS, unbounded elsewhere.
- `MITTS_STORE_HARDLINKS=0|1`: how a store object reaches `target/`. Default (`auto`): reflink where the filesystem can, otherwise a read-only hardlink to the immutable object, so ext4 stops copying every artifact byte (actix-web, fresh 12-worktree swarm: 4.5 s → 0.22 s; on XFS and APFS reflink is within noise of a link). The shared inode stays `0o444` (`0o555` for executables) and the target index replaces a link that gained write bits; a tool that writes workspace files in place hits EACCES, and `chmod u+w` followed by a write would reach the store object (`MITTS_VERIFY_RESTORE=1` catches that). `0` restores the copy-only policy for such workspaces; `1` prefers links everywhere.
- Registries, mirrors and vendor directories follow Cargo's config (`src/adapter/rust/registry_config.rs`): `[registries.<name>] index`, `[source.<name>] registry | directory | git … replace-with`, tokens through Cargo's credential providers (`credential_provider.rs`: `cargo:token` = `CARGO_REGISTRIES_<NAME>_TOKEN` / `[registries.<name>] token` / `credentials.toml`; `cargo:macos-keychain` via `security find-generic-password -s cargo-registry:<index>`; `cargo:libsecret` via `secret-tool lookup url <index>`; `cargo:token-from-stdout <cmd>` with `CARGO_REGISTRY_INDEX_URL`/`CARGO_REGISTRY_NAME_OPT`; any other spec is a plugin run with `--cargo-plugin` speaking protocol v1, `not-found`/`url-not-supported` fall through). `[registries.<name>] credential-provider` (or `CARGO_REGISTRIES_<NAME>_CREDENTIAL_PROVIDER`) wins over `[registry] global-credential-providers`, whose later entries are tried first; `[credential-alias]` expands; nothing configured means `cargo:token`. Tokens are memoised per index URL for the process. A lock source id keeps naming the original registry; `RegistryConfig::effective_source` says where its bytes come from. Packages of a registry other than crates.io, and vendored git packages, unpack into `<cache root>/sources/src-<digest of the lock source>/`; each sparse index other than crates.io caches its entries and `config.json` under `<cache root>/index/<digest of the index URL>/`. Lockfile resolution (`resolve_graph.rs`) is feature-aware: an optional dependency of a transitive package enters the lock only when a feature activates it, `[patch]` (`patch_resolve.rs`) replaces registry packages, and requirements in one compatibility range unify by restarting with the conflicting requirement pinned.
- `MITTS_INDEX_TTL_SECS=<n>`: how long a cached sparse-index entry list is trusted while online (default 900). Resolving a lockfile re-reads entries older than that, so a fresh lock sees today's versions; offline, and when the index cannot be reached, the cached copy serves.
- `MITTS_GIT=<path>`: the git binary Mitts runs to clone git dependencies into `<cache root>/git/` (bare mirror under `db/`, working tree per locked commit under `checkouts/<repo>-<url digest>/<rev7>/` with `.mitts-ok`). Default: `git` on `PATH`. Authentication is git's own (SSH keys and agents, credential helpers, `GIT_SSH_COMMAND`); `GIT_TERMINAL_PROMPT=0` is forced so a missing credential fails fast. The exact commit is fetched when the server allows it, else the locked tag or branch, else all refs.
- `MITTS_NO_HARDLINK=1`: Diagnostic flag to disable intra-workspace zero-copy hardlinks, forcing fallback to byte copying or reflink.


