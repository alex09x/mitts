# Cache size, cleanup and compiler wrappers

## A size limit that needs no `mitts gc`

The shared cache (`~/.cache/mitts`, or `--cache-dir`) is bounded. After a command that works
with the cache (`build`, `check`, `clippy`, `test`, `bench`, `coverage`, `llvm-cov`, `run`,
`rustc`), Mitts looks at the size of the content-addressed store and, when it is over the limit,
prunes the oldest objects until it is under it. Objects of a build that is running are not
pruned: the pruning waits until no build holds the cache.

| Variable | Meaning | Default |
| --- | --- | --- |
| `MITTS_CACHE_MAX_BYTES` | the limit: a number of bytes with an optional `K`, `M`, `G` or `T` suffix (binary multiples, `500M`, `2G`), or `off` / `0` for no limit | 100 GiB |
| `MITTS_GC_INTERVAL_SECS` | the least time between two checks (the check scans the store) | 3600 |

A value that is not a size keeps the default limit. When something is pruned, one line on
stderr says how much (`[mitts::gc] the cache was over its limit ...`). Pruning goes by file
modification time, oldest first, not by last use; a pruned object is compiled again when
needed, which costs time and nothing else. `mitts gc --max-bytes N` still prunes on demand.

## Uncached builds do not use the shared store

A build without the cache (`--no-cache`, or a compiler wrapper, below) keeps its outputs in a
private store `<cache>/cas-ephemeral-<id>` of its own, removed by the next build or by
`mitts gc` after its process has exited. Nothing of it is added to the shared store.

## Compiler wrappers

A `rustc-wrapper` in the cargo config (or `RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WRAPPER`,
`RUSTC_WORKSPACE_WRAPPER`, `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER`) is an input Mitts cannot know:
it may change what the compiler produces or depend on external state. By default, a build with a
wrapper is therefore not cached, stages its outputs in a private `cas-ephemeral-<id>` store, and
prints a warning that names the wrapper and says how to clear it (set the variables to an empty
value, or remove the key from the config).

`sccache` is the exception. It only caches compiler results and does not change them, and many
users set it globally for every project. Mitts leaves it out: it compiles without it, and the
units are reused from the Mitts cache.

### Explicit opt-in for deterministic or analysis wrappers

Pass-through, telemetry, or static-analysis wrappers that are deterministic functions of the
`rustc` invocation can opt into a wrapper-scoped cache namespace:

| Variable | Meaning | Default |
| --- | --- | --- |
| `MITTS_RUSTC_WRAPPER_CACHE` | set to `1` or `true` to cache wrapped units and build scripts in a namespace keyed by the wrapper executable's resolved path and SHA-256 content hash | off (`0`) |
| `MITTS_RUSTC_WRAPPER_SALT` | optional caller-provided string mixed into wrapper-scoped keys when the wrapper's behavior depends on external config, schema version, or environment state | unset |
| `MITTS_RUSTC_WRAPPER_OUTPUTS` | directory where the wrapper's own extra outputs belong (see below) | unset |

When `MITTS_RUSTC_WRAPPER_CACHE=1` is set:
- Every active wrapper executable must resolve on disk (`PATH` or file path) and is hashed by
  content; editing or replacing the wrapper binary invalidates its namespace automatically.
- Wrapped cache entries never satisfy an unwrapped build, and unwrapped cache entries never
  satisfy a wrapped build.
- Without `MITTS_RUSTC_WRAPPER_CACHE=1`, the default uncached behavior and warning are unchanged.

#### Extra outputs of a wrapper

A cache hit does not run the wrapper, so files the wrapper writes besides rustc's outputs would
be missing after a hit. Declare where they belong with `MITTS_RUSTC_WRAPPER_OUTPUTS=<dir>`
(together with `MITTS_RUSTC_WRAPPER_CACHE=1`):
- For each compiled unit, Mitts sets `MITTS_WRAPPER_OUT_DIR` to a private, empty directory.
  The wrapper writes its extra files there, in any relative layout (for example
  `<crate>.idx` and `.deps/<crate>.json`).
- After a successful compile, Mitts stores those files with the unit and places them at
  `<dir>/<relative path>`. A cache hit restores them to the same place without running the
  wrapper.
- The declaration is part of the key: entries made without it never satisfy a build with it.
  If the files cannot be stored, the unit is not cached.
