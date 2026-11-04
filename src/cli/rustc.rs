/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::build::handle_build;
use super::BuildArgs;

pub fn handle_rustc(args: &BuildArgs) -> i32 {
    if !args.lib {
        eprintln!("error: `mitts rustc` currently requires --lib");
        return 101;
    }
    let mut native_args = args.clone();
    native_args.native = true;
    native_args.cargo = false;
    native_args.quiet = true;
    handle_build(&native_args)
}
