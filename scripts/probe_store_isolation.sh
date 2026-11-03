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
# Cargo compiles all test executables once under one disposable controller HOME
# and XDG_CACHE_HOME with inherited MITTS_* variables dropped and the real Cargo
# and rustup homes preserved. A Cargo target.<host>.runner wrapper passed via
# --config then gives every actual test executable its own fresh HOME and
# XDG_CACHE_HOME and strips all MITTS_* variables (including MITTS_PROBE_SANDBOX)
# before exec.
#
# Usage: scripts/probe_store_isolation.sh [test-name ...]
# Exit status: 1 when any target or controller phase wrote to the default store
#              or when any target failed; 2 when setup or prebuild failed.
set -uo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd) || exit 2
real_home=${HOME:?HOME must be set}
cargo_home=${CARGO_HOME:-$real_home/.cargo}
rustup_home=${RUSTUP_HOME:-$real_home/.rustup}
mkdir -p "${TMPDIR:-/tmp}" || exit 2
cd "$repo" || exit 2

drop=()
while IFS='=' read -r name _; do
  case $name in MITTS_*) drop+=("-u" "$name") ;; esac
done < <(env)

controller=$(mktemp -d "${TMPDIR:-/tmp}/mitts-probe-ctl.XXXXXX") || exit 2
active_sandbox=""
cleanup() {
  [ -n "$active_sandbox" ] && rm -rf "$active_sandbox"
  rm -rf "$controller"
}
trap cleanup EXIT

controller_home="$controller/home"
controller_xdg="$controller/xdg"
runner="$controller/runner.sh"
config="$controller/config.toml"
mkdir -p "$controller_home" || exit 2

rustc_info=$(env ${drop[@]+"${drop[@]}"} \
  HOME="$controller_home" \
  XDG_CACHE_HOME="$controller_xdg" \
  CARGO_HOME="$cargo_home" \
  RUSTUP_HOME="$rustup_home" \
  rustc -vV) || { echo "probe_store_isolation: rustc -vV failed" >&2; exit 2; }
host=$(printf '%s\n' "$rustc_info" | sed -n 's/^host: //p')
if [ -z "$host" ]; then
  echo "probe_store_isolation: missing host in rustc -vV" >&2
  exit 2
fi

cat > "$runner" <<'EOF' || exit 2
#!/bin/bash
set -euo pipefail

sandbox=${MITTS_PROBE_SANDBOX:?MITTS_PROBE_SANDBOX must be set}
if [ $# -eq 0 ]; then
  echo "mitts probe runner: missing test executable" >&2
  exit 2
fi

exec_home=$(mktemp -d "$sandbox/exec.XXXXXX")
drop=()
while IFS='=' read -r name _; do
  case $name in MITTS_*) drop+=("-u" "$name") ;; esac
done < <(env)

exec env ${drop[@]+"${drop[@]}"} \
  HOME="$exec_home" \
  XDG_CACHE_HOME="$exec_home/xdg" \
  "$@"
EOF
chmod 755 "$runner" || exit 2

python3 -c '
import json, sys
host, *runner = sys.argv[1:]
print("[target." + json.dumps(host) + "]")
print("runner = " + json.dumps(runner))
' "$host" /bin/bash "$runner" > "$config" || {
  echo "probe_store_isolation: failed to write cargo runner config" >&2
  exit 2
}

scan_mitts_dirs() { # base_dir, home_dir, xdg_dir
  local base=$1 home_dir=$2 xdg_dir=$3 d rel entries out=""
  for d in "$home_dir/.cache/mitts" "$xdg_dir/mitts"; do
    [ -d "$d" ] || continue
    rel=${d#"$base"/}
    entries=$(ls "$d" | tr '\n' ',')
    out="$out $rel[$entries]"
  done
  printf '%s' "$out"
}

run_target() { # label, cargo test target args...
  local label=$1; shift
  local sandbox rc touched="" ran=0 exec_dir found
  sandbox=$(mktemp -d "${TMPDIR:-/tmp}/mitts-probe.XXXXXX") || exit 2
  active_sandbox=$sandbox
  env ${drop[@]+"${drop[@]}"} \
    HOME="$controller_home" \
    XDG_CACHE_HOME="$controller_xdg" \
    CARGO_HOME="$cargo_home" \
    RUSTUP_HOME="$rustup_home" \
    MITTS_PROBE_SANDBOX="$sandbox" \
    cargo --config "$config" test --workspace "$@" > "$sandbox/out.log" 2>&1
  rc=$?
  for exec_dir in "$sandbox"/exec.*; do
    [ -d "$exec_dir" ] || continue
    ran=$((ran + 1))
    found=$(scan_mitts_dirs "$exec_dir" "$exec_dir" "$exec_dir/xdg")
    touched="$touched$found"
  done
  if [ $rc -ne 0 ]; then
    echo "FAIL $label rc=$rc"
    tail -20 "$sandbox/out.log"
    failures=$((failures + 1))
  elif [ $ran -eq 0 ]; then
    echo "FAIL $label (runner did not execute)"
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

writers=0
failures=0
prebuild_targets=()
for probe_target in "$@"; do prebuild_targets+=("--test" "$probe_target"); done
env ${drop[@]+"${drop[@]}"} \
  HOME="$controller_home" \
  XDG_CACHE_HOME="$controller_xdg" \
  CARGO_HOME="$cargo_home" \
  RUSTUP_HOME="$rustup_home" \
  cargo --config "$config" test --workspace ${prebuild_targets[@]+"${prebuild_targets[@]}"} --no-run > "$controller/build.log" 2>&1 || {
  echo "build failed"
  tail -20 "$controller/build.log"
  exit 2
}

ctl_touched=$(scan_mitts_dirs "$controller" "$controller_home" "$controller_xdg")
if [ -n "$ctl_touched" ]; then
  echo "WRITES controller ->$ctl_touched"
  echo "probe_store_isolation: controller build wrote to the default store"
  exit 1
fi

if [ $# -gt 0 ]; then
  for name in "$@"; do run_target "$name" --test "$name"; done
else
  run_target "unit tests" --lib --bins
  for f in tests/*.rs; do
    [ -e "$f" ] || continue
    name=$(basename "$f" .rs)
    run_target "$name" --test "$name"
  done
fi

ctl_touched=$(scan_mitts_dirs "$controller" "$controller_home" "$controller_xdg")
if [ -n "$ctl_touched" ]; then
  echo "WRITES controller ->$ctl_touched"
  writers=$((writers + 1))
fi

echo "probe_store_isolation: $writers target(s) wrote to the default store, $failures failed"
[ $writers -eq 0 ] && [ $failures -eq 0 ]
