/*
 * Mitts Build — tests for version selection during a lock update
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Issue 140: adding one dependency moved `rsa 0.10.0-rc.19` to `0.10.0`, which lacks the
//! `sha2` feature crypto-glue asks for, and left the lock unresolvable.

use super::*;

fn entry(vers: &str, features: &[&str], features2: &[&str], yanked: bool) -> SparseIndexEntry {
    let table = |names: &[&str]| {
        names
            .iter()
            .map(|n| (n.to_string(), Vec::new()))
            .collect::<HashMap<_, _>>()
    };
    SparseIndexEntry {
        rust_version: None,
        name: "rsa".into(),
        vers: vers.into(),
        deps: Vec::new(),
        cksum: String::new(),
        yanked,
        features: table(features),
        features2: table(features2),
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_locked_version_that_still_fits_is_kept() {
    let entries = [
        entry("0.10.0-rc.19", &["sha2"], &[], false),
        entry("0.10.0", &["sha2"], &[], false),
    ];
    let picked = pick_version(
        &entries,
        &strings(&["0.10.0-rc.19"]),
        &[],
        &strings(&["0.10.0-rc.19"]),
        None,
    );
    assert_eq!(picked.unwrap().vers, "0.10.0-rc.19");
    // Without a lock the highest version wins.
    let picked = pick_version(&entries, &strings(&["0.10.0-rc.19"]), &[], &[], None);
    assert_eq!(picked.unwrap().vers, "0.10.0");
    // A locked version the requirement no longer allows is not kept.
    let picked = pick_version(
        &entries,
        &strings(&["=0.10.0"]),
        &[],
        &strings(&["0.10.0-rc.19"]),
        None,
    );
    assert_eq!(picked.unwrap().vers, "0.10.0");
}

#[test]
fn a_locked_yanked_version_is_kept_but_never_chosen_fresh() {
    let entries = [
        entry("1.0.1", &[], &[], true),
        entry("1.0.0", &[], &[], false),
    ];
    assert_eq!(
        pick_version(&entries, &strings(&["1"]), &[], &strings(&["1.0.1"]), None)
            .unwrap()
            .vers,
        "1.0.1"
    );
    assert_eq!(
        pick_version(&entries, &strings(&["1"]), &[], &[], None)
            .unwrap()
            .vers,
        "1.0.0"
    );
}

#[test]
fn a_version_without_a_requested_feature_is_skipped() {
    let entries = [
        entry("0.10.0-rc.19", &["sha2"], &[], false),
        entry("0.10.0", &["std"], &[], false),
    ];
    let picked = pick_version(
        &entries,
        &strings(&["0.10.0-rc.19"]),
        &strings(&["sha2"]),
        &[],
        None,
    );
    assert_eq!(picked.unwrap().vers, "0.10.0-rc.19");
    assert!(pick_version(
        &entries,
        &strings(&["=0.10.0"]),
        &strings(&["sha2"]),
        &[],
        None
    )
    .is_none());
    // `default` is always acceptable.
    assert!(pick_version(
        &entries,
        &strings(&["=0.10.0"]),
        &strings(&["default"]),
        &[],
        None
    )
    .is_some());
}

#[test]
fn fallback_prefers_versions_the_rust_limit_supports() {
    let mut e1 = entry("1.0.0", &[], &[], false);
    e1.rust_version = Some("1.70".into());
    let mut e2 = entry("1.1.0", &[], &[], false);
    e2.rust_version = Some("1.90".into());
    let entries = [e1, e2];
    let pick = |limit| {
        pick_version(&entries, &strings(&["1"]), &[], &[], limit)
            .unwrap()
            .vers
            .clone()
    };
    assert_eq!(pick(None), "1.1.0", "allow: the highest");
    assert_eq!(
        pick(Some((1, 80, 0))),
        "1.0.0",
        "fallback: the highest that fits"
    );
    assert_eq!(pick(Some((1, 60, 0))), "1.1.0", "none fits: the highest");
    // A version without rust-version always fits.
    let mut plain = entry("1.2.0", &[], &[], false);
    plain.rust_version = None;
    let entries = [entries[0].clone(), entries[1].clone(), plain];
    let picked = pick_version(&entries, &strings(&["1"]), &[], &[], Some((1, 60, 0)));
    assert_eq!(picked.unwrap().vers, "1.2.0");
}

#[test]
fn features_published_in_features2_count() {
    let e = entry("1.0.0", &[], &["serde"], false);
    assert!(has_features(&e, &strings(&["serde"])));
    assert!(all_features(&e).contains_key("serde"));
}
