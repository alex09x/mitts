# Coverage reports

`mitts coverage` and `mitts llvm-cov` build instrumented test binaries, run them and report with
LLVM's tools. They need the `llvm-tools-preview` component of the active toolchain.

## Which files are reported

Every report format (summary table, `--lcov`, `--json`, `--text`, `--html`) names a file by the
path rustc recorded, and the same path is what `--ignore-filename-regex` is matched against.

| Source | Reported as |
|---|---|
| Workspace member or any package under the workspace root, including a path dependency that has its own `Cargo.lock` | its path relative to the workspace root, for example `crates/a/src/lib.rs` or `vendored/dep/src/lib.rs` |
| Path dependency outside the workspace root | `external/<name>-<version>/src/...` |
| Registry and git dependencies, Mitts' source store, the Rust standard library | left out of every report by default |

Paths are unique, so a regex can single out one crate, and the totals are the same on every
operating system for the same sources.

The recorded names are made unique when the instrumented units are compiled: rustc runs in the
workspace root for these units and maps each package's directory to its label above. Builds
without coverage instrumentation are not affected, and instrumented units carry a separate
cache key, so older cached coverage objects are never reused.

## Filtering and thresholds

`--ignore-filename-regex <RE>` removes matching files from every report and from the totals
that `--fail-under-lines` checks, so the gate measures the filtered code:

```sh
mitts llvm-cov --ignore-filename-regex '(tests/|vendored/)' --fail-under-lines 90
```

## Writing a report to a file

`--output-path <FILE>` writes a single-file report to exactly that path (parent directories are
created) and keeps it off stdout. It applies to exactly one of `--json`, `--lcov` and `--text`:

```sh
mitts llvm-cov --workspace --json --summary-only --output-path target/coverage-report.json
```

`--summary-only` with `--json` exports per-file summaries and totals instead of every line
segment. `--html` writes a directory, so it uses `--output-dir` instead. Combining
`--output-path` with `--html`, with no format, or with more than one format is a usage error
(exit code 2).
