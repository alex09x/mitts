# Benchmark environment

The 0.3.17 corpus series (100 projects, stages T0–T5) runs on three machines with one
toolchain. Every results file also records these versions in its `provenance` block; this page
is the human-readable copy. Hosts are described by hardware and OS only.

| Machine | CPU | Cores | RAM | OS / kernel | Filesystem under test |
|---|---|---|---|---|---|
| Linux x86_64 rig | AMD Ryzen 9 9950X | 32 | 186 GB | Ubuntu 24.04.3, kernel 6.8.0-111 | XFS (reflink) on NVMe |
| Linux aarch64 host | Ampere Altra (Neoverse-N1) | 128 | 250 GB | Ubuntu 24.04.2, kernel 6.8.0-147 | XFS (reflink) on NVMe |
| macOS workstation | Apple M2 Ultra | 24 | 128 GB | macOS 15.7.8 | APFS on the internal SSD |

Toolchain on all three, pinned on 2026-10-30:

| Component | Version |
|---|---|
| rustc | 1.99.1 (58cf8f9c3 2026-10-13) |
| cargo | 1.99.1 (5f94df478 2026-08-27) |
| clippy | shipped with the toolchain above |
| sccache | 0.17.0 |
| Mitts | 0.3.17 (tag `v0.3.17`, built from the tag on the Linux hosts, installed from crates.io on macOS) |

Series parameters: 100 projects from `scripts/corpus_100.json` at pinned commits, 10
consecutive first-parent commits for the history replay, a 12-worktree swarm over 4 commits,
3 repeats per series, every timed run gated on the machine's background CPU load (the
measured steady background is recorded with each file). The aarch64 host runs other work
alongside the series; its background note names it.

Each machine measures its own clone of every project; the clone's HEAD is recorded per project
in the results file (`commits_selected.head`), and clones made on different days can sit at
different upstream commits. Numbers are therefore read per machine, never as a cross-machine
comparison of one commit.

A project pinned to an older toolchain through `rust-toolchain` builds with that toolchain in
every arm (the file records the version per project in `project_rustc_version`); three corpus
projects do (Rust 1.70, 1.88 and 1.90).

A project Cargo itself cannot build on a series machine leaves the corpus and another takes
its place, so the corpus stays at 100 measured projects: `minijinja` (its CLI needs a Python
development package) gave way to `textwrap` on 2026-11-01, after `askama`, `syn` and `prost`
gave way to `minijinja`, `strum` and `log` on 2026-10-30. Every replacement passes
`scripts/corpus_smoke.py` first (Cargo and Mitts agree on check, clippy and test builds).

Earlier series (0.3.9 to 0.3.15) were measured on Rust 1.97.1 and 1.98.1 and with 5 repeats;
their files were retired when this toolchain was pinned. The 0.3.16 series was complete on the
x86_64 rig and partial on the other two machines when 0.3.17 fixed five defects it had found;
every machine restarted on 0.3.17 and only the current version's files are kept. Numbers are
never compared across Mitts versions (see `docs/benchmarks-execution-plan.md`).
