# Mitts in GitHub Actions

Two small composite actions install a verified Mitts release and keep its build cache between runs.

```yaml
steps:
  - uses: actions/checkout@v4
  - uses: dtolnay/rust-toolchain@stable
  - uses: alex09x/mitts@v0.3.23
  - run: mitts test
  - uses: alex09x/mitts/save@v0.3.23
    if: always()
```

Pin the action to a release tag or to a full commit SHA. The setup action installs the release you ask for with the installer that ships in the same ref, and the installer checks the archive against the published SHA-256 sums before it installs.

## What gets cached

Mitts keeps its own content-addressed store and action cache in one directory (by default `~/.cache/mitts`). That directory is the whole cache: Mitts does not need the Cargo registry or `target/` to be restored, so a Cargo-oriented cache action only adds weight.

The cache key is `mitts-v1`, the runner OS and architecture, the installed Mitts version, your `cache-key` text, and a hash of every `Cargo.lock` in the repository. On a miss the newest cache with the same prefix, without the lockfile hash, is restored, so a changed dependency set only rebuilds what changed.

## setup action: `alex09x/mitts`

| Input | Default | Meaning |
|---|---|---|
| `version` | latest stable | Release to install, `vX.Y.Z` or `X.Y.Z`. |
| `cache` | `true` | Restore the cache. Set `false` to only install. |
| `cache-key` | empty | Extra text for the key, for example a job name. Use it when jobs build different things. |
| `cache-dir` | platform default | Cache root. Order when empty: `MITTS_CACHE_DIR`, `XDG_CACHE_HOME/mitts`, `~/.cache/mitts`. |

| Output | Meaning |
|---|---|
| `version` | The installed version, for example `0.3.22`. |
| `cache-dir` | The cache root used. It is also exported as `MITTS_CACHE_DIR` for the rest of the job. |
| `cache-hit` | `true` when the exact key was restored. |

## save action: `alex09x/mitts/save`

Runs `mitts gc` to bound the store, then saves it. Nothing is saved after an exact cache hit, because the cache already holds this key.

| Input | Default | Meaning |
|---|---|---|
| `max-bytes` | 2 GiB | Prune the store to at most this size before saving, oldest objects first. |

Use `if: always()` to keep the work of a failed build, which still speeds up the retry. The save action must run after the setup action in the same job.

## Supported runners

Linux x86-64 and ARM64, and macOS on Apple Silicon. Intel macOS and Windows have no prebuilt Mitts. A Rust toolchain and a native linker must already be present, as with any Rust build; `dtolnay/rust-toolchain` or the preinstalled toolchain both work.

## Limits

GitHub keeps 10 GB of caches per repository and evicts the least recently used. Lower `max-bytes` when several workflows share the repository budget.
