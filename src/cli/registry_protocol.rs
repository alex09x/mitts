/*
 * Mitts Build — crates.io registry protocol
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io::{self, Write};

/// Write the registry publish request: JSON length, JSON, archive length, archive.
pub fn write_publish_body(
    mut writer: impl Write,
    metadata: &serde_json::Value,
    archive: &[u8],
) -> io::Result<()> {
    let json = serde_json::to_vec(metadata).map_err(io::Error::other)?;
    let json_len = u32::try_from(json.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "registry metadata too large"))?;
    let archive_len = u32::try_from(archive.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "crate archive too large"))?;
    writer.write_all(&json_len.to_le_bytes())?;
    writer.write_all(&json)?;
    writer.write_all(&archive_len.to_le_bytes())?;
    writer.write_all(archive)
}

#[cfg(test)]
mod tests {
    use super::write_publish_body;

    #[test]
    fn publish_body_contains_little_endian_lengths_and_exact_bytes() {
        let metadata = serde_json::json!({"name": "example", "vers": "1.0.0"});
        let json = serde_json::to_vec(&metadata).unwrap();
        let mut body = Vec::new();
        write_publish_body(&mut body, &metadata, b"archive").unwrap();
        assert_eq!(&body[..4], &(json.len() as u32).to_le_bytes());
        assert_eq!(&body[4..4 + json.len()], json);
        assert_eq!(&body[4 + json.len()..8 + json.len()], &7u32.to_le_bytes());
        assert_eq!(&body[8 + json.len()..], b"archive");
    }
}
