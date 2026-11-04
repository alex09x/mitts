#!/usr/bin/env bash
# Mitts Build — per-target isolation probe using Mitts as the only Rust runner.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0

set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd) || exit 2
mitts_bin=${MITTS_CI_BIN:-mitts}
real_home=${HOME:?HOME must be set}
cargo_home=${CARGO_HOME:-$real_home/.cargo}
rustup_home=${RUSTUP_HOME:-$real_home/.rustup}
controller=$(mktemp -d "${TMPDIR:-/tmp}/mitts-probe.XXXXXX") || exit 2
active_sandbox=""

remove_temporary_tree() {
  local root=$1
  [ -d "$root" ] || return 0
  # CAS directories may be read-only. Never chmod files: some can be hardlinks.
  find "$root" -type d -exec chmod u+w {} +
  rm -rf "$root"
}

cleanup() {
  [ -z "$active_sandbox" ] || remove_temporary_tree "$active_sandbox"
  remove_temporary_tree "$controller"
}
trap cleanup EXIT

drop=()
while IFS='=' read -r name _; do
  case "$name" in
    MITTS_*) drop+=(-u "$name") ;;
  esac
done < <(env)

scan_default_store() {
  local home_dir=$1 xdg_dir=$2 path found=""
  for path in "$home_dir/.cache/mitts" "$xdg_dir/mitts"; do
    if [ -e "$path" ]; then found="$found ${path##*/}"; fi
  done
  printf '%s' "$found"
}

controller_home="$controller/home"
controller_xdg="$controller/xdg"
controller_cache="$controller/cache"
mkdir -p "$controller_home" "$controller_xdg" "$controller_cache" || exit 2
mkdir -p "$controller/guard"
cat > "$controller/guard/cargo" <<'EOF'
#!/bin/sh
echo 'Cargo CLI is forbidden in Mitts test workflows' >&2
exit 126
EOF
chmod 755 "$controller/guard/cargo"

if ! env ${drop[@]+"${drop[@]}"} \
  HOME="$controller_home" \
  XDG_CACHE_HOME="$controller_xdg" \
  CARGO_HOME="$cargo_home" \
  RUSTUP_HOME="$rustup_home" \
  PATH="$controller/guard:$PATH" CARGO="$mitts_bin" \
  RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
  CARGO_BUILD_RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER= \
  "$mitts_bin" metadata --format-version 1 --manifest-path "$repo/Cargo.toml" \
    --locked --cache-dir "$controller_cache" > "$controller/metadata.json" 2> "$controller/metadata.log"; then
  echo "probe_store_isolation: Mitts metadata failed" >&2
  tail -20 "$controller/metadata.log" >&2
  exit 2
fi

controller_touched=$(scan_default_store "$controller_home" "$controller_xdg")
if [ -n "$controller_touched" ]; then
  echo "probe_store_isolation: metadata touched a default store:$controller_touched" >&2
  exit 1
fi

test_names=()
test_count=0
if [ "$#" -gt 0 ]; then
  test_names=("$@")
else
  # This repository auto-discovers each top-level tests/*.rs as an integration target.
  # A non-empty but incomplete metadata response must not silently skip targets.
  python3 - "$controller/metadata.json" "$repo/Cargo.toml" <<'PY'
import json
import pathlib
import sys

manifest = pathlib.Path(sys.argv[2]).resolve()
metadata = json.loads(pathlib.Path(sys.argv[1]).read_text())
package = next((p for p in metadata.get("packages", [])
                if pathlib.Path(p.get("manifest_path", "")).resolve() == manifest), None)
names = {t["name"] for t in (package or {}).get("targets", []) if "test" in t.get("kind", [])}
expected = {p.stem for p in (manifest.parent / "tests").glob("*.rs")}
missing = sorted(expected - names)
if missing:
    print("probe_store_isolation: metadata omitted integration test targets: " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)
PY
  while IFS= read -r name; do test_names+=("$name"); test_count=$((test_count + 1)); done < <(
    python3 - "$controller/metadata.json" "$repo/Cargo.toml" <<'PY'
import json
import pathlib
import sys

metadata = json.loads(pathlib.Path(sys.argv[1]).read_text())
root_manifest = pathlib.Path(sys.argv[2]).resolve()
for package in metadata.get("packages", []):
    manifest = pathlib.Path(package.get("manifest_path", "")).resolve()
    if manifest != root_manifest:
        continue
    for target in package.get("targets", []):
        if "test" in target.get("kind", []):
            print(target["name"])
PY
  )
fi

if [ "$#" -eq 0 ] && [ "$test_count" -eq 0 ] && [ -d "$repo/tests" ] \
   && find "$repo/tests" -type f -name '*.rs' -print -quit | grep -q .; then
  echo 'probe_store_isolation: metadata omitted integration test targets' >&2
  exit 1
fi

# MITTS_PROBE_SHARD=INDEX/COUNT (1-based) runs a deterministic slice of the targets so CI can
# spread the probe over parallel jobs. Targets are sorted, then dealt round-robin; shard 1
# also runs the library and binary unit tests. Every shard keeps the metadata completeness
# checks above, so a missing target still fails all of them.
shard_index=1
shard_count=1
if [ -n "${MITTS_PROBE_SHARD:-}" ]; then
  case "$MITTS_PROBE_SHARD" in
    *[!0-9/]* | */*/* | /* | */) shard_valid=0 ;;
    */*) shard_valid=1 ;;
    *) shard_valid=0 ;;
  esac
  if [ "$shard_valid" -ne 1 ]; then
    echo "probe_store_isolation: MITTS_PROBE_SHARD must look like INDEX/COUNT, got '$MITTS_PROBE_SHARD'" >&2
    exit 2
  fi
  shard_index=$((10#${MITTS_PROBE_SHARD%/*}))
  shard_count=$((10#${MITTS_PROBE_SHARD#*/}))
  if [ "$shard_count" -lt 1 ] || [ "$shard_index" -lt 1 ] || [ "$shard_index" -gt "$shard_count" ]; then
    echo "probe_store_isolation: MITTS_PROBE_SHARD index must be between 1 and the shard count, got '$MITTS_PROBE_SHARD'" >&2
    exit 2
  fi
fi
if [ "$shard_count" -gt 1 ]; then
  all_names=(${test_names[@]+"${test_names[@]}"})
  test_names=()
  position=0
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    if [ $((position % shard_count)) -eq $((shard_index - 1)) ]; then test_names+=("$name"); fi
    position=$((position + 1))
  done < <(printf '%s\n' ${all_names[@]+"${all_names[@]}"} | LC_ALL=C sort)
fi
echo "probe_store_isolation: shard $shard_index/$shard_count runs ${#test_names[@]} integration target(s)"

writers=0
failures=0

run_target() {
  local label=$1 rc touched
  shift
  local sandbox
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/mitts-probe-target.XXXXXX") || exit 2
  active_sandbox=$sandbox
  mkdir -p "$sandbox/home" "$sandbox/xdg" "$sandbox/target"
  rc=0
  env ${drop[@]+"${drop[@]}"} \
    HOME="$sandbox/home" \
    XDG_CACHE_HOME="$sandbox/xdg" \
    CARGO_HOME="$cargo_home" \
    RUSTUP_HOME="$rustup_home" \
    PATH="$controller/guard:$PATH" CARGO="$mitts_bin" \
    RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
    CARGO_BUILD_RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER= \
    RUST_TEST_THREADS=1 \
    "$mitts_bin" test --manifest-path "$repo/Cargo.toml" --workspace --locked \
      --target-dir "$sandbox/target" --cache-dir "$controller_cache" "$@" \
      > "$sandbox/out.log" 2>&1 || rc=$?
  touched=$(scan_default_store "$sandbox/home" "$sandbox/xdg")
  if [ "$rc" -ne 0 ]; then
    echo "FAIL $label rc=$rc"
    tail -100 "$sandbox/out.log"
    failures=$((failures + 1))
  elif [ -n "$touched" ]; then
    echo "WRITES $label ->$touched"
    writers=$((writers + 1))
  else
    echo "clean  $label"
  fi
  remove_temporary_tree "$sandbox"
  active_sandbox=""
}

if [ "$shard_index" -eq 1 ]; then run_target "library and binary unit tests" --lib --bins; fi
for name in ${test_names[@]+"${test_names[@]}"}; do run_target "$name" --test "$name"; done

echo "probe_store_isolation: $writers target(s) wrote to the default store, $failures failed"
[ "$writers" -eq 0 ] && [ "$failures" -eq 0 ]
