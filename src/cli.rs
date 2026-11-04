/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

const MITTS_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("MITTS_GIT_SHA"), ")");

#[derive(Parser, Debug)]
#[command(name = "mitts")]
#[command(author = "Alexander Panasenko <alex@prod.codes>")]
#[command(version = MITTS_VERSION)]
#[command(
    about = "Rust-first build and package tooling with a shared artifact cache",
    after_help = "Environment Variables:\n  MITTS_JOBS=N          Limit concurrent native build units to a positive integer\n  MITTS_TIMINGS=1       Enable sub-millisecond phase timings and I/O metrics on stderr\n  MITTS_NO_REFLINK=1    Disable CoW reflink (FICLONE/clonefile) to force byte copying\n  MITTS_NO_HARDLINK=1   Disable intra-workspace zero-copy hardlinks\n  MITTS_VERIFY_RESTORE=1 Re-hash every cache object before restoring it\n  MITTS_CACHE_IO_BUDGET=N Cache readers allowed at once (0 = unbounded; default 1 on macOS)\n  MITTS_STORE_HARDLINKS=0|1 Store objects reach target/ by reflink, else read-only hardlink (default); 0 = copy instead, 1 = always link\n  MITTS_GIT=<path>      git binary used to clone git dependencies (default: git on PATH)\n  MITTS_INDEX_TTL_SECS=N  How long cached sparse-index entries are trusted when online (default 900)\n  MITTS_FAST_LOCK=0|1   Fast build lock with an explicit --cache-dir (default: on only for the default cache root)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Compile the local package and its dependencies into the isolated Mitts target
    #[command(
        after_help = "Set MITTS_JOBS=N to limit concurrent native build units. N must be a positive integer."
    )]
    Build(BuildArgs),

    /// Run native rustc queries for the selected package target
    Rustc(BuildArgs),

    /// Check a package to catch errors without producing binary artifacts
    #[command(
        after_help = "Set MITTS_JOBS=N to limit concurrent native build units. N must be a positive integer."
    )]
    Check(CheckArgs),

    /// Run Clippy lints over local packages in the workspace
    #[command(
        after_help = "Set MITTS_JOBS=N to limit concurrent native build units. N must be a positive integer."
    )]
    Clippy(ClippyArgs),

    /// Format Rust source files in the workspace using rustfmt
    Fmt(FmtArgs),

    /// Execute unit and integration tests of a package
    #[command(
        after_help = "Set MITTS_JOBS=N to limit concurrent native build units. N must be a positive integer."
    )]
    Test(TestArgs),

    /// Generate code coverage using LLVM source-based coverage tools
    Coverage(CoverageArgs),

    /// Cargo-compatible llvm-cov wrapper command
    LlvmCov(CoverageArgs),

    /// Run a binary of the local package
    Run(RunArgs),

    /// Explain the effective build plan, compilation units, and cache eligibility
    Explain(ExplainArgs),

    /// Print a target-filtered package and dependency graph as JSON
    #[command(
        after_help = "Format 1 includes packages, workspace members, resolve.root, resolve.nodes, dependency kinds, and selected features. The graph describes active native build units for the requested platform and features."
    )]
    Metadata(MetadataArgs),

    /// Fetch dependencies into the global source store
    Fetch(FetchArgs),

    /// Create a source package archive
    #[command(
        after_help = "Git packages currently require a clean repository with the package at its root. Include/exclude patterns are supported; include can select ignored files. Nested workspace packages and symlinked directories are not yet supported.\n\nExamples:\n  mitts package --locked\n  mitts package --offline --cache-dir /path/to/cache\n  mitts package --no-verify"
    )]
    Package(PackageArgs),

    /// Publish a source package to crates.io
    #[command(
        after_help = "Publishes to crates.io using CARGO_REGISTRY_TOKEN. The default operation creates and verifies a package; --archive accepts a previously prepared archive.\n\nExamples:\n  mitts publish --dry-run\n  mitts publish --locked\n  mitts publish --archive target/package/name-version.crate"
    )]
    Publish(PublishArgs),

    /// Prune old or unreferenced CAS objects according to storage quotas
    Gc(GcArgs),

    /// Remove generated artifacts and fast build lock for the workspace
    Clean(CleanArgs),
}

#[derive(Args, Debug, Clone, Default)]
pub struct CleanArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Directory for all generated artifacts
    #[arg(long)]
    pub target_dir: Option<PathBuf>,

    /// Clean release artifacts only
    #[arg(short, long)]
    pub release: bool,

    /// Clean the shared global artifact and action cache as well
    #[arg(long)]
    pub cache: bool,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct BuildArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package to build
    #[arg(skip)]
    pub package: Option<String>,

    /// Packages to build (repeat for multiple packages)
    #[arg(short = 'p', long = "package", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Build artifacts in release mode, with optimizations
    #[arg(short, long)]
    pub release: bool,

    /// Directory for all generated artifacts
    #[arg(long)]
    pub target_dir: Option<PathBuf>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Do not read from or write to the shared action cache
    #[arg(long)]
    pub no_cache: bool,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing compatible Cargo.lock; do not resolve or write one.
    #[arg(long)]
    pub locked: bool,

    /// Do not use the network to resolve dependencies; use only local sources.
    #[arg(long)]
    pub offline: bool,

    /// Require a compatible Cargo.lock and use only locally available sources.
    #[arg(long)]
    pub frozen: bool,

    /// Select target triple; outputs go under target/<triple>
    #[arg(long)]
    pub target: Option<String>,

    /// Build a named example target (repeatable; output under target/<profile>/examples)
    #[arg(long = "example", action = clap::ArgAction::Append)]
    pub example: Vec<String>,

    /// Suppress Mitts progress and success summaries; keep errors and program output
    #[arg(short, long)]
    pub quiet: bool,

    /// Override library crate types (for example, staticlib; use with --lib)
    #[arg(long = "crate-type", action = clap::ArgAction::Append)]
    pub crate_types: Vec<String>,

    /// Keep the fast build lock active with an explicit cache directory (API only)
    #[arg(skip)]
    pub fast_lock: Option<bool>,

    /// Force native rustc DAG compilation (zero intermediate target bloat)
    #[arg(long)]
    pub native: bool,

    /// Force fallback to Cargo execution engine
    #[arg(long)]
    pub cargo: bool,

    /// Use verbose output (-v, -vv for very verbose)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override a configuration value (not yet supported)
    #[arg(long, action = clap::ArgAction::Append)]
    pub config: Vec<String>,

    /// Exclude packages from being built
    #[arg(long, value_name = "SPEC", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Build the named binary target (repeatable; combines with other target selectors)
    #[arg(long = "bin", action = clap::ArgAction::Append)]
    pub bin: Vec<String>,

    /// Build all binary targets
    #[arg(long)]
    pub bins: bool,

    /// Build only this package's library
    #[arg(long)]
    pub lib: bool,

    /// Additional arguments passed to the underlying builder
    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct RunArgs {
    #[command(flatten)]
    pub build_args: BuildArgs,

    /// Arguments for the binary
    #[arg(skip)]
    pub trailing_args: Vec<String>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct ExplainArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package to explain
    #[arg(skip)]
    pub package: Option<String>,
    /// Packages to explain (repeat for multiple packages)
    #[arg(short = 'p', long = "package", action = clap::ArgAction::Append)]
    pub packages: Vec<String>,

    /// Check release mode profile
    #[arg(short, long)]
    pub release: bool,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Output explanation in JSON format
    #[arg(long)]
    pub json: bool,

    /// Write derived compilation unit ActionKeys (JSON) to a file
    #[arg(long, value_name = "FILE")]
    pub keys: Option<PathBuf>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct MetadataArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Package whose dependency graph should be selected
    #[arg(short, long)]
    pub package: Option<String>,

    /// Space or comma separated list of features to activate
    #[arg(short = 'F', long, value_delimiter = ',', num_args = 1..)]
    pub features: Vec<String>,

    /// Do not activate the `default` feature
    #[arg(long)]
    pub no_default_features: bool,

    /// Activate all available features
    #[arg(long)]
    pub all_features: bool,

    /// Filter dependencies for a target triple
    #[arg(long)]
    pub filter_platform: Option<String>,

    /// Require an existing compatible Cargo.lock
    #[arg(long)]
    pub locked: bool,

    /// Resolve using local index and source data only
    #[arg(long)]
    pub offline: bool,

    /// Combine locked and offline resolution
    #[arg(long)]
    pub frozen: bool,

    /// Metadata JSON format version
    #[arg(long, default_value_t = 1)]
    pub format_version: u32,

    /// Directory for the shared artifact and source cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,
}

#[derive(Args, Debug, Clone, Default)]
pub struct FetchArgs {
    /// Path to Cargo.toml
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the global source store
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Run in offline mode without network access
    #[arg(long)]
    pub offline: bool,

    /// Require Cargo.lock and cache to be up to date
    #[arg(long)]
    pub locked: bool,

    /// Require Cargo.lock to be unchanged
    #[arg(long)]
    pub frozen: bool,
}

#[derive(Args, Debug, Clone, Default)]
pub struct GcArgs {
    /// Directory for the shared artifact and action cache
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Maximum cache size in bytes (pruning oldest until under quota)
    #[arg(long)]
    pub max_bytes: Option<u64>,

    /// Prune CAS objects older than specified seconds
    #[arg(long)]
    pub max_age_secs: Option<u64>,
}

pub mod build;
pub mod check;
pub mod clippy;
pub mod coverage;
pub mod coverage_tools;
pub mod coverage_types;
pub mod diagnostic;
pub mod fmt;
pub mod handlers;
pub mod metadata;
mod package;
mod package_archive;
mod package_directory;
mod package_lock;
mod package_patterns;
mod registry_metadata;
mod registry_protocol;
mod registry_upload;
pub mod run;
pub mod rustc;
pub mod terminal;
pub mod test;
pub mod test_doctest;
mod test_progress;
pub mod test_runner;
pub mod ui;

pub use build::handle_build;
pub use check::{handle_check, CheckArgs};
pub use clippy::{handle_clippy, ClippyArgs};
pub use coverage::{handle_coverage, CoverageArgs};
pub use diagnostic::render_compiler_diagnostics;
pub use fmt::{handle_fmt, FmtArgs};
pub use handlers::{handle_clean, handle_explain, handle_fetch, handle_gc};
pub use metadata::handle_metadata;
pub use package::{handle_package, handle_publish, PackageArgs, PublishArgs};
pub use run::handle_run;
pub use rustc::handle_rustc;
pub use terminal::TerminalGuard;
pub use test::{handle_test, TestArgs};
pub use ui::Ui;
