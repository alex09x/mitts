# Machine-readable and compact diagnostics

`mitts build`, `mitts check`, `mitts clippy`, `mitts test`, `mitts bench`, and `mitts run` accept `--message-format`:

| Value | Meaning |
|---|---|
| `human` (default) | Progress, banners and rendered multi-line diagnostics. |
| `short` | Single-line `file:line:col: level: message` diagnostics on stderr without Mitts banners. |
| `json` | Cargo-compatible JSON lines on stdout; nothing else is printed there during compilation. |
| `json-diagnostic-short` | JSON lines on stdout whose `compiler-message.message.rendered` uses single-line short text. |
| `json-diagnostic-rendered-ansi` | JSON lines on stdout whose `compiler-message.message.rendered` preserves ANSI color escapes. |
| `json-render-diagnostics` | Omits `compiler-message` records from stdout, renders diagnostics to stderr instead, and still emits `build-finished` on stdout. |

Cargo's comma-separated JSON modifiers work as well (`json,json-diagnostic-short`,
`json,json-diagnostic-rendered-ansi`, `json-render-diagnostics,json-diagnostic-short`, and
`json-render-diagnostics,json-diagnostic-rendered-ansi`). Both `--message-format json` and
`--message-format=json` work. Conflicting combinations (`human,json`, `json,json-render-diagnostics`,
`json-diagnostic-short,json-diagnostic-rendered-ansi`) or unknown values are rejected with exit code 2.

## Quiet mode (`-q` / `--quiet`)

`mitts build`, `mitts check`, `mitts clippy`, `mitts test`, `mitts coverage` (`llvm-cov`), `mitts bench`,
and `mitts run` accept `-q` / `--quiet`:

- Suppresses Mitts banners, progress bars, `[mitts::stats]`, and decorative stage footers.
- In `mitts test -q`, standard libtest binaries receive `-q` and per-binary `Running ...` / `Doc-tests ...`
  headers are suppressed; test failures and the final `test result: ...` summary remain visible and the
  exit code is unchanged.
- In `mitts test -c -q`, per-test `PASS [...]` lines and stage banners are suppressed while `FAIL [...]`
  output and the `Summary [...]` line remain visible.

## The JSON stream

With `json` (and `json-diagnostic-short` / `json-diagnostic-rendered-ansi`), each line of stdout during
the build phase is one JSON object with a `reason`.

`compiler-message` is written once per compiler diagnostic, in the order units finish:

```json
{"reason":"compiler-message","package_id":"demo@0.1.0","manifest_path":"/w/demo/Cargo.toml",
 "target":{"kind":["lib"],"crate_types":["lib"],"name":"demo","src_path":"/w/demo/src/lib.rs","edition":"2021"},
 "message":{"$message_type":"diagnostic","level":"error","code":{"code":"E0308"},"spans":[],"rendered":"..."}}
```

`message` is the diagnostic as rustc (or Clippy) emitted it, including `level`, `code`, `spans`,
`children` and `rendered`. `target.kind` and `target.crate_types` both hold the unit's crate-type
list. `package_id` is Mitts' own package id (`name@version`), not Cargo's URL form.

`build-finished` closes the compilation stream:

```json
{"reason":"build-finished","success":false}
```

`success` is false when any unit failed or the build could not start. The process exit code is
the same as in human mode.

## Cache

Each diagnostic format (`human`, `short`, `json`, `json-diagnostic-short`, `json-diagnostic-rendered-ansi`)
uses distinct entries in both the action cache and the fast build lock, so cached runs replay
diagnostics in the exact requested format without cross-contaminating other modes.

## Job limit

`-j N` (also `--jobs N`) on `build`, `check`, `clippy`, `test`, `bench`, `run` and `coverage` caps
concurrent native build units like `MITTS_JOBS=N`. The flag wins when both are set. `N` must be a
positive integer.
