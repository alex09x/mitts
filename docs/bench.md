# `mitts bench`

Compiles the benchmark targets of the selected packages and runs them, as `cargo bench` does.

```sh
mitts bench                              # every [[bench]] and benches/ target
mitts bench --bench speed                # one target (repeat the flag for several)
mitts bench fast -- --save-baseline main # BENCHNAME and arguments after `--` go to the binary
mitts bench -p foo --features turbo --no-run
```

## What it does

- Builds the benchmark targets in the **bench profile**: the release profile plus `[profile.bench]`
  when the manifest has one. Artifacts are in `target/release/` like Cargo's.
- Runs each benchmark binary with the working directory of its package and with `--bench`, the
  argument Cargo gives a benchmark binary so that it measures instead of testing. A libtest
  benchmark (`#[bench]`, nightly) and a `harness = false` benchmark such as Criterion's both
  understand it. `BENCHNAME` is passed before `--bench`, the arguments after `--` after it.
- Selection: without `--bench`, every benchmark target of the selected packages; with
  `--bench NAME`, only the named ones. The manifest key `bench = false` is not interpreted.
- Library and binary unit tests, integration tests and documentation tests are **not** run.
  (Cargo also runs a library's or a binary's own `#[bench]` functions; Mitts runs the benchmark
  targets only.)
- `--no-run` compiles and stops. `--no-fail-fast` runs the remaining targets after a failure.

Package selection (`-p`, `--workspace`, `--exclude`), features (`-F`, `--no-default-features`,
`--all-features`), `--target`, `--target-dir`, `--manifest-path`, `--locked`, `--offline`,
`--frozen`, `--cache-dir`, `--no-cache`, `-j` and `-v` work as for `mitts test`.

A package without benchmark targets prints `mitts bench: no benchmark targets found to run` and
exits 0.

## Not covered

`mitts bench` does not summarise or store the measurements: the benchmark binary prints them.
`mitts test --benches` is a different command: it runs the targets with the test harness, not to
collect measurements.
