# Cargo commands in Mitts

Mitts has every command of stable Cargo, with Cargo's options. Each one reads the same
`Cargo.toml`, `Cargo.lock` and Cargo configuration ([cargo-config.md](cargo-config.md)), and
writes the same files. Build commands go through Mitts' shared cache; the others behave as
Cargo's do. Options every command takes: `--config`, `--color`, `-C DIR`, `-Z`,
`+TOOLCHAIN`, `-q`, `-v`, `--locked`, `--offline`, `--frozen` where Cargo has them.

| Command | What it does | Differences from Cargo |
|---|---|---|
| `build`, `check`, `test`, `run`, `bench`, `rustc`, `clippy`, `fmt` | Build with the shared cache | `--artifact-dir` (Cargo: unstable) copies final artifacts |
| `doc`, `rustdoc` | rustdoc for the package and its dependencies into `target/doc` | A crate whose inputs did not change is not documented again |
| `fix` | Applies rustc's machine-applicable suggestions until none are left | `--edition` applies the migration lints; set `edition` in Cargo.toml yourself |
| `new`, `init` | Package files, the repository and its ignore file (`git`, `hg`, `pijul`, `fossil`), workspace membership | |
| `add`, `remove` | Edit Cargo.toml in place, comments and order kept, then the lock | `--base`, `--public` (Cargo: unstable) are refused |
| `update`, `generate-lockfile` | Resolve Cargo.lock: `-p`, `--precise`, `--recursive`, `--workspace`, `--dry-run` | `--publish-time` (Cargo: unstable) is refused |
| `tree` | The dependency graph: `-i`, `-d`, `-e` (with `features`), `--depth`, `--prefix`, `--charset`, `-f`, `--prune` | |
| `metadata`, `pkgid`, `locate-project` | Package information as Cargo prints it | |
| `fetch`, `vendor` | Sources into the shared store (`fetch --target`: only what a build for that target uses); a `vendor/` copy with `.cargo-checksum.json` | |
| `package`, `publish` | A verified `.crate`; upload to crates.io or `--registry` | `--exclude-lockfile`, `--message-format` (Cargo: unstable) are refused |
| `install`, `uninstall` | Build a binary from a registry, `--path` or `--git` into `<root>/bin`, tracked in `.crates.toml` / `.crates2.json` | |
| `search`, `info`, `login`, `logout`, `owner`, `yank` | The registry's web API; tokens through the configured credential providers (`$CARGO_HOME/credentials.toml` by default) | |
| `report future-incompatibilities` | Shows the last `--future-incompat-report` | The report is produced by an extra check run when `--future-incompat-report` is given, not after every build |
| `clean` | Removes the target directory's Mitts artifacts: `-p`, `--doc`, `--profile`, `--target`, `--dry-run` | Only files Mitts recorded are removed; `--cache` also prunes the shared cache |
| `version`, `help` | | |

Commands Mitts adds: `explain` (the plan and why each unit is or is not cached),
`coverage` / `llvm-cov`, `gc` (prune the shared cache).

External subcommands work as in Cargo: `mitts NAME ARGS` runs `cargo-NAME NAME ARGS` from
`$CARGO_HOME/bin` or `PATH` (`mitts nextest run`, `mitts expand`), with `CARGO` set to Mitts,
so a tool that calls `$CARGO` back goes through Mitts. An unknown name is Cargo's
`no such command` error, with a suggestion for a typo. `mitts --list` prints the built-in
commands, aliases and the `cargo-NAME` tools found.

## `rust-version`

A build fails before compiling anything when a package of the build declares a
`rust-version` newer than the rustc in use, with Cargo's message. `--ignore-rust-version`
skips the check.
