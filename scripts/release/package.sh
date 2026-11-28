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
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|aarch64-apple-darwin|x86_64-pc-windows-msvc) ;;
    riscv64gc-unknown-linux-gnu) ;;
    *) echo 'Unsupported release target' >&2; exit 1 ;;
esac
exe=mitts
# Git Bash on Windows: rustc and the binary see native paths (`D:\a\…`), not `/d/a/…`.
native() { if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi; }
if [[ "$target" == *-windows-* ]]; then
    exe=mitts.exe
fi
cd "$source_root"
commit=$(git rev-parse HEAD)
[[ $(git rev-parse "$tag^{commit}") == "$commit" ]]
[[ -z $(git status --porcelain --untracked-files=no) ]]
package_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
[[ "$tag" == "v$package_version" ]]
# Native dependency assembly may retain absolute paths in DWARF despite Rust
# remapping. Distribution binaries omit debug info while keeping normal symbols.
export RUSTFLAGS="--remap-path-prefix=$(native "$source_root")=. --remap-path-prefix=$(native "$HOME")=/rust-home -C strip=debuginfo"
if [[ "$target" == *-windows-msvc ]]; then
    # The C runtime linked in, so the binary does not need the VC++ redistributable.
    RUSTFLAGS="$RUSTFLAGS -C target-feature=+crt-static"
fi
export CARGO_INCREMENTAL=0
export MACOSX_DEPLOYMENT_TARGET=11.0
builder=${MITTS_CI_BIN:-mitts}
cache=${MITTS_CI_CACHE_DIR:-$source_root/target/mitts-release-cache}
host=$(rustc -vV | sed -n 's/^host: //p')
# Every target builds on its own platform, except riscv64 (no hosted runners): it is
# cross-built on x86_64 Linux with the GNU cross toolchain and run under qemu-user.
target_args=()
built=target/release/$exe
run=()
if [[ "$target" == riscv64gc-unknown-linux-gnu ]]; then
    [[ "$host" == x86_64-unknown-linux-gnu ]]
    export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc
    export CC_riscv64gc_unknown_linux_gnu=riscv64-linux-gnu-gcc
    target_args=(--target "$target")
    built=target/$target/release/$exe
    run=(qemu-riscv64 -L /usr/riscv64-linux-gnu)
else
    [[ "$host" == "$target" ]]
fi
lock_digest() {
    if command -v sha256sum >/dev/null; then sha256sum Cargo.lock; else shasum -a 256 Cargo.lock; fi
}
lock_before=$(lock_digest)
"$builder" --version
"$builder" build --release --native --bin mitts --manifest-path "$source_root/Cargo.toml" \
    --cache-dir "$cache" --target-dir "$source_root/target" ${target_args[@]+"${target_args[@]}"}
[[ "$(lock_digest)" == "$lock_before" ]]
# Mitts outputs may share immutable CAS inodes. Sign only an owned private copy.
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp "$built" "$stage/$exe"
chmod u+w "$stage/$exe"
binary="$stage/$exe"
if [[ "$target" == *-windows-* ]]; then
    : # No glibc floor and no codesign step for the MSVC build.
elif [[ "$target" == *apple-darwin ]]; then
    # rustc strips debug info with rust-objcopy, which some runner toolchains cannot start
    # (libLLVM.dylib missing). The linker's debug map then still names the precompiled std
    # objects under $HOME/.rustup, which the path check below rejects. Apple's strip removes
    # the debug map whatever rustc did; the signature is made afterwards.
    strip -S "$binary"
    codesign --force --sign - "$binary"
else
    bash "$source_root/scripts/release/check-glibc.sh" "$binary"
fi
version=$(${run[@]+"${run[@]}"} "$binary" --version)
[[ "$version" == "mitts $package_version ($commit)" ]]
# Reject either checkout/home spelling, including canonical aliases on macOS.
for needle in "$source_root" "$HOME" "$(cd "$HOME" && pwd -P)" \
    "$(native "$source_root")" "$(native "$HOME")"; do
    if grep -aFiq "$needle" "$binary"; then
        echo 'Private build path found in release binary' >&2
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
    elif [[ "$target" == *-windows-* ]]; then
        printf 'Built on: %s\n' "$(uname -sr)"
    else
        sw_vers
        codesign --verify "$binary"
    fi
} > "$stage/BUILD.txt"
${run[@]+"${run[@]}"} "$binary" --help > /dev/null
archive=mitts-$tag-$target.tar.gz
tar -czf "$output/$archive" -C "$stage" "$exe" LICENSE-MIT LICENSE-APACHE BUILD.txt
cd "$output"
# `<hash>  <name>` on every platform: Git Bash's sha256sum marks binary mode with `*name`.
if command -v sha256sum >/dev/null; then
    hash=$(sha256sum < "$archive")
else
    hash=$(shasum -a 256 < "$archive")
fi
printf '%s  %s\n' "${hash%% *}" "$archive" > "$archive.sha256"
cat "$archive.sha256"
