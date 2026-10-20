/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::thread;

/// Runs bounded chunks while sharing the caller's invocation-scoped manifests.
pub fn parallel_map_chunks<T, R, F>(items: &[T], f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    if items.is_empty() {
        return Vec::new();
    }
    let num_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8)
        .min(items.len());
    if num_threads <= 1 {
        return items.iter().map(f).collect();
    }
    let chunk_size = items.len().div_ceil(num_threads);
    let chunks: Vec<&[T]> = items.chunks(chunk_size).collect();
    let f = &f;
    let snapshot = super::super::manifest_snapshot::current();
    thread::scope(|s| {
        let mut handles = Vec::new();
        for chunk in chunks {
            let snapshot = snapshot.clone();
            handles.push(s.spawn(move || {
                let _scope = snapshot.as_ref().map(|snapshot| snapshot.enter());
                chunk.iter().map(f).collect::<Vec<R>>()
            }));
        }
        let mut all = Vec::with_capacity(items.len());
        for handle in handles {
            match handle.join() {
                Ok(result) => all.extend(result),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        all
    })
}
