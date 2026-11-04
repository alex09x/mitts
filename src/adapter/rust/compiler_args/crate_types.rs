/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// The `--crate-type` values of a library unit that also builds a native library, or none for a
/// plain library (rustc then defaults to an rlib). `lib` in a manifest's `crate-type` list is
/// the Rust library, an rlib; it must be passed on as such, or the unit builds no rlib and
/// dependents find only the cdylib, which carries no Rust metadata.
pub(super) fn for_unit_kind(kind: &str) -> Vec<&str> {
    let kinds: Vec<_> = kind.split(',').map(str::trim).collect();
    let has_native_type = kinds
        .iter()
        .any(|kind| matches!(*kind, "dylib" | "cdylib" | "staticlib"));
    if !has_native_type {
        return Vec::new();
    }
    let mut types: Vec<&str> = Vec::new();
    for kind in kinds {
        let crate_type = match kind {
            "lib" | "rlib" => "rlib",
            "dylib" | "cdylib" | "staticlib" => kind,
            _ => continue,
        };
        if !types.contains(&crate_type) {
            types.push(crate_type);
        }
    }
    types
}

#[cfg(test)]
mod tests {
    use super::for_unit_kind;

    #[test]
    fn lib_is_an_rlib_next_to_native_types() {
        assert_eq!(
            for_unit_kind("lib,cdylib,staticlib"),
            ["rlib", "cdylib", "staticlib"]
        );
        assert_eq!(
            for_unit_kind("staticlib,cdylib,rlib"),
            ["staticlib", "cdylib", "rlib"]
        );
        assert_eq!(for_unit_kind("lib,rlib,cdylib"), ["rlib", "cdylib"]);
    }

    #[test]
    fn a_plain_library_passes_no_crate_type() {
        assert!(for_unit_kind("lib").is_empty());
        assert!(for_unit_kind("rlib").is_empty());
        assert!(for_unit_kind("bin").is_empty());
    }

    #[test]
    fn only_native_types_without_an_rlib_stay_so() {
        assert_eq!(for_unit_kind("cdylib"), ["cdylib"]);
        assert_eq!(for_unit_kind("cdylib,staticlib"), ["cdylib", "staticlib"]);
    }
}
