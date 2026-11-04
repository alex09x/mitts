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
# Runs the Mitts test suite in a clean room.
#
# Every test that builds is expected to use its own cache root; this runner is
# the guard that proves no test reached the default store, which resolves to
# $MITTS_CACHE_DIR, $XDG_CACHE_HOME/mitts or ~/.cache/mitts. It gives the whole
# run a throwaway HOME and XDG_CACHE_HOME, drops inherited MITTS_* variables,
# keeps the real Cargo and rustup homes so no sources are re-downloaded, reports
# which stores (if any) still appeared under the throwaway home, and fails if
# anything in the real default store changed during the run.
# scripts/probe_store_isolation.sh finds the offending target when one does.
#
# Usage:
#   scripts/run_tests_isolated.sh [mitts test arguments...]
#   MITTS_TEST_COMMAND=coverage scripts/run_tests_isolated.sh --summary-only
#
# Mitts is the only supported Rust test driver in this repository.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
real_home=${HOME:?HOME must be set}
real_store=${XDG_CACHE_HOME:-$real_home/.cache}/mitts
sandbox=$(mktemp -d "${TMPDIR:-/tmp}/mitts-test-home.XXXXXX")
marker=$sandbox/.started
trap 'rm -rf "$sandbox"' EXIT
touch "$marker"
cd "$repo"
if [ "${MITTS_TEST_DRIVER:-mitts}" != mitts ]; then
  echo 'run_tests_isolated: only the Mitts test driver is supported' >&2
  exit 2
fi
lock_before=$(python3 -c 'import hashlib; print(hashlib.sha256(open("Cargo.lock","rb").read()).hexdigest())')
command=("${MITTS_CI_BIN:-mitts}" "${MITTS_TEST_COMMAND:-test}" --workspace --cache-dir "${MITTS_CI_CACHE_DIR:-$repo/target/mitts-ci-cache}")

drop=()
while IFS='=' read -r name _; do
  case $name in MITTS_*) drop+=("-u" "$name") ;; esac
done < <(env)

cd "$repo"
status=0
env ${drop[@]+"${drop[@]}"} \
  HOME="$sandbox" \
  XDG_CACHE_HOME="$sandbox/xdg" \
  CARGO_HOME="${CARGO_HOME:-$real_home/.cargo}" \
  RUSTUP_HOME="${RUSTUP_HOME:-$real_home/.rustup}" \
  RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= \
  CARGO_BUILD_RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER= \
  "${command[@]}" "$@" || status=$?
lock_after=$(python3 -c 'import hashlib; print(hashlib.sha256(open("Cargo.lock","rb").read()).hexdigest())')
if [ "$lock_before" != "$lock_after" ]; then
  echo 'run_tests_isolated: FAIL: Mitts changed Cargo.lock' >&2
  status=1
fi

# Entries of the sandbox store tell which stores the suite still reaches by
# default; they are informational, the sandbox is discarded on exit.
used=()
for d in "$sandbox/.cache/mitts" "$sandbox/xdg/mitts"; do
  [ -d "$d" ] || continue
  for e in "$d"/*; do [ -e "$e" ] && used+=("${d#"$sandbox"/}/${e##*/}"); done
done
if [ ${#used[@]} -gt 0 ]; then
  echo "run_tests_isolated: suite used the sandbox store: ${used[*]}"
fi

# The real default store must be untouched: nothing in its first two levels
# may be newer than the marker written before the run.
changed=()
if [ -d "$real_store" ]; then
  for e in "$real_store" "$real_store"/* "$real_store"/*/*; do
    [ -e "$e" ] && [ "$e" -nt "$marker" ] && changed+=("$e")
  done
fi
if [ ${#changed[@]} -gt 0 ]; then
  echo "run_tests_isolated: FAIL: the real store $real_store changed during the run:" >&2
  printf '  %s\n' "${changed[@]}" >&2
  exit 1
fi
echo "run_tests_isolated: real store $real_store untouched"
exit "$status"
