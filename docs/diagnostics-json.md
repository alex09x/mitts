# Machine-readable diagnostics

`mitts check` and `mitts clippy` accept `--message-format`:

| Value | Meaning |
|---|---|
| `human` (default) | Progress, banners and rendered diagnostics, as before. |
| `json` | Cargo-compatible JSON lines on stdout; nothing else is printed there. |

Both `--message-format json` and `--message-format=json` work. Any other value is a usage
error with exit code 2.

## The stream

With `json`, each line of stdout is one JSON object with a `reason`.

`compiler-message` is written once per compiler diagnostic, in the order units finish:

```json
{"reason":"compiler-message","package_id":"demo@0.1.0","manifest_path":"/w/demo/Cargo.toml",
 "target":{"kind":["lib"],"crate_types":["lib"],"name":"demo","src_path":"/w/demo/src/lib.rs","edition":"2021"},
 "message":{"$message_type":"diagnostic","level":"error","code":{"code":"E0308"},"spans":[],"rendered":"..."}}
```

`message` is the diagnostic exactly as rustc (or Clippy) emitted it, including `level`, `code`,
`spans`, `children` and `rendered`. `target.kind` and `target.crate_types` both hold the unit's
crate-type list. `package_id` is Mitts' own package id (`name@version`), not Cargo's URL form.

`build-finished` is always the last record:

```json
{"reason":"build-finished","success":false}
```

`success` is false when any unit failed or the build could not start. The process exit code is
the same as in human mode.

## stdout and stderr

stdout holds only records. The banner, the progress bar and the final summary are not printed in
`json` mode. Everything else goes to stderr: the `[mitts::stats]` line, errors that stop a build
before any unit ran, and any text that is not a diagnostic (for example output of a rustc
wrapper).

## Cache

JSON diagnostics use their own entries in the action cache, so a human-mode run never replays text
into the JSON stream and the other way round. A cached unit replays its stored diagnostics as
records, so a second run prints the same warnings without compiling.

## Job limit

`-j N` (also `--jobs N`) on `build`, `check`, `clippy`, `test`, `run` and `coverage` caps
concurrent native build units like `MITTS_JOBS=N`. The flag wins when both are set. `N` must be a
positive integer.
