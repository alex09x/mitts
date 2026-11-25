#!/bin/sh
# Mitts Build — acceptance tests for site/install.sh in a Windows POSIX shell
# (Git Bash, MSYS2 environments, Cygwin).
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Usage: windows_install_sh_test.sh <install.sh> [--hold]
# --hold also upgrades mitts.exe while it runs a build (needs Rust with the MSVC linker).
set -eu
installer=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
hold=${2:-}
failures=0
check() { if eval "$1"; then echo "  ok: $2"; else echo "  FAIL: $2"; failures=$((failures + 1)); fi; }
work=$(mktemp -d)
echo "shell $(uname -s) $(uname -m), work dir $work"

auth=
[ -z "${GITHUB_TOKEN:-}" ] || auth="Authorization: Bearer $GITHUB_TOKEN"
tags=$(curl -fsSL ${auth:+-H "$auth"} 'https://api.github.com/repos/alex09x/mitts/releases?per_page=30' |
    grep -o '"browser_download_url": *"[^"]*/download/v[0-9]*\.[0-9]*\.[0-9]*/mitts-v[^"]*-x86_64-pc-windows-msvc\.tar\.gz"' |
    sed 's#.*/download/\(v[0-9.]*\)/.*#\1#' | awk '!seen[$0]++')
latest=$(printf '%s\n' "$tags" | sed -n 1p)
previous=$(printf '%s\n' "$tags" | sed -n 2p)
echo "latest $latest, previous $previous"
run() { env -u MITTS_VERSION -u MITTS_INSTALL_ROOT -u CARGO_INSTALL_ROOT -u CARGO_HOME \
    MITTS_NO_SOURCE_FALLBACK=1 "$@" sh "$installer"; }

echo '== default prefix: $HOME/.cargo (a path with a space)'
home="$work/user home"
mkdir -p "$home"
check 'run HOME="$home" > "$work/out" 2>&1' 'completes'
cat "$work/out"
check '[ -f "$home/.cargo/bin/mitts.exe" ]' 'installs .cargo/bin/mitts.exe'
check '"$home/.cargo/bin/mitts.exe" --version | grep -q "^mitts ${latest#v}"' "is $latest"
check '"$home/.cargo/bin/mitts" --version > /dev/null' 'runs as plain `mitts` from this shell'
check '[ -z "$(ls -A "$home/.cargo/bin" | grep -v "^mitts.exe$")" ]' 'bin holds mitts.exe alone'

echo "== pinned $previous, then reinstalled over it"
root="$work/pinned"
check 'run MITTS_INSTALL_ROOT="$root" MITTS_VERSION="${previous#v}" > /dev/null 2>&1' 'pinned install completes'
check '"$root/bin/mitts.exe" --version | grep -q "^mitts ${previous#v}"' "is $previous"
if [ "$hold" = --hold ]; then
    echo "== upgrade to $latest while the installed mitts.exe is building"
    project="$work/hold project"
    mkdir -p "$project/src"
    printf '[package]\nname = "hold"\nversion = "0.1.0"\nedition = "2021"\n' > "$project/Cargo.toml"
    printf 'fn main() { println!("hold ok"); }\n' > "$project/src/main.rs"
    cat > "$project/build.rs" <<'EOF'
fn main() {
    std::fs::write(std::env::var("HOLD_MARKER").unwrap(), "started").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(45));
}
EOF
    marker="$work/build-script-started"
    win() { if command -v cygpath > /dev/null; then cygpath -w "$1"; else printf '%s\n' "$1"; fi; }
    HOLD_MARKER=$(win "$marker") MITTS_CACHE_DIR=$(win "$work/cache") \
        "$root/bin/mitts.exe" build --manifest-path "$(win "$project/Cargo.toml")" > "$work/hold.log" 2>&1 &
    build=$!
    i=0
    while [ ! -f "$marker" ] && [ $i -lt 600 ] && kill -0 $build 2> /dev/null; do sleep 0.5; i=$((i + 1)); done
    check '[ -f "$marker" ]' 'the build is running (its build script started)'
    check 'run MITTS_INSTALL_ROOT="$root" MITTS_VERSION="$latest" > "$work/out" 2>&1' 'the installer completes'
    cat "$work/out"
    check '"$root/bin/mitts.exe" --version | grep -q "^mitts ${latest#v}"' "mitts.exe is now $latest"
    status=0
    wait $build || status=$?
    cat "$work/hold.log"
    check '[ $status -eq 0 ]' "the running build finished (exit $status)"
    check '[ -f "$project/target/debug/hold.exe" ]' 'and produced target/debug/hold.exe'
fi
check 'run MITTS_INSTALL_ROOT="$root" MITTS_VERSION="$latest" > /dev/null 2>&1' "reinstall of $latest completes"
check '"$root/bin/mitts.exe" --version | grep -q "^mitts ${latest#v}"' "is $latest"
check '[ "$(ls -A "$root/bin")" = mitts.exe ]' 'no staging or old copies left'

rm -rf "$work"
if [ $failures -gt 0 ]; then echo "FAILED: $failures"; exit 1; fi
echo 'ALL INSTALLER CHECKS PASSED'
