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
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|aarch64-apple-darwin) ;;
    *) echo 'Unsupported release target' >&2; exit 1 ;;
esac
cd "$source_root"
commit=$(git rev-parse HEAD)
[[ $(git rev-parse "$tag^{commit}") == "$commit" ]]
[[ -z $(git status --porcelain --untracked-files=no) ]]
package_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
[[ "$tag" == "v$package_version" ]]
export RUSTFLAGS="--remap-path-prefix=$source_root=. --remap-path-prefix=$HOME=/rust-home -C strip=debuginfo"
export CARGO_INCREMENTAL=0
export MACOSX_DEPLOYMENT_TARGET=11.0
builder=${MITTS_CI_BIN:-mitts}
cache=${MITTS_CI_CACHE_DIR:-$source_root/target/mitts-release-cache}
host=$(rustc -vV | sed -n 's/^host: //p')
[[ "$host" == "$target" ]]
lock_digest() {
    if command -v sha256sum >/dev/null; then sha256sum Cargo.lock; else shasum -a 256 Cargo.lock; fi
}
lock_before=$(lock_digest)
"$builder" --version
"$builder" build --release --native --bin mitts --manifest-path "$source_root/Cargo.toml" \
    --cache-dir "$cache" --target-dir "$source_root/target/mitts-release"
[[ "$(lock_digest)" == "$lock_before" ]]
# Mitts outputs may share immutable CAS inodes. Sign only an owned private copy.
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp target/mitts-release/release/mitts "$stage/mitts"
chmod u+w "$stage/mitts"
binary="$stage/mitts"
if [[ "$target" == *apple-darwin ]]; then
    codesign --force --sign - "$binary"
fi
version=$("$binary" --version)
[[ "$version" == "mitts $package_version ($commit)" ]]
# Reject either checkout/home spelling, including canonical aliases on macOS.
for needle in "$source_root" "$HOME" "$(cd "$HOME" && pwd -P)"; do
    if grep -aFq "$needle" "$binary"; then
        echo 'Private build path found in release binary' >&2
        python3 - "$binary" "$needle" <<'PY'
import pathlib
import sys

data = pathlib.Path(sys.argv[1]).read_bytes()
needle = sys.argv[2].encode()
offset = data.find(needle)
end = data.find(b"\0", offset)
suffix = data[offset + len(needle):min(end if end >= 0 else len(data), offset + len(needle) + 200)]
print(f"Path hygiene diagnostic: occurrences={data.count(needle)}, offset={offset}, suffix={suffix!r}", file=sys.stderr)
PY
        exit 1
    fi
done
cp LICENSE-MIT LICENSE-APACHE "$stage/"
{
    printf 'Mitts Build %s\nSource: https://github.com/alex09x/mitts/tree/%s\nTarget: %s\n' "$tag" "$commit" "$target"
    rustc -vV
    printf 'Builder: %s\n' "$("$builder" --version)"
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
