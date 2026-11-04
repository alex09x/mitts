#!/usr/bin/env bash
# Mitts Build — build a GNU release against Amazon Linux 2023 glibc.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

source_root=$(cd "$1" && pwd -P)
tag=$2
target=$3
mkdir -p "$4"
output=$(cd "$4" && pwd -P)
workspace=$(pwd -P)
temp_root=$(cd "${RUNNER_TEMP:?RUNNER_TEMP is required}" && pwd -P)
bootstrap=${MITTS_CI_BIN:?MITTS_CI_BIN is required}
toolchain=$(rustc --print sysroot)
case "$target" in
  x86_64-unknown-linux-gnu) loader=/lib64/ld-linux-x86-64.so.2 ;;
  aarch64-unknown-linux-gnu) loader=/lib/ld-linux-aarch64.so.1 ;;
  *) echo "Unsupported portable target: $target" >&2; exit 1 ;;
esac
loader=$(readlink -f "$loader")
loader_name=$(basename "$loader")
host_libs=$(mktemp -d "$temp_root/mitts-bootstrap-libs.XXXXXX")
trap 'rm -rf "$host_libs"' EXIT
cp -L "$loader" "$host_libs/$loader_name"
resolved=$(ldd "$bootstrap") || { echo 'Cannot resolve bootstrap libraries' >&2; exit 1; }
if grep -q 'not found' <<< "$resolved"; then
  echo 'A bootstrap library is missing' >&2
  exit 1
fi
while IFS= read -r library; do
  [[ -n "$library" ]] || continue
  dest="$host_libs/$(basename "$library")"
  if [[ -e "$dest" ]]; then
    cmp -s "$library" "$dest" || { echo 'Conflicting bootstrap library names' >&2; exit 1; }
  else
    cp -L "$library" "$dest"
  fi
done < <(awk '$2 == "=>" && $3 ~ /^\// {print $3}' <<< "$resolved")
extra_mounts=()
for path in "$source_root" "$output"; do
  case "$path/" in
    "$workspace/"*|"$temp_root/"*) ;;
    *) extra_mounts+=(--mount "type=bind,src=$path,dst=$path") ;;
  esac
done
docker build --tag mitts-portable-release -f scripts/release/amazonlinux.Dockerfile scripts/release
docker run --rm --user "$(id -u):$(id -g)" \
  --mount "type=bind,src=$workspace,dst=$workspace" \
  --mount "type=bind,src=$temp_root,dst=$temp_root" \
  --mount "type=bind,src=$toolchain,dst=/opt/toolchain,readonly" \
  --mount "type=bind,src=$host_libs,dst=/opt/host-libs,readonly" \
  "${extra_mounts[@]}" \
  --workdir "$workspace" \
  --env "PATH=/opt/toolchain/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
  --env "RUNNER_TEMP=$temp_root" --env "SOURCE_ROOT=$source_root" --env "RELEASE_TAG=$tag" \
  --env "RELEASE_TARGET=$target" --env "OUTPUT_DIR=$output" --env "HOST_LOADER=$loader_name" \
  --env "BOOTSTRAP=$bootstrap" --env "MITTS_CI_CACHE_DIR=${MITTS_CI_CACHE_DIR:?}" \
  mitts-portable-release bash -eu -c '
    export HOME="$RUNNER_TEMP/mitts-portable-home"
    mkdir -p "$HOME"
    wrapper="$RUNNER_TEMP/mitts-portable-bootstrap"
    printf '\''#!/bin/sh\nexec "/opt/host-libs/$HOST_LOADER" --library-path /opt/host-libs "$BOOTSTRAP" "$@"\n'\'' > "$wrapper"
    chmod 755 "$wrapper"
    export MITTS_CI_BIN="$wrapper"
    bash scripts/release/package.sh "$SOURCE_ROOT" "$RELEASE_TAG" "$RELEASE_TARGET" "$OUTPUT_DIR"
    python3 scripts/release/test_installer.py --archive "$OUTPUT_DIR/mitts-$RELEASE_TAG-$RELEASE_TARGET.tar.gz" "$RELEASE_TAG"
  '
