#!/bin/sh
# Mitts Build — install a verified GitHub Release binary.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
set -eu

fail() { printf 'mitts installer: %s\n' "$*" >&2; exit 1; }
for command in curl tar awk grep mktemp; do
    command -v "$command" >/dev/null 2>&1 || fail "Required command is missing: $command"
done
case "$(uname -s)" in
    Darwin) system=apple-darwin ;;
    Linux) system=unknown-linux-gnu ;;
    *) fail 'Supported systems: macOS and Linux.' ;;
esac
case "$(uname -m)" in
    arm64|aarch64) architecture=aarch64 ;;
    x86_64|amd64) architecture=x86_64 ;;
    *) fail 'Supported architectures: x86_64 and ARM64.' ;;
esac
target=$architecture-$system
if [ "$target" = x86_64-apple-darwin ]; then
    fail 'macOS Intel prebuilt support ended with v0.3.19. Use its archived release assets; current macOS builds require Apple Silicon.'
fi
repository=https://github.com/alex09x/mitts
download() { curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 "$1" -o "$2"; }
temporary=$(mktemp -d) || fail 'Could not create a temporary directory.'
pending=
trap 'rm -rf "$temporary"; if [ -n "$pending" ]; then rm -f "$pending"; fi' EXIT
trap 'exit 1' HUP INT TERM

version=${MITTS_VERSION:-}
if [ -z "$version" ]; then
    latest=$(curl --head --fail --location --silent --show-error --proto '=https' --tlsv1.2 \
        "$repository/releases/latest" -o /dev/null --write-out '%{url_effective}') ||
        fail 'Cannot resolve the latest stable release. Retry, or set MITTS_VERSION=vX.Y.Z.'
    case "$latest" in
        "$repository/releases/tag/"*) version=${latest##*/} ;;
        *) fail 'Latest release did not resolve to a GitHub release tag.' ;;
    esac
fi
case "$version" in v*) ;; *) version=v$version ;; esac
printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail 'Release must have a stable vX.Y.Z tag.'
asset=mitts-$version-$target.tar.gz
base=$repository/releases/download/$version
printf 'Installing Mitts %s for %s…\n' "$version" "$target"
download "$base/$asset" "$temporary/$asset" || fail "Release $version has no downloadable $target archive."
download "$base/SHA256SUMS" "$temporary/SHA256SUMS" || fail 'Cannot download release checksums.'
expected=$(awk -v name="$asset" 'NF == 2 && $2 == name {print $1}' "$temporary/SHA256SUMS")
printf '%s\n' "$expected" | grep -Eq '^[a-f0-9]{64}$' || fail 'Missing or ambiguous archive checksum.'
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$temporary/$asset" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$temporary/$asset" | awk '{print $1}')
else
    fail 'SHA-256 requires sha256sum or shasum.'
fi
[ "$actual" = "$expected" ] || fail 'Archive checksum mismatch; existing installation was preserved.'
tar -xzf "$temporary/$asset" -C "$temporary" mitts || fail 'Archive does not contain mitts.'
[ -f "$temporary/mitts" ] && [ ! -L "$temporary/mitts" ] || fail 'Release executable is not a regular file.'
root=${MITTS_INSTALL_ROOT:-${CARGO_INSTALL_ROOT:-${CARGO_HOME:-${HOME:?HOME must be set}/.cargo}}}
destination=$root/bin
mkdir -p "$destination" || fail "Cannot create $destination"
[ ! -d "$destination/mitts" ] || fail 'Install destination is a directory.'
pending=$(mktemp "$destination/.mitts-install.XXXXXX") || fail 'Cannot stage the installation.'
cp "$temporary/mitts" "$pending"
chmod 755 "$pending"
installed_version=$("$pending" --version) ||
    fail 'Cannot execute release binary at destination; check mount permissions and OS/libc compatibility.'
case "$installed_version" in
    "mitts ${version#v}"|"mitts ${version#v} ("*")") ;;
    *) fail "Unexpected binary version: $installed_version" ;;
esac
mv -f "$pending" "$destination/mitts"
pending=
printf '%s installed at %s/mitts\n' "$installed_version" "$destination"
printf 'Ensure %s is on PATH. Rust and a native linker are needed to build Rust projects.\n' "$destination"
