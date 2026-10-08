/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256 as CoreSha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Hardware-accelerated SHA-256 implementation adhering to FIPS 180-4.
#[derive(Clone, Debug)]
pub struct Sha256 {
    hasher: CoreSha256,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// Creates a new SHA-256 context.
    pub fn new() -> Self {
        Self {
            hasher: CoreSha256::new(),
        }
    }

    /// Feeds input bytes into the hasher.
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    /// Finalizes the hash calculation and returns 32 raw bytes.
    pub fn finalize(self) -> [u8; 32] {
        self.hasher.finalize().into()
    }

    /// Computes the hex-encoded SHA-256 digest of an in-memory byte slice.
    pub fn digest(data: &[u8]) -> String {
        let mut hasher = Self::new();
        hasher.update(data);
        bytes_to_hex(&hasher.finalize())
    }

    /// Computes the hex-encoded SHA-256 digest of a reader in 64KB chunks.
    pub fn digest_reader<R: Read>(mut reader: R) -> Result<String> {
        let mut hasher = Self::new();
        let mut buffer = [0u8; 65536];
        loop {
            let bytes_read = reader
                .read(&mut buffer)
                .context("Failed to read from stream")?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }
        Ok(bytes_to_hex(&hasher.finalize()))
    }

    /// Computes the hex-encoded SHA-256 digest of a file on disk.
    pub fn digest_file(path: &Path) -> Result<String> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open file for hashing: {:?}", path))?;
        Self::digest_reader(file)
    }

    /// Computes the hex-encoded SHA-256 digest of a file or directory on disk.
    /// If the path is a directory, recursively walks and hashes all entries deterministically.
    pub fn digest_path_recursive(path: &Path) -> Result<String> {
        if !path.exists() {
            bail!("Path does not exist: {:?}", path);
        }
        if path.is_file() {
            return Self::digest_file(path);
        }
        let mut entries = Vec::new();
        fn collect_entries(
            base: &Path,
            current: &Path,
            list: &mut Vec<(String, PathBuf)>,
        ) -> Result<()> {
            for entry in fs::read_dir(current)
                .with_context(|| format!("Failed to read dir {:?}", current))?
            {
                let entry = entry?;
                let p = entry.path();
                if p.is_dir() {
                    collect_entries(base, &p, list)?;
                } else if p.is_file() {
                    let rel = p
                        .strip_prefix(base)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .to_string();
                    list.push((rel, p));
                }
            }
            Ok(())
        }
        collect_entries(path, path, &mut entries)?;
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = Self::new();
        for (rel_path, file_path) in entries {
            hasher.update(rel_path.as_bytes());
            hasher.update(b":");
            let file_digest = Self::digest_file(&file_path)?;
            hasher.update(file_digest.as_bytes());
            hasher.update(b";");
        }
        Ok(bytes_to_hex(&hasher.finalize()))
    }
}

impl Write for Sha256 {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Converts binary bytes to a lowercase hexadecimal string without allocations in the loop.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX_CHARS[(b >> 4) as usize]);
        out.push(HEX_CHARS[(b & 0x0f) as usize]);
    }
    // Safety: HEX_CHARS consists exclusively of valid ASCII/UTF-8 bytes
    unsafe { String::from_utf8_unchecked(out) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nist_empty_string() {
        assert_eq!(
            Sha256::digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_nist_abc() {
        assert_eq!(
            Sha256::digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_nist_long_sequence() {
        assert_eq!(
            Sha256::digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
