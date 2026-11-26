/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The cache stays under its limit on a host that never stops building (ram11: a test gate
//! held the cache all day and it grew to three times the limit).
//!
//! - While a build holds the cache, objects unused for longer than the grace window are
//!   pruned; recently used and linked objects are kept.
//! - Old versions of a test or binary go before anything else; the version a checkout went
//!   back to (used last) is the one kept.
//! - A cache hit moves an object's time forward, so pruning follows use, not creation.

use mitts::core::action_cache::{ActionCache, ActionCacheEntry, CachedOutput};
use mitts::core::cas::ContentAddressedStore;
use mitts::core::gc::{eviction, CacheGc, CacheLease, GcOptions};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

struct Store {
    _dir: TempDir,
    root: std::path::PathBuf,
    cas: ContentAddressedStore,
    ac: ActionCache,
}

fn store() -> Store {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("cache");
    let cas = ContentAddressedStore::new(root.join("cas")).unwrap();
    let ac = ActionCache::new(root.join("ac")).unwrap();
    Store {
        _dir: dir,
        root,
        cas,
        ac,
    }
}

const HOUR: Duration = Duration::from_secs(3600);

/// Stores `bytes` and dates the object `age` ago.
fn object(s: &Store, bytes: &[u8], age: Duration) -> String {
    let (hash, _) = s.cas.store_bytes(bytes).unwrap();
    set_age(&s.cas.object_path(&hash), age);
    hash
}

fn set_age(path: &Path, age: Duration) {
    mitts::core::file_time::set_modified(path, SystemTime::now() - age).unwrap();
}

fn age_of(path: &Path) -> Duration {
    SystemTime::now()
        .duration_since(fs::metadata(path).unwrap().modified().unwrap())
        .unwrap_or_default()
}

const PROJECT: &str = "project-a";

/// An action cache entry for `target` of [`PROJECT`] created `age` ago with one output.
fn entry(s: &Store, key: &str, target: &str, hash: &str, executable: bool, age: Duration) {
    entry_in(s, PROJECT, key, target, hash, executable, age);
}

fn entry_in(
    s: &Store,
    project: &str,
    key: &str,
    target: &str,
    hash: &str,
    executable: bool,
    age: Duration,
) {
    let created = SystemTime::now() - age;
    s.ac.put(&ActionCacheEntry {
        action_key: key.to_string(),
        action_id: target.to_string(),
        created_at_secs: created.duration_since(UNIX_EPOCH).unwrap().as_secs(),
        outputs: vec![CachedOutput {
            relative_path: format!("debug/deps/{key}").into(),
            cas_hash: hash.to_string(),
            file_size: s.cas.object_path(hash).metadata().unwrap().len(),
            executable,
        }],
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        target_scopes: vec![project.to_string()],
    })
    .unwrap();
}

fn limit(bytes: u64) -> GcOptions {
    GcOptions {
        max_bytes: Some(bytes),
        max_age: None,
    }
}

#[test]
fn a_held_cache_still_prunes_what_no_build_used_recently() {
    let s = store();
    let old = object(&s, &[1u8; 1000], 5 * HOUR);
    let fresh = object(&s, &[2u8; 1000], Duration::from_secs(60));
    let linked = object(&s, &[3u8; 1000], 5 * HOUR);
    fs::hard_link(s.cas.object_path(&linked), s.root.join("workspace-link")).unwrap();

    let _lease = CacheLease::acquire(Some(&s.root));
    let report = CacheGc::new(&s.cas, &s.ac).prune(&limit(0)).unwrap();

    assert!(report.build_held_cache);
    assert!(
        !s.cas.has_object(&old),
        "unused for 5 h: pruned under the lease"
    );
    assert!(s.cas.has_object(&fresh), "used a minute ago: kept");
    assert!(s.cas.has_object(&linked), "linked: frees nothing yet, kept");
    assert_eq!(report.objects_pruned, 1);
}

/// hitmeup: builds held the cache without pause and wrote 66 GB within the two-hour grace
/// window, so the cache stayed far over its limit. Over the limit, the window gives way:
/// only what a build stored in the last minutes and what a target links to stay.
#[test]
fn a_held_cache_over_its_limit_prunes_inside_the_grace_window() {
    let s = store();
    let half_hour = object(&s, &[1u8; 1000], Duration::from_secs(1800));
    let just_stored = object(&s, &[2u8; 1000], Duration::from_secs(60));
    let linked = object(&s, &[3u8; 1000], Duration::from_secs(1800));
    fs::hard_link(s.cas.object_path(&linked), s.root.join("workspace-link")).unwrap();

    let _lease = CacheLease::acquire(Some(&s.root));
    let report = CacheGc::new(&s.cas, &s.ac).prune(&limit(1000)).unwrap();

    assert!(report.build_held_cache);
    assert!(!s.cas.has_object(&half_hour), "30 min old, over the limit");
    assert!(s.cas.has_object(&just_stored), "stored a minute ago: kept");
    assert!(s.cas.has_object(&linked), "linked into a target: kept");

    // Under the limit the window still holds: nothing more goes.
    let report = CacheGc::new(&s.cas, &s.ac).prune(&limit(1 << 20)).unwrap();
    assert_eq!(report.objects_pruned, 0);
}

#[test]
fn without_a_build_the_limit_is_kept_as_before() {
    let s = store();
    let fresh = object(&s, &[2u8; 1000], Duration::from_secs(60));
    let report = CacheGc::new(&s.cas, &s.ac).prune(&limit(0)).unwrap();
    assert!(!report.build_held_cache);
    assert!(
        !s.cas.has_object(&fresh),
        "no build: the grace window does not apply"
    );
}

#[test]
fn old_versions_of_a_test_binary_go_before_older_libraries() {
    let s = store();
    let lib = object(&s, &[9u8; 1000], 10 * HOUR);
    entry(&s, "lib1", "dep@1.0.0::lib:dep", &lib, false, 10 * HOUR);
    let v1 = object(&s, &[1u8; 1000], 6 * HOUR);
    entry(&s, "t1", "app@0.1.0::test:bridge", &v1, true, 6 * HOUR);
    let v2 = object(&s, &[2u8; 1000], 3 * HOUR);
    entry(&s, "t2", "app@0.1.0::test:bridge", &v2, true, 3 * HOUR);

    let report = CacheGc::new(&s.cas, &s.ac).prune(&limit(2000)).unwrap();

    assert_eq!(report.objects_pruned, 1);
    assert!(
        !s.cas.has_object(&v1),
        "the superseded test binary goes first"
    );
    assert!(s.cas.has_object(&lib), "the older library stays");
    assert!(s.cas.has_object(&v2), "the latest version stays");
}

#[test]
fn the_version_a_checkout_went_back_to_is_kept() {
    let s = store();
    let v1 = object(&s, &[1u8; 1000], 6 * HOUR);
    entry(&s, "t1", "app@0.1.0::test:bridge", &v1, true, 6 * HOUR);
    let v2 = object(&s, &[2u8; 1000], 3 * HOUR);
    entry(&s, "t2", "app@0.1.0::test:bridge", &v2, true, 3 * HOUR);
    // Back on the old branch: v1 was a cache hit a minute ago.
    set_age(&s.cas.object_path(&v1), Duration::from_secs(60));

    let superseded = eviction::superseded_executables(&s.ac, |h| {
        fs::metadata(s.cas.object_path(h)).ok()?.modified().ok()
    });
    assert!(superseded.contains(&v2) && !superseded.contains(&v1));
}

#[test]
fn a_hit_moves_an_object_forward_at_most_once_an_hour() {
    let s = store();
    let old = object(&s, &[1u8; 100], 5 * HOUR);
    s.cas.touch_object(&old);
    assert!(age_of(&s.cas.object_path(&old)) < Duration::from_secs(60));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(s.cas.object_path(&old))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o222, 0, "the object stays read-only");
    }

    let recent = object(&s, &[2u8; 100], Duration::from_secs(600));
    s.cas.touch_object(&recent);
    let age = age_of(&s.cas.object_path(&recent));
    assert!(
        age >= Duration::from_secs(590),
        "touched within the hour: unchanged"
    );
}

/// A real cache hit: a second build in a fresh target directory restores every unit, and
/// the objects it restored are young again.
#[test]
fn a_build_that_hits_the_cache_marks_its_objects_used() {
    let dir = TempDir::new().unwrap();
    let proj = dir.path().join("proj");
    fs::create_dir_all(proj.join("src")).unwrap();
    fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname = \"lru\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(proj.join("src/main.rs"), "fn main() {}\n").unwrap();
    let cache = dir.path().join("cache");
    let build = |target: &str| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(["build", "--cache-dir"])
            .arg(&cache)
            .arg("--target-dir")
            .arg(dir.path().join(target))
            .current_dir(&proj)
            .env("CARGO_HOME", dir.path().join("cargo-home"))
            // The project's own crates go to the shared cache only when asked.
            .env("MITTS_SHARE_WORKSPACE_CRATES", "1")
            .env_remove("MITTS_CACHE_DIR")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build("t1");
    let objects: Vec<_> = fs::read_dir(cache.join("cas/objects"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(!objects.is_empty());
    for o in &objects {
        set_age(o, 5 * HOUR);
    }
    build("t2");
    for o in &objects {
        assert!(
            age_of(o) < Duration::from_secs(600),
            "{o:?} not marked used"
        );
    }
}

/// Two projects whose binaries share an action id (`app@0.1.0::bin:app`) are not versions of
/// one target: each keeps its latest, and neither is pruned ahead of the LRU order.
#[test]
fn same_named_targets_of_two_projects_are_not_versions_of_each_other() {
    let s = store();
    let a = object(&s, &[1u8; 1000], 6 * HOUR);
    entry_in(
        &s,
        "project-a",
        "a1",
        "app@0.1.0::bin:app",
        &a,
        true,
        6 * HOUR,
    );
    let b = object(&s, &[2u8; 1000], 3 * HOUR);
    entry_in(
        &s,
        "project-b",
        "b1",
        "app@0.1.0::bin:app",
        &b,
        true,
        3 * HOUR,
    );
    // Entries of older Mitts versions have no scope: never treated as superseded.
    let legacy = object(&s, &[3u8; 1000], 9 * HOUR);
    let legacy_new = object(&s, &[4u8; 1000], 1 * HOUR);
    for (key, hash, age) in [("l1", &legacy, 9 * HOUR), ("l2", &legacy_new, HOUR)] {
        let mut e = ActionCacheEntry {
            action_key: key.to_string(),
            action_id: "old@0.1.0::bin:old".to_string(),
            created_at_secs: (SystemTime::now() - age)
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            outputs: vec![],
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            target_scopes: Vec::new(),
        };
        e.outputs.push(CachedOutput {
            relative_path: "debug/old".into(),
            cas_hash: hash.to_string(),
            file_size: 1000,
            executable: true,
        });
        s.ac.put(&e).unwrap();
    }

    let superseded = eviction::superseded_executables(&s.ac, |h| {
        fs::metadata(s.cas.object_path(h)).ok()?.modified().ok()
    });
    assert!(superseded.is_empty(), "{superseded:?}");
}

/// Pruning an object never changes the mode of its inode: a workspace may still hold a
/// store hardlink to it, and an executable there must stay executable.
#[cfg(unix)]
#[test]
fn pruning_leaves_the_mode_of_a_linked_inode_alone() {
    use std::os::unix::fs::PermissionsExt;
    let s = store();
    let exe = object(&s, &[7u8; 1000], 5 * HOUR);
    let path = s.cas.object_path(&exe);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
    let workspace_copy = s.root.join("app");
    fs::hard_link(&path, &workspace_copy).unwrap();

    CacheGc::new(&s.cas, &s.ac).prune(&limit(0)).unwrap();

    assert!(!s.cas.has_object(&exe), "no build holds the cache: pruned");
    let mode = fs::metadata(&workspace_copy).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o555, "the workspace link keeps its mode");
}

/// Keys are path-independent: project B hits the entry project A wrote, then A builds a
/// newer version. The shared old version is still B's latest and is not superseded; once
/// B has a newer version too, it is. A hash that an entry without scopes (older Mitts)
/// also has is never superseded.
#[test]
fn a_version_another_project_hit_stays_until_it_is_old_there_too() {
    let s = store();
    let shared = object(&s, &[1u8; 1000], 6 * HOUR);
    entry_in(
        &s,
        "project-a",
        "v1",
        "app@0.1.0::bin:app",
        &shared,
        true,
        6 * HOUR,
    );
    let v1 = s.ac.get("v1").unwrap().unwrap();
    s.ac.record_scope(&v1, "project-b".to_string());
    assert_eq!(
        s.ac.get("v1").unwrap().unwrap().target_scopes,
        ["project-a", "project-b"]
    );
    let a2 = object(&s, &[2u8; 1000], 3 * HOUR);
    entry_in(
        &s,
        "project-a",
        "a2",
        "app@0.1.0::bin:app",
        &a2,
        true,
        3 * HOUR,
    );
    let superseded = |s: &Store| {
        eviction::superseded_executables(&s.ac, |h| {
            fs::metadata(s.cas.object_path(h)).ok()?.modified().ok()
        })
    };
    assert!(superseded(&s).is_empty(), "still B's latest");

    let b2 = object(&s, &[3u8; 1000], 2 * HOUR);
    entry_in(
        &s,
        "project-b",
        "b2",
        "app@0.1.0::bin:app",
        &b2,
        true,
        2 * HOUR,
    );
    assert!(superseded(&s).contains(&shared), "old in A and in B");

    // The same bytes in an entry an older Mitts wrote: kept out of the superseded set.
    s.ac.put(&ActionCacheEntry {
        action_key: "legacy".to_string(),
        action_id: "app@0.1.0::bin:app".to_string(),
        created_at_secs: 0,
        outputs: v1.outputs.clone(),
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        target_scopes: Vec::new(),
    })
    .unwrap();
    assert!(!superseded(&s).contains(&shared));
}

/// A second checkout that hits the first one's entry adds its own scope to it.
#[test]
fn a_hit_from_a_second_checkout_records_its_project() {
    let dir = TempDir::new().unwrap();
    let cache = dir.path().join("cache");
    let build = |name: &str| {
        let proj = dir.path().join(name);
        fs::create_dir_all(proj.join("src")).unwrap();
        fs::write(
            proj.join("Cargo.toml"),
            "[package]\nname = \"scoped\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(proj.join("src/main.rs"), "fn main() {}\n").unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mitts"))
            .args(["build", "--cache-dir"])
            .arg(&cache)
            .current_dir(&proj)
            .env("CARGO_HOME", dir.path().join("cargo-home"))
            .env("MITTS_SHARE_WORKSPACE_CRATES", "1")
            .env_remove("MITTS_CACHE_DIR")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    build("one");
    build("two");
    let ac = ActionCache::new(cache.join("ac")).unwrap();
    let bin = fs::read_dir(ac.root())
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let key = p.file_stem()?.to_str()?.to_string();
            p.extension().is_some_and(|x| x == "json").then_some(key)
        })
        .filter_map(|k| ac.get(&k).ok().flatten())
        .find(|e| e.action_id.contains("::bin:scoped"))
        .expect("the binary's entry");
    assert_eq!(bin.target_scopes.len(), 2, "{:?}", bin.target_scopes);
}
