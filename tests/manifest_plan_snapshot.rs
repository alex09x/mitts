/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::inputs::hash_package_common_inputs;
use mitts::adapter::rust::key::{derive_unit_action_keys, parallel_map_chunks};
use mitts::adapter::rust::manifest::CargoToml;
use mitts::adapter::rust::manifest_snapshot::ManifestSnapshot;
use mitts::adapter::{RustAdapter, RustBuildOptions};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn manifest(version: &str) -> String {
    format!("[package]\nname = \"snapshot_case\"\nversion = \"{version}\"\nedition = \"2021\"\n")
}

fn project(root: &Path) -> RustBuildOptions {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), manifest("0.1.0")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "pub const V: &str = env!(\"CARGO_PKG_VERSION\");",
    )
    .unwrap();
    RustBuildOptions {
        manifest_path: Some(root.join("Cargo.toml")),
        cache_dir: Some(root.join("isolated-cache")),
        target_dir: Some(root.join("target")),
        ..Default::default()
    }
}

#[test]
fn snapshot_shares_bytes_and_digests_across_workers_but_rejects_disk_changes() {
    let temp = TempDir::new().unwrap();
    let opt = project(temp.path());
    let path = opt.manifest_path.unwrap();
    let expected = hash_package_common_inputs(temp.path(), None, false).unwrap();
    let snapshot = ManifestSnapshot::default();
    {
        let _scope = snapshot.enter();
        assert_eq!(
            CargoToml::from_file(&path)
                .unwrap()
                .package
                .unwrap()
                .version
                .as_deref(),
            Some("0.1.0")
        );
        assert_eq!(
            hash_package_common_inputs(temp.path(), None, false).unwrap(),
            expected
        );
        // Same-length bytes: freshness must not rely on size (or timestamps).
        fs::write(&path, manifest("0.2.0")).unwrap();
        let versions = parallel_map_chunks(&(0..48).collect::<Vec<_>>(), |_| {
            let parsed = CargoToml::from_file(&path).unwrap();
            assert_eq!(
                hash_package_common_inputs(temp.path(), None, false).unwrap(),
                expected
            );
            parsed.package.unwrap().version.unwrap()
        });
        assert!(versions.iter().all(|v| v == "0.1.0"));
        assert_eq!(snapshot.captured_file_count(), 1);
        assert!(snapshot
            .verify_fresh()
            .unwrap_err()
            .to_string()
            .contains("Manifest changed"));
    }
    assert_eq!(
        CargoToml::from_file(&path)
            .unwrap()
            .package
            .unwrap()
            .version
            .as_deref(),
        Some("0.2.0")
    );
    assert_ne!(
        hash_package_common_inputs(temp.path(), None, false).unwrap(),
        expected
    );
    let fresh = ManifestSnapshot::default();
    let _scope = fresh.enter();
    CargoToml::from_file(&path).unwrap();
    fresh.verify_fresh().unwrap();
}

#[test]
fn snapshot_tracks_inherited_bytes_and_previously_absent_ancestors() {
    let temp = TempDir::new().unwrap();
    let member = temp.path().join("member");
    fs::create_dir_all(&member).unwrap();
    let path = member.join("Cargo.toml");
    fs::write(
        &path,
        "[package]\nname = \"member\"\nversion.workspace = true\nedition.workspace = true\n",
    )
    .unwrap();
    let absent = ManifestSnapshot::default();
    {
        let _scope = absent.enter();
        CargoToml::from_file(&path).unwrap();
    }
    let workspace = "[workspace]\nmembers = [\"member\"]\n[workspace.package]\nversion = \"1.0.0\"\nedition = \"2021\"\n";
    fs::write(temp.path().join("Cargo.toml"), workspace).unwrap();
    assert!(absent.verify_fresh().is_err());
    let inherited = ManifestSnapshot::default();
    {
        let _scope = inherited.enter();
        assert_eq!(
            CargoToml::from_file(&path)
                .unwrap()
                .package
                .unwrap()
                .version
                .as_deref(),
            Some("1.0.0")
        );
        assert_eq!(inherited.captured_file_count(), 2);
    }
    fs::write(
        temp.path().join("Cargo.toml"),
        workspace.replace("1.0.0", "2.0.0"),
    )
    .unwrap();
    assert!(inherited.verify_fresh().is_err());
    assert_eq!(
        CargoToml::from_file(&path)
            .unwrap()
            .package
            .unwrap()
            .version
            .as_deref(),
        Some("2.0.0")
    );
}

#[test]
fn changed_manifest_rejects_old_plan_and_next_plan_has_new_keys() {
    let temp = TempDir::new().unwrap();
    let opt = project(temp.path());
    let first = RustAdapter::explain_plan(&opt).unwrap();
    let first_keys = derive_unit_action_keys(&opt, &first).unwrap();
    let json = serde_json::to_value(&first).unwrap();
    assert!(json.get("manifest_snapshot").is_none());
    fs::write(opt.manifest_path.as_ref().unwrap(), manifest("0.2.0")).unwrap();
    assert!(derive_unit_action_keys(&opt, &first)
        .unwrap_err()
        .to_string()
        .contains("Manifest changed"));
    let next = RustAdapter::explain_plan(&opt).unwrap();
    assert!(next.units.iter().all(|u| u.version == "0.2.0"));
    assert_ne!(derive_unit_action_keys(&opt, &next).unwrap(), first_keys);
}

#[test]
fn builds_reuse_warm_cache_and_invalidate_changed_manifest() {
    let temp = TempDir::new().unwrap();
    let opt = project(temp.path());
    let (_, cold) = RustAdapter::build_native(&opt).unwrap();
    assert_eq!(cold.exit_code, 0, "{}", cold.stderr);
    let (_, warm) = RustAdapter::build_native(&opt).unwrap();
    assert_eq!(warm.exit_code, 0, "{}", warm.stderr);
    fs::write(opt.manifest_path.as_ref().unwrap(), manifest("0.2.0")).unwrap();
    let (_, changed) = RustAdapter::build_native(&opt).unwrap();
    assert_eq!(changed.exit_code, 0, "{}", changed.stderr);
    assert!(!cold.cached);
    assert!(warm.cached, "warm build must reuse the cached artifact");
    assert!(
        !changed.cached,
        "manifest version must invalidate compiled env! metadata"
    );
}

#[test]
fn nested_scope_restores_its_parent_after_unwinding() {
    let temp = TempDir::new().unwrap();
    let opt = project(temp.path());
    let path = opt.manifest_path.unwrap();
    let outer = ManifestSnapshot::default();
    let _scope = outer.enter();
    CargoToml::from_file(&path).unwrap();
    fs::write(&path, manifest("0.2.0")).unwrap();
    let inner = ManifestSnapshot::default();
    let panic = std::panic::catch_unwind(|| {
        let _scope = inner.enter();
        assert_eq!(
            CargoToml::from_file(&path)
                .unwrap()
                .package
                .unwrap()
                .version
                .as_deref(),
            Some("0.2.0")
        );
        panic!("exercise scope cleanup");
    });
    assert!(panic.is_err());
    assert_eq!(
        CargoToml::from_file(&path)
            .unwrap()
            .package
            .unwrap()
            .version
            .as_deref(),
        Some("0.1.0")
    );
    assert!(outer.verify_fresh().is_err());
    inner.verify_fresh().unwrap();
}

#[test]
fn immutable_package_hash_uses_the_same_captured_bytes_as_the_parser() {
    let temp = TempDir::new().unwrap();
    let pkg = temp.path().join("sources/snapshot_case-0.1.0");
    let opt = project(&pkg);
    fs::write(pkg.join(mitts::core::source_store::MITTS_OK_MARKER), "").unwrap();
    let path = opt.manifest_path.unwrap();
    let original = manifest("0.1.0");
    let snapshot = ManifestSnapshot::default();
    let original_inputs;
    {
        let _scope = snapshot.enter();
        CargoToml::from_file(&path).unwrap();
        fs::write(&path, format!("{original}# changed registry bytes\n")).unwrap();
        original_inputs = hash_package_common_inputs(&pkg, None, false).unwrap();
        assert_eq!(
            original_inputs[0].1,
            mitts::core::hash::Sha256::digest(original.as_bytes())
        );
        assert!(original_inputs
            .iter()
            .any(|(name, _)| name == "immutable_pkg"));
        assert_eq!(snapshot.captured_file_count(), 1);
        assert!(snapshot.verify_fresh().is_err());
    }
    assert_ne!(
        hash_package_common_inputs(&pkg, None, false).unwrap(),
        original_inputs
    );
}
