# Mitts for JetBrains IDEs

A plugin for RustRover, IntelliJ IDEA and the other JetBrains IDEs (2026.2 and later)
that runs [Mitts](https://github.com/alex09x/mitts):

- **Tools → Mitts**: Check, Build, Test, Run, Clippy in the project's directory. Each
  makes a temporary run configuration (`mitts check`, ...) and runs it.
- A **Mitts** run configuration (Run → Edit Configurations → + → Mitts) for any command
  line after `mitts` (`test -p a --release`) and a working directory.
- rustc's error and warning locations (` --> src/main.rs:2:18`) in run consoles are links
  to the source.
- **Settings → Tools → Mitts**: the Mitts executable (default: `mitts` on `PATH`).

It does not replace RustRover's own Cargo support: its project model, run gutter icons
and test runner still use Cargo.

## Build and install

```sh
cd editors/jetbrains
./gradlew buildPlugin
```

Then **Settings → Plugins → ⚙ → Install Plugin from Disk…** and pick
`build/distributions/mitts-jetbrains-*.zip`. Gradle needs a JDK (the build uses Java 25
through a Gradle toolchain).

## Tests

```sh
./gradlew test                         # platform tests
MITTS_JB_E2E=1 ./gradlew test          # plus a real `mitts check` on a broken project
./gradlew verifyPlugin                 # JetBrains' Plugin Verifier: RustRover 2026.3, IDEA 2026.2
```

The platform tests check the console links on captured Mitts output (and none for
summaries, Mitts' own report frame or missing files), absolute paths, the run
configuration's defaults and saved state, how the command line is split, the executable
setting, and the menu's temporary configurations. With `MITTS_JB_E2E=1` the real
`mitts check` output is linked to `src/main.rs:2:18`. CI runs them on Linux, macOS and
Windows, and the Plugin Verifier on Linux (`.github/workflows/editors.yml`).
