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

A `rustc-wrapper` in the cargo config (or `RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WRAPPER`) is an
input Mitts cannot know: it may change what the compiler produces. A build with a wrapper is
therefore not cached, and Mitts prints a warning that names the wrapper and says how to clear
it (set the variables to an empty value, or remove the key from the config).

`sccache` is the exception. It only caches compiler results and does not change them, and many
users set it globally for every project. Mitts leaves it out: it compiles without it, and the
units are reused from the Mitts cache.
