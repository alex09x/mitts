#!/bin/bash
# Mitts Build — one recording run used for the worktree demo
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
# One recording run: ripgrep 14.1.1, first build, unchanged rebuild, second worktree.
set -u
D="$(cd "$(dirname "$0")" && pwd)"
cd "$D"
pri=$(ps -o pri= -p $$ | tr -d ' ')
[ "$pri" = 31 ] || { echo "scheduler priority $pri, not 31: not a normal terminal, stopping"; exit 1; }
export MITTS_CACHE_DIR="$D/cache"
[ -e "$MITTS_CACHE_DIR" ] && { echo "cache dir exists, stopping"; exit 1; }
TIMEFORMAT='  wall %R s'
run() { echo; echo "\$ $*"; time "$@"; }
echo "# Mitts demo, $(date -u '+%Y-%m-%d %H:%M UTC')"
echo "# $(mitts --version) | $(rustc -V)"
echo "# $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) cores, $(sysctl -n hw.memsize | awk '{printf "%d GB", $1/2^30}'), macOS $(sw_vers -productVersion), APFS"
echo "# project: ripgrep 14.1.1 ($(git -C rg-a rev-parse --short HEAD)), dev profile; empty Mitts cache; sources fetched first"
echo "# scheduler priority $pri (normal terminal); other programs on the machine were not stopped"
cd rg-a
run mitts fetch
echo; echo "## 1. first build, empty cache"
run mitts build
echo; echo "## 2. same build again, nothing changed"
run mitts build
echo; echo "## 3. a second worktree of the same commit, empty target/"
git worktree add -q --detach ../rg-b HEAD
cd ../rg-b
echo "\$ cd ../rg-b   # git worktree add ../rg-b"
run mitts build
echo
echo "\$ du -sh \$MITTS_CACHE_DIR ../rg-a/target ../rg-b/target"
du -sh "$MITTS_CACHE_DIR" ../rg-a/target ../rg-b/target
echo; echo "# done"
