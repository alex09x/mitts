# Cargo configuration in Mitts

Mitts reads the same configuration as Cargo: `$CARGO_HOME/config.toml`, every
`.cargo/config.toml` from the filesystem root down to the current directory, and
`--config KEY=VALUE` / `--config FILE` on the command line. A value set in several places
follows Cargo's order: `--config` first, then the `CARGO_*` environment variable, then the
nearest file. Relative paths and programs in a file are relative to the directory that
holds its `.cargo` directory.

## Supported keys

| Key | Environment variable | Notes |
|---|---|---|
| `build.jobs` | `CARGO_BUILD_JOBS` | Below `-j` and `MITTS_JOBS`. Negative: that many CPUs fewer. `"default"`: all CPUs. |
| `build.rustc` | `CARGO_BUILD_RUSTC` | `RUSTC` wins over it. |
| `build.rustdoc` | `CARGO_BUILD_RUSTDOC` | `RUSTDOC` wins over it. |
| `build.rustflags`, `target.<triple>.rustflags`, `target.'cfg(..)'.rustflags` | `CARGO_BUILD_RUSTFLAGS`, `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` | `CARGO_ENCODED_RUSTFLAGS` / `RUSTFLAGS` win. |
| `build.rustdocflags`, `target.<triple>.rustdocflags` | `CARGO_BUILD_RUSTDOCFLAGS` | `CARGO_ENCODED_RUSTDOCFLAGS` / `RUSTDOCFLAGS` win. Used by doctests. |
| `build.rustc-wrapper`, `build.rustc-workspace-wrapper` | `CARGO_BUILD_RUSTC_WRAPPER`, … | |
| `build.target`, `build.target-dir` | `CARGO_BUILD_TARGET`, `CARGO_TARGET_DIR` | |
| `build.incremental` | `CARGO_BUILD_INCREMENTAL` | `CARGO_INCREMENTAL` wins; wins over the profile. |
| `[alias]` | `CARGO_ALIAS_<NAME>` | Strings or arrays; aliases may use aliases; built-in `b`, `c`, `t`, `r`. A user alias never shadows a command. |
| `target.<triple>.runner`, `target.'cfg(..)'.runner` | `CARGO_TARGET_<TRIPLE>_RUNNER` | Used by `run`, `test` (also `-c`). Two matching `cfg` runners are an error. |
| `target.<triple>.linker` | `CARGO_TARGET_<TRIPLE>_LINKER` | |
| `target.<triple>.<links>` | | Build script override for a package with `links = "<links>"`: `rustc-link-lib`, `rustc-link-search`, `rustc-flags`, `rustc-cfg`, `rustc-env`, `rustc-cdylib-link-arg`, other keys as `DEP_<LINKS>_<KEY>` metadata. The script is not run. |
| `[env]` | | `force`, `relative`. |
| `[profile.*]` | `CARGO_PROFILE_<NAME>_<KEY>` | One default differs from Cargo: see [Debug info](#debug-info-line-tables-by-default). |
| `[registries]`, `[registry]`, `[source]`, `[credential-alias]`, `[patch]` | `CARGO_REGISTRIES_<NAME>_*` | |
| `net.offline` | `CARGO_NET_OFFLINE` | Same as `--offline` on every command that has it. |
| `net.retry` | `CARGO_NET_RETRY` | `MITTS_NET_RETRY` wins. |
| `http.proxy` | `CARGO_HTTP_PROXY` | Without it: `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` (either case). `NO_PROXY` is not read yet. |
| `http.cainfo` | `CARGO_HTTP_CAINFO` | PEM bundle that replaces the built-in roots. |
| `http.timeout` | `CARGO_HTTP_TIMEOUT` | Seconds to connect and between reads. |
| `term.verbose`, `term.quiet` | `CARGO_TERM_VERBOSE`, `CARGO_TERM_QUIET` | The command line wins; both set is an error. |
| `term.color` | `CARGO_TERM_COLOR` | `--color` wins. |

## Debug info: line tables by default

When no `debug` is set for the profile (in `Cargo.toml`, a Cargo config file or
`CARGO_PROFILE_<NAME>_DEBUG`), `dev`, `test` and custom profiles that inherit `dev` build
with `-C debuginfo=line-tables-only`. Cargo's default there is full debug info. Panic
backtraces keep file and line numbers; the variable and type information that only a
debugger (gdb, lldb, an IDE) reads is left out. In a 750 MB test binary we measured, that
information was about 90% of the file.

On macOS, backtrace frames and lldb do not yet show file and line for the workspace's own
crates, with any `debug` setting (dependencies and the standard library do). This is not
new: Apple's linker leaves those crates' objects out of the debug map, because Mitts
strips the checkout path from them so another checkout can reuse the build. A fix is
planned.

To debug with variables, set `debug` explicitly; Mitts then passes it as written, like
Cargo:

```toml
# Cargo.toml for one project, or ~/.cargo/config.toml for every project of the user
[profile.dev]
debug = true
```

For a single run: `CARGO_PROFILE_DEV_DEBUG=true mitts build`. `release` and `bench` are
unchanged (no debug info unless set).

## Accepted and ignored

Keys that only tune Cargo itself or do not apply to Mitts' layout are accepted without
effect: `http.ssl-version`, `http.check-revoke`, `http.multiplexing`, `http.debug`,
`http.low-speed-limit`, `http.user-agent`, `term.progress.*`, `term.hyperlinks`,
`term.unicode`, `cache.*`, `future-incompat-report.*`, `target.<triple>.ar` (Cargo ignores
it as well), `build.build-dir` and `build.dep-info-basedir` (Mitts keeps intermediate files
in its cache), `paths` (superseded by `[patch]`), `net.git-fetch-with-cli` (Mitts always uses
the system `git`), `resolver.incompatible-rust-versions`. `doc.browser`, `install.root` and
`cargo-new.*` belong to commands Mitts does not have yet.
