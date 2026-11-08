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
api=https://api.github.com/repos/alex09x/mitts
# Transient 5xx and timeouts are retried by curl itself; 4xx answers are final.
fetch() { curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 --retry 3 --retry-delay 2 --connect-timeout 20 "$@"; }
download() { fetch "$1" -o "$2"; }
# github.com release pages can answer 504 while api.github.com stays up, so every
# lookup and download has an API fallback. GITHUB_TOKEN, when set, only raises the rate limit.
api_get() {
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        fetch -H "Authorization: Bearer $GITHUB_TOKEN" -H 'X-GitHub-Api-Version: 2022-11-28' -H "Accept: $1" "$2" -o "$3"
    else
        fetch -H 'X-GitHub-Api-Version: 2022-11-28' -H "Accept: $1" "$2" -o "$3"
    fi
}
api_latest_tag() {
    api_get application/vnd.github+json "$api/releases/latest" "$temporary/latest.json" || return 1
    tr ',{}' '\n\n\n' <"$temporary/latest.json" |
        awk '/"tag_name": *"/ { t=$0; sub(/.*"tag_name": *"/, "", t); sub(/".*/, "", t); print t; exit }'
}
api_download() {
    [ -s "$temporary/release.json" ] ||
        api_get application/vnd.github+json "$api/releases/tags/$version" "$temporary/release.json" || return 1
    id=$(tr ',{}' '\n\n\n' <"$temporary/release.json" | awk -v name="$1" '
        /"url": *"[^"]*\/releases\/assets\/[0-9]+"/ { u=$0; sub(/.*\/releases\/assets\//, "", u); sub(/".*/, "", u); id=u }
        /"name": *"/ { n=$0; sub(/.*"name": *"/, "", n); sub(/".*/, "", n); if (n == name && id != "") { print id; exit } }')
    printf '%s\n' "$id" | grep -Eq '^[0-9]+$' || return 1
    api_get application/octet-stream "$api/releases/assets/$id" "$2"
}
release_file() {
    download "$base/$1" "$2" && return 0
    rm -f "$2"
    printf 'mitts installer: github.com download failed for %s; retrying through api.github.com…\n' "$1" >&2
    api_download "$1" "$2" || { rm -f "$2"; return 1; }
}
temporary=$(mktemp -d) || fail 'Could not create a temporary directory.'
pending=
trap 'rm -rf "$temporary"; if [ -n "$pending" ]; then rm -f "$pending"; fi' EXIT
trap 'exit 1' HUP INT TERM

version=${MITTS_VERSION:-}
if [ -z "$version" ]; then
    if latest=$(fetch --head "$repository/releases/latest" -o /dev/null --write-out '%{url_effective}'); then
        case "$latest" in
            "$repository/releases/tag/"*) version=${latest##*/} ;;
            *) fail 'Latest release did not resolve to a GitHub release tag.' ;;
        esac
    else
        printf 'mitts installer: github.com release lookup failed; retrying through api.github.com…\n' >&2
        version=$(api_latest_tag) ||
            fail 'Cannot resolve the latest stable release. Retry, or set MITTS_VERSION=vX.Y.Z.'
    fi
fi
case "$version" in v*) ;; *) version=v$version ;; esac
printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail 'Release must have a stable vX.Y.Z tag.'
asset=mitts-$version-$target.tar.gz
base=$repository/releases/download/$version
printf 'Installing Mitts %s for %s…\n' "$version" "$target"
release_file "$asset" "$temporary/$asset" || fail "Release $version has no downloadable $target archive."
release_file SHA256SUMS "$temporary/SHA256SUMS" || fail 'Cannot download release checksums.'
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
