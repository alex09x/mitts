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
| `new`, `init` | Package files, `.gitignore`, git repository, workspace membership | Version control: `git` or `none` (`hg`, `pijul`, `fossil` are refused with a warning) |
| `add`, `remove` | Edit Cargo.toml in place, comments and order kept, then the lock | `--base`, `--public` (Cargo: unstable) are refused |
| `update`, `generate-lockfile` | Resolve Cargo.lock: `-p`, `--precise`, `--recursive`, `--workspace`, `--dry-run` | `--publish-time` (Cargo: unstable) is refused |
| `tree` | The dependency graph: `-i`, `-d`, `-e`, `--depth`, `--prefix`, `--charset`, `-f`, `--prune` | `-e features` is refused; feature lists are in `mitts metadata` |
| `metadata`, `pkgid`, `locate-project` | Package information as Cargo prints it | |
| `fetch`, `vendor` | Sources into the shared store; a `vendor/` copy with `.cargo-checksum.json` | `fetch --target` fetches every locked source |
| `package`, `publish` | A verified `.crate`; upload to crates.io or `--registry` | `--exclude-lockfile`, `--message-format` (Cargo: unstable) are refused |
| `install`, `uninstall` | Build a binary from a registry, `--path` or `--git` into `<root>/bin`, tracked in `.crates.toml` / `.crates2.json` | |
| `search`, `info`, `login`, `logout`, `owner`, `yank` | The registry's web API; tokens in `$CARGO_HOME/credentials.toml` | Credential providers other than `cargo:token` are used for reading only |
| `report future-incompatibilities` | Shows the last `--future-incompat-report` | The report is produced by an extra check run when `--future-incompat-report` is given, not after every build |
| `clean` | Removes the target directory's Mitts artifacts: `-p`, `--doc`, `--profile`, `--target`, `--dry-run` | Only files Mitts recorded are removed; `--cache` also prunes the shared cache |
| `version`, `help` | | |

Commands Mitts adds: `explain` (the plan and why each unit is or is not cached),
`coverage` / `llvm-cov`, `gc` (prune the shared cache).

## `rust-version`

A build fails before compiling anything when a package of the build declares a
`rust-version` newer than the rustc in use, with Cargo's message. `--ignore-rust-version`
skips the check.
