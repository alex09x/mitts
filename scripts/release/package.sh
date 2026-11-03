#!/usr/bin/env bash
# Mitts Build — package exact-tag native release binaries.
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
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
case "$target" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|aarch64-apple-darwin|x86_64-apple-darwin) ;;
    *) echo 'Unsupported release target' >&2; exit 1 ;;
esac
cd "$source_root"
commit=$(git rev-parse HEAD)
[[ $(git rev-parse "$tag^{commit}") == "$commit" ]]
[[ -z $(git status --porcelain --untracked-files=no) ]]
package_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
[[ "$tag" == "v$package_version" ]]
export RUSTFLAGS="--remap-path-prefix=$source_root=. --remap-path-prefix=$HOME=/rust-home"
export CARGO_INCREMENTAL=0
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo build --release --locked --bin mitts --target "$target"
binary=target/$target/release/mitts
if [[ "$target" == *apple-darwin ]]; then
    codesign --force --sign - "$binary"
fi
version=$("$binary" --version)
[[ "$version" == "mitts $package_version ($commit)" ]]
# Reject either checkout/home spelling, including canonical aliases on macOS.
for needle in "$source_root" "$HOME" "$(cd "$HOME" && pwd -P)"; do
    if grep -aFq "$needle" "$binary"; then
        echo 'Private build path found in release binary' >&2
        exit 1
    fi
done
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp "$binary" "$stage/mitts"
cp LICENSE-MIT LICENSE-APACHE "$stage/"
{
    printf 'Mitts Build %s\nSource: https://github.com/alex09x/mitts/tree/%s\nTarget: %s\n' "$tag" "$commit" "$target"
    rustc -vV
    if [[ "$target" == *linux* ]]; then
        printf 'Required GLIBC symbols:\n'
        readelf --version-info "$binary" | grep -oE 'GLIBC_[0-9.]+' | sort -Vu
    else
        sw_vers
        codesign --verify "$binary"
    fi
} > "$stage/BUILD.txt"
"$stage/mitts" --help > /dev/null
archive=mitts-$tag-$target.tar.gz
tar -czf "$output/$archive" -C "$stage" mitts LICENSE-MIT LICENSE-APACHE BUILD.txt
cd "$output"
if command -v sha256sum >/dev/null; then
    sha256sum "$archive" > "$archive.sha256"
else
    shasum -a 256 "$archive" > "$archive.sha256"
fi
cat "$archive.sha256"
