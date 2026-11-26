# Mitts Build for VS Code

Run [Mitts](https://mittsbuild.com) from VS Code. Mitts is a build tool for Rust projects that reads
your existing `Cargo.toml` and `Cargo.lock` and keeps compiled artifacts in a shared,
content-addressed cache.

## What it does

- **Finds Mitts.** It uses the `mitts.path` setting if set, otherwise `PATH`, otherwise `~/.cargo/bin`.
  The status bar shows the version. If Mitts is missing, **Mitts: Install Mitts** runs the official
  installer in a terminal you can see: `curl -fsSL https://mittsbuild.com/install.sh | sh` on
  Linux and macOS, the PowerShell installer (`irm https://mittsbuild.com/install.ps1 | iex`) on
  Windows.
- **Tasks.** `mitts build`, `check`, `clippy`, `test`, `bench`, `run`, `doc` and `clean` show up
  under *Terminal → Run Task* for every folder with a `Cargo.toml`. Compiler errors and warnings
  from those tasks appear in the Problems panel through the `$mitts` problem matcher. You can
  define your own:

  ```json
  {
    "type": "mitts",
    "command": "test",
    "args": ["--workspace", "--release"]
  }
  ```

- **rust-analyzer, if you choose.** **Mitts: Use Mitts for rust-analyzer checks and runnables**
  writes two settings for this workspace:
  - `rust-analyzer.check.overrideCommand`, so the check that runs on save goes through `mitts check`;
  - `rust-analyzer.runnables.command`, so the ▶ Run and Test lenses go through Mitts.

  **Mitts: Stop using Mitts for rust-analyzer** removes exactly those two settings.

## What it does not do

- rust-analyzer still loads your project with `cargo metadata` and runs build scripts with Cargo.
  The extension does not change that.
- A plain `cargo` typed in a terminal, and tools that call Cargo themselves, still run Cargo. Nothing
  replaces the `cargo` command.
- Mitts does not have Cargo's external subcommands, such as `cargo nextest` or `cargo llvm-cov`.
  For the rest of the gaps, see the
  [command reference](https://github.com/alex09x/mitts/blob/main/docs/cargo-commands.md).

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `mitts.path` | empty | Path to `mitts`. If empty, it is looked up on `PATH`, then in `~/.cargo/bin`. |
| `mitts.extraArgs` | `[]` | Arguments added to every Mitts task. |

## Requirements

- Mitts 0.4.0 or newer is recommended, because it is the first version with every Cargo command.
  Older versions still run build, check and test.
- Linux, macOS, or Windows x86-64 with the MSVC toolchain (Mitts 0.5.0 or newer on Windows).
- A Rust toolchain from rustup.

## License

MIT OR Apache-2.0.
