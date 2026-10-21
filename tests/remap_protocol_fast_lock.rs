/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use mitts::adapter::rust::builder::compute_options_hash;
use mitts::adapter::rust::types::RustBuildOptions;
use std::path::{Path, PathBuf};

/// Golden options signature from 0.3.3: an old workspace fast lock must not
/// bypass the new remap protocol even when every user option is unchanged.
#[test]
fn remap_protocol_invalidates_legacy_fast_lock_options() {
    let options = RustBuildOptions {
        native: true,
        cache_dir: Some(PathBuf::from("/workspace/cache")),
        ..Default::default()
    };
    let signature = compute_options_hash(
        &options,
        Path::new("/workspace/project/target"),
        Path::new("/workspace/project/Cargo.toml"),
    );
    assert_ne!(
        signature,
        "14dd7b2c51b5d08cf1a517ded78666979cdf157edf4a2eb001a8e4c3c45d896e"
    );
}
