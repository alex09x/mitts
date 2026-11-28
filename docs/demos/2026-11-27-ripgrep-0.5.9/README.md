# Demo: ripgrep 14.1.1 on Mitts 0.5.9, first build, no-op rebuild, second worktree

One recording run on 2026-11-27 22:40 UTC. It shows what happens; it is not a benchmark.

- Mitts 0.5.9 (ff04c5b), rustc 1.99.1
- Apple M2 Ultra, 24 cores, 128 GB, macOS 15.7.8, APFS
- ripgrep 14.1.1 (4649aa9), dev profile, empty Mitts cache, sources fetched first
- Normal terminal priority (31); other programs on the machine were not stopped

| Step | Wall | Units |
|---|---|---|
| `mitts fetch` (not part of the build) | 1.3 s | 51 packages |
| 1. first build, empty cache | 6.5 s | 33 compiled |
| 2. same build again, nothing changed | 0.016 s | no-op |
| 3. second worktree of the same commit, empty `target/` | 5.5 s | 9 compiled, 24 from the cache |

The 9 compiled in step 3 are ripgrep's own crates, rebuilt because since 0.5.8 workspace
crates are not shared across checkouts unless `MITTS_SHARE_WORKSPACE_CRATES=1`; the 24
restored are its dependencies.

A dry run with the same script went first in another folder (numbers within 0.4–1.1 s);
it is not used because its output carried long temporary paths.

## Files

- `demo.sh` — the script that was run (`script -r rec.script bash demo.sh` in Terminal.app).
- `rec.script` — the terminal recording with timing; replay on macOS: `script -p rec.script`.
- `transcript-full.txt` — the recording as text, progress frames included.
- `transcript.txt` — the same without progress frames.

## Repeat it

```sh
git clone --depth 1 --branch 14.1.1 https://github.com/BurntSushi/ripgrep.git rg-a
cd rg-a
export MITTS_CACHE_DIR=$PWD/../cache   # a fresh, empty cache
mitts fetch
mitts build                            # 1. first build
mitts build                            # 2. nothing changed
git worktree add --detach ../rg-b HEAD && cd ../rg-b
mitts build                            # 3. second worktree
```
