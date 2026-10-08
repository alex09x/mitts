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
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Standard SHA-256 round constants (first 32 bits of the fractional parts
/// of the cube roots of the first 64 prime numbers).
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Initial hash state (first 32 bits of the fractional parts of the square
/// roots of the first 8 prime numbers).
const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

#[inline]
fn rotr(x: u32, n: u32) -> u32 {
    x.rotate_right(n)
}

#[inline]
fn ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (!x & z)
}

#[inline]
fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (x & z) ^ (y & z)
}

#[inline]
fn sigma0(x: u32) -> u32 {
    rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22)
}

#[inline]
fn sigma1(x: u32) -> u32 {
    rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25)
}

#[inline]
fn gamma0(x: u32) -> u32 {
    rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3)
}

#[inline]
fn gamma1(x: u32) -> u32 {
    rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10)
}

/// Pure-Rust standard SHA-256 implementation adhering to FIPS 180-4.
#[derive(Clone, Debug)]
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len_bytes: u64,
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
            state: H_INIT,
            buffer: [0u8; 64],
            buffer_len: 0,
            total_len_bytes: 0,
        }
    }

    /// Feeds input bytes into the hasher.
    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len_bytes = self.total_len_bytes.wrapping_add(data.len() as u64);

        if self.buffer_len > 0 {
            let to_fill = 64 - self.buffer_len;
            if data.len() < to_fill {
                self.buffer[self.buffer_len..self.buffer_len + data.len()].copy_from_slice(data);
                self.buffer_len += data.len();
                return;
            } else {
                self.buffer[self.buffer_len..64].copy_from_slice(&data[..to_fill]);
                Self::process_block(&mut self.state, &self.buffer);
                self.buffer_len = 0;
                data = &data[to_fill..];
            }
        }

        while data.len() >= 64 {
            let block: &[u8; 64] = data[..64]
                .try_into()
                .expect("slice has guaranteed 64-byte length");
            Self::process_block(&mut self.state, block);
            data = &data[64..];
        }

        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    fn process_block(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, item) in w.iter_mut().take(16).enumerate() {
            let offset = i * 4;
            *item = u32::from_be_bytes([
                block[offset],
                block[offset + 1],
                block[offset + 2],
                block[offset + 3],
            ]);
        }

        for i in 16..64 {
            let s0 = gamma0(w[i - 15]);
            let s1 = gamma1(w[i - 2]);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];

        for i in 0..64 {
            let t1 = h
                .wrapping_add(sigma1(e))
                .wrapping_add(ch(e, f, g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = sigma0(a).wrapping_add(maj(a, b, c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    /// Finalizes the hash calculation and returns 32 raw bytes.
    pub fn finalize(mut self) -> [u8; 32] {
        let total_bits = self.total_len_bytes.wrapping_mul(8);

        // Append bit '1' (0x80 byte)
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;

        if self.buffer_len > 56 {
            for i in self.buffer_len..64 {
                self.buffer[i] = 0;
            }
            Self::process_block(&mut self.state, &self.buffer);
            self.buffer = [0u8; 64];
            self.buffer_len = 0;
        }

        for i in self.buffer_len..56 {
            self.buffer[i] = 0;
        }

        let bit_len_bytes = total_bits.to_be_bytes();
        self.buffer[56..64].copy_from_slice(&bit_len_bytes);
        Self::process_block(&mut self.state, &self.buffer);

        let mut output = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            let bytes = word.to_be_bytes();
            output[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
        }
        output
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
