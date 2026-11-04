/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(super) fn for_unit_kind(kind: &str) -> Vec<&str> {
    let kinds: Vec<_> = kind.split(',').map(str::trim).collect();
    let has_native_type = kinds
        .iter()
        .any(|kind| matches!(*kind, "dylib" | "cdylib" | "staticlib"));
    if !has_native_type {
        return Vec::new();
    }
    kinds
        .into_iter()
        .filter(|kind| matches!(*kind, "rlib" | "dylib" | "cdylib" | "staticlib"))
        .collect()
}
