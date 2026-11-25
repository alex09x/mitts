# Mitts for Zed

Two pieces of configuration for [Zed](https://zed.dev):

- [`tasks.json`](tasks.json): Zed's own Rust tasks (check, test the function, doc test,
  module, run the binary, test the package, run, clean) with
  [Mitts](https://github.com/alex09x/mitts) in place of Cargo. They carry Zed's Rust tags,
  so the run buttons in the gutter offer them next to the Cargo ones. Run them with
  `task: spawn`; paths in the output are clickable.
- [`settings.json`](settings.json): rust-analyzer's on-save check and its runnables run
  through Mitts, so errors from `mitts check` show up as diagnostics. rust-analyzer still
  loads the project with `cargo metadata` and runs build scripts with Cargo.

## Install

Copy the tasks into `~/.config/zed/tasks.json` (`zed: open tasks`) and merge the `lsp`
section into `~/.config/zed/settings.json` (`zed: open settings`); or put them in a
project's `.zed/tasks.json` and `.zed/settings.json`. Mitts itself must be on `PATH`.

## Tests

```sh
python3 editors/zed/test/check.py                    # the files
MITTS_ZED_E2E=1 python3 editors/zed/test/check.py    # plus real Mitts runs
```

They check that every task runs Mitts with only the variables Zed's Rust support defines
and its Rust tags, and the rust-analyzer settings. With `MITTS_ZED_E2E=1`, on a broken
project: the workspace check task fails with the type error, and the command
rust-analyzer is given prints the error as `compiler-message` JSON for the binary and its
test build, ending with `build-finished`, as `cargo check --message-format=json` does.
Zed itself is not started. CI runs them on Linux, macOS and Windows
(`.github/workflows/editors.yml`).
