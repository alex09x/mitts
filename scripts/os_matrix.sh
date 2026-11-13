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
# Runs the Mitts test suite on several Linux distributions and architectures in
# containers, one container per entry, and prints a pass/fail table.
#
# Usage:
#   scripts/os_matrix.sh                     # the default matrix
#   scripts/os_matrix.sh ubuntu:24.04 fedora:45@linux/arm64
#   OS_MATRIX_MODE=smoke scripts/os_matrix.sh   # a few end-to-end tests only
#
# An entry is IMAGE[@PLATFORM]. Native-platform entries run the full suite
# (scripts/run_tests_isolated.sh); emulated ones (qemu binfmt, e.g. linux/arm64
# or linux/riscv64 on x86_64) run the smoke set unless OS_MATRIX_MODE=full,
# because the full suite under emulation takes hours.
#
# Each container needs a Mitts binary for its architecture to drive the tests
# (Mitts is the only test driver): OS_MATRIX_MITTS_<ARCH> (x86_64, aarch64,
# riscv64) points at one; otherwise the released binary for x86_64/aarch64 is
# downloaded from GitHub. The checkout is copied into the container read-only,
# so nothing in the working tree changes. rustup data is kept in one Docker
# volume per image so repeated runs do not download toolchains again.
#
# Logs: $OS_MATRIX_OUT (default target/os-matrix/<timestamp>/<entry>.log).
set -euo pipefail

# `wait -n` throttles the parallel entries; without it every container would start at once.
if ((BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 3))); then
  echo "os_matrix: needs bash 4.3 or newer (this is $BASH_VERSION)" >&2
  exit 2
fi

repo=$(cd "$(dirname "$0")/.." && pwd)
host_arch=$(uname -m)
mode=${OS_MATRIX_MODE:-auto}
jobs=${OS_MATRIX_JOBS:-4}
out=${OS_MATRIX_OUT:-$repo/target/os-matrix/$(date +%Y%m%d-%H%M%S)}
smoke_tests=${OS_MATRIX_SMOKE:-"--test cli_dependency_features_test --test cli_native_features --test m8_jobserver_pool_test --test verbose_toolchain_summary_test --test slow_units_test"}

default_matrix=(
  ubuntu:22.04
  ubuntu:24.04
  debian:12
  debian:13
  fedora:45
  rockylinux/rockylinux:9
  opensuse/tumbleweed
  archlinux:latest
  ubuntu:24.04@linux/arm64
  fedora:45@linux/arm64
)
if [ $# -gt 0 ]; then entries=("$@"); else entries=("${default_matrix[@]}"); fi

mkdir -p "$out/bin"
version=$(awk -F'"' '/^version = /{print $2; exit}' "$repo/Cargo.toml")

# A Mitts binary for one architecture, as a path on the host.
mitts_for() {
  local arch=$1 var="OS_MATRIX_MITTS_$1"
  if [ -n "${!var:-}" ]; then echo "${!var}"; return; fi
  local bin="$out/bin/mitts-$arch"
  if [ ! -x "$bin" ]; then
    case $arch in x86_64|aarch64) ;; *)
      echo "os_matrix: no released Mitts for $arch; set $var" >&2; return 1 ;;
    esac
    local release latest
    release=$(curl -fsSL https://api.github.com/repos/alex09x/mitts/releases/latest)
    latest=$(printf '%s\n' "$release" | awk -F'"' '/"tag_name"/{print $4; exit}')
    local unpack="$out/bin/$arch"
    mkdir -p "$unpack"
    curl -fsSL "https://github.com/alex09x/mitts/releases/download/$latest/mitts-$latest-$arch-unknown-linux-gnu.tar.gz" |
      tar -xz -C "$unpack" mitts
    mv "$unpack/mitts" "$bin"
  fi
  echo "$bin"
}

# The local tag an entry's image is pulled under.
local_tag() {
  echo "mitts-os-matrix:$(printf '%s' "$1" | tr '/:@.' '____')"
}

platform_arch() {
  case $1 in
    linux/amd64) echo x86_64 ;;
    linux/arm64) echo aarch64 ;;
    linux/riscv64) echo riscv64 ;;
    *) echo "os_matrix: unsupported platform $1" >&2; return 1 ;;
  esac
}

native_platform() {
  case $host_arch in
    x86_64) echo linux/amd64 ;;
    aarch64|arm64) echo linux/arm64 ;;
    riscv64) echo linux/riscv64 ;;
  esac
}

# Runs inside the container: system packages, rustup, then the suite.
read -r -d '' inner <<'INNER' || true
set -eu
if command -v apt-get >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq && apt-get install -y -qq gcc g++ make git curl ca-certificates python3 pkg-config >/dev/null
elif command -v dnf >/dev/null; then
  dnf install -y -q --allowerasing gcc gcc-c++ make git curl python3 pkgconf-pkg-config findutils tar xz >/dev/null
elif command -v zypper >/dev/null; then
  zypper -n -q install gcc gcc-c++ make git curl python3 pkg-config tar gzip xz >/dev/null
elif command -v pacman >/dev/null; then
  pacman -Sy --noconfirm --needed -q gcc make git curl python pkgconf tar >/dev/null
else
  echo "os_matrix: no supported package manager" >&2; exit 2
fi
if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --profile minimal --component rustfmt,clippy,llvm-tools >/dev/null
fi
. "$HOME/.cargo/env"
rustup -q toolchain install stable --profile minimal --component rustfmt,clippy,llvm-tools >/dev/null
# A second toolchain for the `mitts +nightly` tests.
rustup -q toolchain install nightly --profile minimal >/dev/null
mkdir -p /work && tar -C /src --exclude=./target -cf - . | tar -C /work -xf -
cd /work
echo "os: $(. /etc/os-release; echo "$PRETTY_NAME") $(uname -m), $(rustc --version), $(/mitts --version)"
export MITTS_CI_BIN=/mitts
if [ "$MATRIX_MODE" = smoke ]; then
  # shellcheck disable=SC2086
  scripts/run_tests_isolated.sh $MATRIX_SMOKE
else
  scripts/run_tests_isolated.sh --no-fail-fast
fi
INNER

run_entry() {
  local entry=$1 image platform arch entry_mode name bin volume status
  image=${entry%@*}
  platform=$(native_platform)
  [ "$entry" != "$image" ] && platform=${entry#*@}
  arch=$(platform_arch "$platform")
  entry_mode=$mode
  if [ "$entry_mode" = auto ]; then
    if [ "$platform" = "$(native_platform)" ]; then entry_mode=full; else entry_mode=smoke; fi
  fi
  name=$(printf '%s' "$entry" | tr '/:@' '___')
  volume="mitts-os-matrix-$(printf '%s-%s' "$image" "$arch" | tr '/:.' '___')"
  bin=$(mitts_for "$arch") || { echo "$entry $entry_mode SKIP (no Mitts binary for $arch)" > "$out/$name.result"; return; }
  status=0
  docker run --rm --platform "$platform" \
    -v "$repo:/src:ro" -v "$bin:/mitts:ro" -v "$volume:/root" \
    -e HOME=/root -e MATRIX_MODE="$entry_mode" -e MATRIX_SMOKE="$smoke_tests" \
    "$(local_tag "$entry")" bash -c "$inner" > "$out/$name.log" 2>&1 || status=$?
  local verdict=PASS
  [ $status -eq 0 ] || verdict="FAIL (exit $status)"
  echo "$entry $entry_mode $verdict" > "$out/$name.result"
}

echo "os_matrix: Mitts $version, ${#entries[@]} entries, logs in $out"
# Before the entries run in parallel: fetch each architecture's driver once, and pull
# each image for its platform under a tag of its own (two platforms of one image share
# a tag otherwise, and the last pull wins).
for entry in "${entries[@]}"; do
  platform=$(native_platform)
  [ "$entry" != "${entry%@*}" ] && platform=${entry#*@}
  mitts_for "$(platform_arch "$platform")" >/dev/null || true
  # A locally imported image (no registry to pull from) is used as it is.
  if docker pull -q --platform "$platform" "${entry%@*}" >/dev/null 2>&1 ||
    docker image inspect "${entry%@*}" >/dev/null 2>&1; then
    docker tag "${entry%@*}" "$(local_tag "$entry")"
  else
    echo "os_matrix: cannot pull ${entry%@*} for $platform" >&2
  fi
done
running=0
for entry in "${entries[@]}"; do
  run_entry "$entry" &
  running=$((running + 1))
  if [ $running -ge "$jobs" ]; then wait -n || true; running=$((running - 1)); fi
done
wait || true

failed=0
printf '\n%-32s %-6s %s\n' ENTRY MODE RESULT
for entry in "${entries[@]}"; do
  name=$(printf '%s' "$entry" | tr '/:@' '___')
  read -r e m r < "$out/$name.result" || { e=$entry; m=-; r="NO RESULT"; }
  printf '%-32s %-6s %s\n' "$e" "$m" "$r"
  case $r in PASS) ;; *) failed=1 ;; esac
done
exit $failed
