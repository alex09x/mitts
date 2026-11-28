# Contributing to Mitts

Thanks for wanting to help. Bug reports, fixes, tests and docs are all welcome.

## Reporting a bug

Open an issue at <https://github.com/alex09x/mitts/issues> with:

- `mitts --version`, `rustc -vV`, your OS and architecture;
- the exact command and the full output (`-v` or `-vv` shows every rustc command);
- whether `cargo` does something different on the same project and command — a
  difference from Cargo is a bug unless it is documented;
- a small project that shows it, or the public repository and commit.

Cache problems are the most useful reports: a unit restored when it should have been
rebuilt, or rebuilt when nothing changed. `mitts explain` shows every unit and why it
hit or missed; `MITTS_LOG_KEYS=1` prints the inputs of each key.

## Finding something to work on

Issues labelled [`good first issue`](https://github.com/alex09x/mitts/labels/good%20first%20issue)
are small and self-contained. Each says how hard it is (easy / medium / hard) and which
files it touches. Comment on the issue before you start, so two people don't do the same
work.

## Building and testing

Mitts builds and tests itself. Install the latest release, then use it on the checkout:

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
git clone https://github.com/alex09x/mitts && cd mitts
mitts build
mitts test --test <name>          # one integration test file while you work
scripts/run_tests_isolated.sh     # the whole suite, before you open a PR
mitts fmt && mitts clippy --all-targets -- -D warnings
```

Rules that keep the project honest:

- **Use Mitts, not Cargo,** to build, test, lint or format this repository. The one
  exception is `tests/cargo_differential_test.rs`, which runs Cargo as the reference
  Mitts is compared with. If Mitts can't do something you need, that is an issue to
  file, not a reason to fall back to Cargo.
- **Every test that builds uses its own cache** (`cache_dir`, `--cache-dir` or
  `MITTS_CACHE_DIR` on a temporary directory). `scripts/run_tests_isolated.sh` fails if
  a test touched the real store in `~/.cache/mitts`.
- **A cache hit needs every input to match.** An input Mitts can't account for means a
  rebuild or no caching, never a guess. A change to a key comes with a test that the
  key changes when that input changes, and stays the same when it doesn't.
- **A bug fix comes with a test** that fails before the fix and passes after it.

[docs/architecture.md](docs/architecture.md) explains how planning, keys and the store
fit together.

## Pull requests

- One topic per PR, with a description of what changed and how you checked it.
- CI runs the suite on Linux, macOS and Windows; it has to be green before a merge.
- Keep the existing license and copyright headers in files you edit.
- AI-assisted changes are fine, as long as you have read the result, understand it and
  tested it. The PR description is yours to stand behind.

## License

Mitts is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option. Unless you say otherwise, a contribution you submit is licensed the same
way, without any additional terms.
