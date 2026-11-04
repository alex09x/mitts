#!/usr/bin/env bash
# Mitts Build — check the GNU release's maximum glibc symbol version.
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail
versions=$(readelf --version-info "$1" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sort -Vu)
[[ -n "$versions" ]] || { echo 'No GLIBC symbol versions found' >&2; exit 1; }
highest=$(printf '%s\n' "$versions" | tail -n 1)
limit=GLIBC_2.34
if [[ "$(printf '%s\n%s\n' "$highest" "$limit" | sort -V | tail -n 1)" != "$limit" ]]; then
  printf 'Unsupported glibc requirement: %s exceeds %s\n' "$highest" "$limit" >&2
  exit 1
fi
printf 'Maximum glibc requirement: %s\n' "$highest"
