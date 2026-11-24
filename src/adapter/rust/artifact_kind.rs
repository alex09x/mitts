/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a library output of rustc is, on every platform. A dynamic library is `.dylib`
//! (Apple), `.so` or `.dll` (Windows, where a proc-macro is one too); a static one is `.a`,
//! or `.lib` on Windows, where rustc also writes `NAME.dll.lib` beside a `.dll`: the import
//! library that links against it, never a static archive and never a crate rustc can load.
//! Names follow the platform: `libNAME.rlib` everywhere, but `NAME.dll` and `NAME.lib`
//! without the `lib` prefix on Windows.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Rlib,
    Dynamic,
    Static,
    /// `NAME.dll.lib`: links against a `.dll`.
    ImportLib,
    Rmeta,
}

pub fn classify(path: &Path) -> Option<ArtifactKind> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".dll.lib") {
        return Some(ArtifactKind::ImportLib);
    }
    kind_of_extension(path.extension()?.to_str()?)
}

pub fn kind_of_extension(ext: &str) -> Option<ArtifactKind> {
    match ext {
        "rlib" => Some(ArtifactKind::Rlib),
        "dylib" | "so" | "dll" => Some(ArtifactKind::Dynamic),
        "a" | "lib" => Some(ArtifactKind::Static),
        "rmeta" => Some(ArtifactKind::Rmeta),
        _ => None,
    }
}

/// A crate rustc can take with `--extern` (rlib or dynamic library).
pub fn is_linkable(path: &Path) -> bool {
    matches!(
        classify(path),
        Some(ArtifactKind::Rlib | ArtifactKind::Dynamic)
    )
}

/// A static archive (not an import library).
pub fn is_static(path: &Path) -> bool {
    classify(path) == Some(ArtifactKind::Static)
}

/// How strongly an output stands for its crate as a dependency: rlib, then a dynamic
/// library, then a static one; anything else (import library, rmeta) not at all.
pub fn extern_priority(path: &Path) -> u8 {
    match classify(path) {
        Some(ArtifactKind::Rlib) => 3,
        Some(ArtifactKind::Dynamic) => 2,
        Some(ArtifactKind::Static) => 1,
        _ => 0,
    }
}

/// The suffix rustc gives an executable for `triple` (its target spec's `exe-suffix`):
/// `.exe` on Windows, `.wasm` on WebAssembly, `.js` with Emscripten, none elsewhere.
pub fn exe_suffix_for_triple(triple: &str) -> &'static str {
    if triple.contains("-windows") {
        ".exe"
    } else if triple.contains("emscripten") {
        ".js"
    } else if triple.starts_with("wasm32") || triple.starts_with("wasm64") {
        ".wasm"
    } else {
        ""
    }
}

/// Every executable suffix a target can have, for finding what rustc wrote.
pub const EXE_SUFFIXES: [&str; 3] = [".exe", ".wasm", ".js"];

/// The file name a library of `crate_name` with extension `ext` has on its platform:
/// `libNAME.ext`, except `NAME.dll` and `NAME.lib` (Windows).
pub fn library_file_name(crate_name: &str, ext: &str) -> String {
    match ext {
        "dll" | "lib" => format!("{crate_name}.{ext}"),
        _ => format!("lib{crate_name}.{ext}"),
    }
}

/// Whether `file_name` is an output of crate `crate_name` (`libNAME…` or, for Windows
/// libraries, `NAME…`).
pub fn names_crate(file_name: &str, crate_name: &str) -> bool {
    let named = |rest: &str| rest.starts_with(['-', '.']);
    file_name
        .strip_prefix(&format!("lib{crate_name}"))
        .is_some_and(named)
        || (file_name.ends_with(".dll") || file_name.ends_with(".lib"))
            && file_name.strip_prefix(crate_name).is_some_and(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_outputs_are_told_apart() {
        let k = |n: &str| classify(Path::new(n));
        assert_eq!(k("libfoo-1.rlib"), Some(ArtifactKind::Rlib));
        assert_eq!(k("foo_derive-1.dll"), Some(ArtifactKind::Dynamic));
        assert_eq!(k("foo_derive-1.dll.lib"), Some(ArtifactKind::ImportLib));
        assert_eq!(k("foo.lib"), Some(ArtifactKind::Static));
        assert_eq!(k("libfoo.so"), Some(ArtifactKind::Dynamic));
        assert!(is_linkable(Path::new("foo_derive-1.dll")));
        assert!(!is_linkable(Path::new("foo_derive-1.dll.lib")));
        assert!(!is_static(Path::new("foo_derive-1.dll.lib")));
        assert!(extern_priority(Path::new("a.dll")) > extern_priority(Path::new("a.dll.lib")));
    }

    #[test]
    fn names_follow_the_platform() {
        assert_eq!(library_file_name("foo", "rlib"), "libfoo.rlib");
        assert_eq!(library_file_name("foo", "dll"), "foo.dll");
        assert_eq!(library_file_name("foo", "lib"), "foo.lib");
        assert_eq!(library_file_name("foo", "so"), "libfoo.so");
        assert!(names_crate("libfoo-1.rlib", "foo"));
        assert!(names_crate("foo-1.dll", "foo"));
        assert!(names_crate("foo.lib", "foo"));
        assert!(!names_crate("foobar-1.dll", "foo"));
        assert!(!names_crate("foo-1.rlib", "foo"));
    }
}
