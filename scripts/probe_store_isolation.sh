#!/usr/bin/env bash
#
# Mitts Build — Rust-first build and package tooling with shared artifact cache
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Finds test targets that reach the default Mitts store instead of an isolated one.
#
# Every test target (unit tests, then each file under tests/) runs on its own with
# a throwaway HOME and XDG_CACHE_HOME and no inherited MITTS_* variables. A target
# that leaves a Mitts store under that throwaway home resolved the default cache
# root somewhere, so it is not isolated. The real Cargo and rustup homes are kept.
#
# Usage: scripts/probe_store_isolation.sh [test-name ...]
# Exit status: 1 when at least one target wrote to the default store.
set -uo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
real_home=${HOME:?HOME must be set}
cd "$repo"

drop=()
while IFS='=' read -r name _; do
  case $name in MITTS_*) drop+=("-u" "$name") ;; esac
done < <(env)

run_target() { # label, cargo test target args...
  local label=$1; shift
  local sandbox; sandbox=$(mktemp -d "${TMPDIR:-/tmp}/mitts-probe.XXXXXX")
  env "${drop[@]}" HOME="$sandbox" XDG_CACHE_HOME="$sandbox/xdg" \
    CARGO_HOME="${CARGO_HOME:-$real_home/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$real_home/.rustup}" \
    cargo test --workspace "$@" > "$sandbox/out.log" 2>&1
  local rc=$?
  local touched=""
  for d in "$sandbox/.cache/mitts" "$sandbox/xdg/mitts"; do
    [ -d "$d" ] || continue
    touched="$touched ${d#"$sandbox"/}[$(ls "$d" | tr '\n' ',')]"
  done
  if [ $rc -ne 0 ]; then
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
}

writers=0
failures=0
cargo test --workspace --no-run > /dev/null 2>&1 || { echo "build failed"; exit 2; }
if [ $# -gt 0 ]; then
  for name in "$@"; do run_target "$name" --test "$name"; done
else
  run_target "unit tests" --lib --bins
  for f in tests/*.rs; do
    name=$(basename "$f" .rs)
    run_target "$name" --test "$name"
  done
fi
echo "probe_store_isolation: $writers target(s) wrote to the default store, $failures failed"
[ $writers -eq 0 ] && [ $failures -eq 0 ]
