#!/usr/bin/env bash
#
# Mitts Build for VS Code — editor integration for the Mitts Rust build tool
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# The clean-machine check, run as root inside a fresh ubuntu:24.04 container with the Mitts
# repository mounted read-only at /src and an output directory at /out:
#
#   docker run --rm --platform linux/amd64 -v "$REPO:/src:ro" -v "$OUT:/out" ubuntu:24.04 \
#     bash /src/editors/vscode/scripts/clean-machine.sh
#
# Installs Mitts with the official installer into an empty home, uses it on a new project,
# then runs the extension's unit and VS Code integration tests. Every step is logged to
# /out/clean-machine.log; the last line says PASS or FAIL.
set -euo pipefail
exec > >(tee /out/clean-machine.log) 2>&1

step() { printf '\n=== %s\n' "$*"; }
trap 'echo; echo "FAIL (line $LINENO)"' ERR

step "machine"
. /etc/os-release && echo "$PRETTY_NAME $(uname -m)"

step "system packages (compiler, VS Code runtime libraries, Xvfb, Node.js)"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates git build-essential xvfb xauth xz-utils \
  libnss3 libgtk-3-0t64 libasound2t64 libgbm1 libxss1 libsecret-1-0 libxkbfile1 >/dev/null
# Node.js 22 (the packager needs 20 or newer; Ubuntu 24.04 ships 18).
curl -fsSL https://nodejs.org/dist/v22.12.0/node-v22.12.0-linux-x64.tar.xz | tar xJ -C /opt
export PATH="/opt/node-v22.12.0-linux-x64/bin:$PATH"
node --version

step "empty home; Rust from rustup"
cd "$HOME" && ls -A "$HOME"
curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal -c clippy >/dev/null
. "$HOME/.cargo/env"
rustc --version
command -v mitts && { echo "mitts already present: not a clean machine"; exit 1; } || echo "no mitts yet"

step "install Mitts with the official installer"
curl -fsSL https://mittsbuild.com/install.sh | sh
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
command -v mitts
mitts --version

step "a new project, built, tested, linted and documented by Mitts"
if mitts new --help >/dev/null 2>&1; then
  mitts new hello
else
  # Mitts before 0.4.0 has no `new`: the same files by hand.
  echo "this Mitts has no \`new\`; creating the project by hand"
  mkdir -p hello/src
  printf '[package]\nname = "hello"\nversion = "0.1.0"\nedition = "2021"\n' > hello/Cargo.toml
  printf 'fn main() {\n    println!("Hello, world!");\n}\n' > hello/src/main.rs
fi
cd hello
mitts build
mitts test
mitts clippy
if mitts doc --help >/dev/null 2>&1; then mitts doc; else echo "this Mitts has no \`doc\`"; fi
mitts run
cd "$HOME"

step "extension: unit tests"
mkdir -p "$HOME/repo/editors" && cp /src/LICENSE-MIT /src/LICENSE-APACHE "$HOME/repo/"
cp -r /src/editors/vscode "$HOME/repo/editors/vscode" && cd "$HOME/repo/editors/vscode"
rm -rf node_modules out
npm ci --no-audit --no-fund
npm run test:unit

step "extension: integration tests in VS Code with rust-analyzer"
xvfb-run -a npm run test:integration

step "extension: package"
npm run package -- -o /out/
ls -la /out/*.vsix

echo
echo PASS
