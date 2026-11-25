# Mitts for Helix

Helix has no plugins yet; the integration is configuration.

- [`languages.toml`](languages.toml): rust-analyzer's on-save check and its runnables run
  through [Mitts](https://github.com/alex09x/mitts), so errors and warnings from
  `mitts check` show up as diagnostics. rust-analyzer still loads the project with
  `cargo metadata` and runs build scripts with Cargo.
- [`config.toml`](config.toml): `Space m` then `c` check, `b` build, `t` test, `r` run,
  `k` clippy: runs Mitts from Helix's working directory and shows its output.

Copy the sections into your own files: `~/.config/helix/` (Windows:
`%AppData%\helix\`), or a project's `.helix/languages.toml`. Mitts itself must be on
`PATH`.

## Tests

```sh
python3 editors/helix/test/check.py     # hx on PATH, or HX=/path/to/hx
```

It checks the values, that `hx --health rust` reads `languages.toml` without an error,
and on Unix that Helix starts with `config.toml` in a pseudo-terminal without "Bad
config". Each check has a broken counterpart that must be reported, so a check that can
never fail would show. It does not press the keys: running `:sh mitts ...` is left to
Helix. CI runs it with Helix's own release build on Linux, macOS and Windows
(`.github/workflows/editors.yml`).
