/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Issue 143 on riscv64: skia-bindings' build script runs `skia/bin/fetch-gn`, which has no
//! riscv64 download; the reporter replaced it by hand inside Cargo's unpacked copy
//! (`$CARGO_HOME/registry/src/.../skia-bindings-0.157.0`) and Cargo built. Mitts built its own
//! pristine copy from the source store and failed. Cargo builds the unpacked copy as it finds
//! it, so Mitts does too; an edited copy must never be keyed or stored as the published one.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use tempfile::TempDir;

const INDEX_DIR: &str = "index.crates.io-1949cf8c6b5b557f";

fn write_file(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Unpacks `helper` 0.1.0 the way Cargo does: every file first, `.cargo-ok` last. Its build
/// script fails while `tool/fetch` says `fail`, like `fetch-gn` on riscv64.
fn unpack_helper(cargo_home: &Path) -> PathBuf {
    let dir = cargo_home
        .join("registry/src")
        .join(INDEX_DIR)
        .join("helper-0.1.0");
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    write_file(
        dir.join("Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    );
    write_file(
        dir.join("build.rs"),
        "fn main() {\n\
         let tool = std::fs::read_to_string(\"tool/fetch\").unwrap();\n\
         assert!(tool.trim() != \"fail\", \"KeyError: 'riscv64'\");\n\
         println!(\"cargo:rustc-env=HELPER_TOOL={}\", tool.trim());\n}\n",
    );
    write_file(
        dir.join("src/lib.rs"),
        // Cargo caps lints for a registry package, edited or not.
        "#![deny(warnings)]\nfn unused() {}\npub fn tool() -> &'static str { env!(\"HELPER_TOOL\") }\n",
    );
    write_file(dir.join("tool/fetch"), "fail\n");
    std::thread::sleep(Duration::from_millis(30));
    fs::write(dir.join(".cargo-ok"), "{\"v\":1}").unwrap();
    std::thread::sleep(Duration::from_millis(30));
    dir
}

fn project(root: &Path) {
    write_file(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nhelper = \"0.1.0\"\n",
    );
    write_file(
        root.join("app/Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"helper\"]\n\n\
         [[package]]\nname = \"helper\"\nversion = \"0.1.0\"\n\
         source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
         checksum = \"0000000000000000000000000000000000000000000000000000000000000000\"\n",
    );
    write_file(
        root.join("app/src/main.rs"),
        "fn main() { println!(\"tool={}\", helper::tool()); }\n",
    );
}

fn mitts(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mitts"));
    cmd.args(args)
        .arg("--cache-dir")
        .arg(root.join("mitts-cache"))
        .current_dir(root.join("app"))
        .env("CARGO_HOME", root.join("cargo-home"))
        .env_remove("MITTS_CACHE_DIR");
    for name in [
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
    ] {
        cmd.env_remove(name);
    }
    cmd.output().expect("run Mitts")
}

fn build(root: &Path) -> Output {
    mitts(root, &["build", "--offline"])
}

fn run_app(root: &Path) -> String {
    let out = Command::new(root.join("app/target/debug/app"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn a_hand_edited_cargo_registry_copy_is_built_like_cargo_and_never_cached_as_published() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let cargo_home = root.join("cargo-home");
    project(root);
    let helper = unpack_helper(&cargo_home);

    // The published package fails, as skia-bindings does on riscv64.
    let out = build(root);
    assert!(!out.status.success(), "the pristine helper must fail");
    assert!(stderr(&out).contains("riscv64"), "{}", stderr(&out));

    // The reporter's workaround, inside Cargo's copy.
    fs::write(helper.join("tool/fetch"), "stub\n").unwrap();
    let out = build(root);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(run_app(root), "tool=stub");
    let stored = root.join("mitts-cache/sources/helper-0.1.0/tool/fetch");
    assert!(
        fs::read_to_string(&stored).map_or(true, |s| s == "fail\n"),
        "an edited copy never reaches the source store"
    );

    // Unpacked again (`cargo clean` of the registry, a new machine): the published package
    // must fail again, not come back from the cache entry the edited copy produced.
    unpack_helper(&cargo_home);
    let out = build(root);
    assert!(
        !out.status.success(),
        "a cache entry of the edited copy was served for the published package"
    );
}

/// Packs the copy's files as Cargo's cached `.crate` for that package.
fn cache_archive(cargo_home: &Path, copy: &Path) {
    let name = copy.file_name().unwrap().to_str().unwrap().to_string();
    let mut tar_buf = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut tar_buf, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        for file in ["Cargo.toml", "build.rs", "src/lib.rs", "tool/fetch"] {
            let bytes = fs::read(copy.join(file)).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, format!("{name}/{file}"), bytes.as_slice())
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }
    let dest = cargo_home.join("registry/cache").join(INDEX_DIR);
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join(format!("{name}.crate")), tar_buf).unwrap();
}

fn rewrite_unchanged(path: &Path) {
    let bytes = fs::read(path).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A tool that saves files unchanged (or an experiment that put them back) moves mtimes
/// only: the copy still is the published package, compared against Cargo's archive.
#[test]
fn a_copy_rewritten_with_the_published_contents_stays_the_published_package() {
    use mitts::core::cargo_registry_copy::{is_edited, is_pristine, reset};
    use mitts::core::cargo_registry_verify::set_verdict_dir;
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo-home");
    set_verdict_dir(Some(temp.path().join("verdicts")));

    let dir = unpack_helper(&cargo_home);
    cache_archive(&cargo_home, &dir);
    rewrite_unchanged(&dir.join("src/lib.rs"));
    reset();
    assert!(is_pristine(&dir), "same contents as the archive");
    reset();
    assert!(is_pristine(&dir), "the remembered verdict");

    fs::write(dir.join("tool/fetch"), "stub\n").unwrap();
    reset();
    assert!(is_edited(&dir), "a changed file");
    reset();
    assert!(is_edited(&dir), "the remembered edited verdict");
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.join("tool/fetch"), "fail\n").unwrap();
    reset();
    assert!(is_pristine(&dir), "put back: checked again");

    let dir = unpack_helper(&cargo_home);
    fs::write(dir.join("Cargo.lock"), "version = 3\n").unwrap();
    reset();
    assert!(is_edited(&dir), "a file the archive does not have");

    let dir = unpack_helper(&cargo_home);
    fs::remove_file(
        cargo_home
            .join("registry/cache")
            .join(INDEX_DIR)
            .join("helper-0.1.0.crate"),
    )
    .unwrap();
    rewrite_unchanged(&dir.join("src/lib.rs"));
    reset();
    assert!(is_edited(&dir), "nothing to compare with");
    cache_archive(&cargo_home, &dir);
    reset();
    assert!(is_pristine(&dir), "the archive arrived");
    set_verdict_dir(None);
}

/// Only the copy Cargo uses counts: crates.io's sparse index directory, the most recently
/// unpacked one when several Cargo versions left copies.
#[test]
fn an_edit_in_a_copy_cargo_does_not_use_is_ignored() {
    use mitts::core::source_store::RegistryLookup;
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo-home");
    let copy_in = |index: &str| {
        let src = cargo_home.join("registry/src");
        let dir = unpack_helper(&cargo_home);
        let to = src.join(index).join("helper-0.1.0");
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::rename(&dir, &to).unwrap();
        to
    };
    let edit = |dir: &Path| fs::write(dir.join("tool/fetch"), "stub\n").unwrap();
    let lookup = || RegistryLookup::scan(Some(&cargo_home), Path::new("/nonexistent"));

    let legacy = copy_in("github.com-1ecc6299db9ec823");
    edit(&legacy);
    copy_in("index.crates.io-1949cf8c6b5b557f");
    mitts::core::cargo_registry_copy::reset();
    assert_eq!(
        lookup().find_edited_in_src("helper", "0.1.0"),
        None,
        "legacy git index"
    );

    let older = copy_in("index.crates.io-6f17d22bba15001f");
    edit(&older);
    std::thread::sleep(Duration::from_millis(30));
    let newer = copy_in("index.crates.io-1949cf8c6b5b557f");
    mitts::core::cargo_registry_copy::reset();
    assert_eq!(
        lookup().find_edited_in_src("helper", "0.1.0"),
        None,
        "an older Cargo's copy"
    );
    edit(&newer);
    mitts::core::cargo_registry_copy::reset();
    assert_eq!(lookup().find_edited_in_src("helper", "0.1.0"), Some(newer));
}

/// The reporter had built with Mitts before, so the source store already held the published
/// helper; the edit in Cargo's copy still wins, as it does for Cargo.
#[test]
fn an_edited_cargo_copy_wins_over_the_published_copy_in_the_source_store() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    project(root);
    let helper = unpack_helper(&root.join("cargo-home"));
    let stored = root.join("mitts-cache/sources/helper-0.1.0");
    for file in ["Cargo.toml", "build.rs", "src/lib.rs", "tool/fetch"] {
        let contents = fs::read_to_string(helper.join(file)).unwrap();
        write_file(stored.join(file), &contents);
    }
    let marker = stored.join(mitts::core::source_store::MITTS_OK_MARKER);
    fs::write(marker, "ok\n").unwrap();
    assert!(!build(root).status.success(), "the stored helper fails");

    fs::write(helper.join("tool/fetch"), "stub\n").unwrap();
    let out = build(root);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(run_app(root), "tool=stub");
}

#[test]
fn only_an_untouched_cargo_copy_counts_as_the_published_package() {
    use mitts::core::cargo_registry_copy::{is_edited, is_pristine, reset};
    let temp = TempDir::new().unwrap();
    let cargo_home = temp.path().join("cargo-home");

    let dir = unpack_helper(&cargo_home);
    reset();
    assert!(is_pristine(&dir) && !is_edited(&dir));

    fs::write(dir.join("tool/fetch"), "stub\n").unwrap();
    reset();
    assert!(is_edited(&dir), "an edited file");

    let dir = unpack_helper(&cargo_home);
    #[cfg(unix)]
    std::os::unix::fs::symlink("/usr/bin/true", dir.join("tool/gn")).unwrap();
    #[cfg(not(unix))]
    fs::write(dir.join("tool/gn"), "").unwrap();
    reset();
    assert!(is_edited(&dir), "an added file");

    let dir = unpack_helper(&cargo_home);
    fs::remove_file(dir.join("build.rs")).unwrap();
    reset();
    assert!(is_edited(&dir), "a removed top-level file");

    // A file named like Cargo's marker below the package root is the package's own.
    let dir = unpack_helper(&cargo_home);
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.join("tool/.cargo-ok"), "edited").unwrap();
    reset();
    assert!(is_edited(&dir), "a nested .cargo-ok is a package file");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `chmod` moves only the status-change time; `touch -r` cannot hide an edit either.
        let dir = unpack_helper(&cargo_home);
        cache_archive(&cargo_home, &dir);
        fs::set_permissions(dir.join("tool/fetch"), fs::Permissions::from_mode(0o755)).unwrap();
        reset();
        assert!(is_edited(&dir), "an execute bit the archive does not have");

        let dir = unpack_helper(&cargo_home);
        cache_archive(&cargo_home, &dir);
        let before = fs::metadata(dir.join("src/lib.rs"))
            .unwrap()
            .modified()
            .unwrap();
        fs::write(
            dir.join("src/lib.rs"),
            "pub fn tool() -> &'static str { \"x\" }\n",
        )
        .unwrap();
        fs::File::options()
            .write(true)
            .open(dir.join("src/lib.rs"))
            .unwrap()
            .set_modified(before)
            .unwrap();
        reset();
        assert!(is_edited(&dir), "an edit with its mtime put back");

        let dir = unpack_helper(&cargo_home);
        let fifo = dir.join("tool/pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.is_ok_and(|s| s.success()) {
            reset();
            assert!(is_edited(&dir), "a FIFO is never a published file");
        }
    }

    let half = cargo_home
        .join("registry/src")
        .join(INDEX_DIR)
        .join("half-0.1.0");
    write_file(half.join("Cargo.toml"), "[package]\nname = \"half\"\n");
    reset();
    assert!(
        !is_pristine(&half) && !is_edited(&half),
        "without `.cargo-ok` it is not a Cargo copy at all"
    );
}
