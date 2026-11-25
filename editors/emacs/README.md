# Mitts for Emacs

`mitts.el` runs [Mitts](https://github.com/alex09x/mitts) with `compile` from the
workspace root, so rustc's errors and warnings are links in the compilation buffer
(`next-error`, `M-g n`). It can make rust-mode and rustic run Mitts where they run
Cargo, and route rust-analyzer's check through Mitts in Eglot and lsp-mode.

Emacs 27.1 or later.

## Install

`use-package` with `:vc` (Emacs 30):

```elisp
(use-package mitts
  :vc (:url "https://github.com/alex09x/mitts" :lisp-dir "editors/emacs")
  :bind-keymap ("C-c m" . mitts-command-map)
  :config
  (mitts-cargo-mode 1)        ; rust-mode / rustic run Mitts
  (mitts-eglot-setup))        ; or (mitts-lsp-setup) with lsp-mode
```

By hand: put `editors/emacs/mitts.el` on `load-path` and `(require 'mitts)`.

Mitts itself must be on `exec-path` (or set `mitts-executable`).

## Use

| | |
|---|---|
| `M-x mitts-check`, `mitts-build`, `mitts-test`, `mitts-run`, `mitts-clippy` | runs from the workspace root |
| `M-x mitts-compile` | any Mitts command line (`build --release`, `test -p foo`) |
| `mitts-command-map` | `c` check, `b` build, `t` test, `r` run, `k` clippy, `m` any; bind it to a prefix |

The workspace root is the one `mitts locate-project --workspace` names, as Cargo would.
Without Mitts installed, it is the nearest directory upwards whose `Cargo.toml` has a
`[workspace]` table, else the nearest with a `Cargo.toml`.

Emacs 29 and later read rustc's diagnostics with their built-in `rust` pattern; on 27 and
28 mitts.el adds the same pattern.

## rust-mode, rustic, rust-analyzer

- `mitts-cargo-mode`: a global minor mode that sets `rust-cargo-bin` (rust-mode) and
  `rustic-cargo-bin` (rustic) to Mitts; turning it off puts the old values back.
- `mitts-eglot-setup`: adds to the default `eglot-workspace-configuration`
  `rust-analyzer.check.overrideCommand` = `mitts check --workspace --all-targets
  --message-format=json` and `rust-analyzer.runnables.command` = `mitts`, keeping your
  other settings. `mitts-rust-analyzer-settings` returns the plist for a `.dir-locals.el`.
- `mitts-lsp-setup`: sets `lsp-rust-analyzer-cargo-override-command` for lsp-mode. lsp-mode
  has no setting for the runnables' command; they still run Cargo.

rust-analyzer still loads the project with `cargo metadata` and runs build scripts with
Cargo; only the check (and in Eglot the runnables) goes through Mitts.

## Tests

```sh
cd editors/emacs
emacs --batch -L . -l test/mitts-test.el -f ert-run-tests-batch-and-exit
```

With `MITTS_EMACS_E2E=1` the tests also run `mitts-check` on a broken project. CI runs
Emacs 27.2 and 30.1 on Linux and Windows, 28.2 and 30.1 on macOS (Apple silicon has no
27.2 build), in `.github/workflows/editors.yml`.
