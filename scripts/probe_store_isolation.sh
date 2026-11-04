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

cleanup() {
  [ -z "$active_sandbox" ] || rm -rf "$active_sandbox"
  rm -rf "$controller"
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

if ! env ${drop[@]+"${drop[@]}"} \
  HOME="$controller_home" \
  XDG_CACHE_HOME="$controller_xdg" \
  CARGO_HOME="$cargo_home" \
  RUSTUP_HOME="$rustup_home" \
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

writers=0
failures=0

run_target() {
  local label=$1 rc touched
  shift
  local sandbox
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/mitts-probe-target.XXXXXX") || exit 2
  active_sandbox=$sandbox
  mkdir -p "$sandbox/home" "$sandbox/xdg" "$sandbox/target" "$sandbox/cache"
  rc=0
  env ${drop[@]+"${drop[@]}"} \
    HOME="$sandbox/home" \
    XDG_CACHE_HOME="$sandbox/xdg" \
    CARGO_HOME="$cargo_home" \
    RUSTUP_HOME="$rustup_home" \
    RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
    CARGO_BUILD_RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER= \
    RUST_TEST_THREADS=1 \
    "$mitts_bin" test --manifest-path "$repo/Cargo.toml" --workspace --locked \
      --target-dir "$sandbox/target" --cache-dir "$sandbox/cache" "$@" \
      > "$sandbox/out.log" 2>&1 || rc=$?
  touched=$(scan_default_store "$sandbox/home" "$sandbox/xdg")
  if [ "$rc" -ne 0 ]; then
    echo "FAIL $label rc=$rc"
    tail -20 "$sandbox/out.log"
    failures=$((failures + 1))
  elif [ -n "$touched" ]; then
    echo "WRITES $label ->$touched"
    writers=$((writers + 1))
  else
    echo "clean  $label"
  fi
  rm -rf "$sandbox"
  active_sandbox=""
}

run_target "library and binary unit tests" --lib --bins
for name in "${test_names[@]}"; do run_target "$name" --test "$name"; done

echo "probe_store_isolation: $writers target(s) wrote to the default store, $failures failed"
[ "$writers" -eq 0 ] && [ "$failures" -eq 0 ]
