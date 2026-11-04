/*
 * Mitts Build — crates.io package upload
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Read;

use super::registry_protocol::write_publish_body;

pub fn upload(metadata: &Value, archive: &[u8], token: &str) -> Result<()> {
    upload_to(
        "https://crates.io/api/v1/crates/new",
        metadata,
        archive,
        token,
    )
}

fn upload_to(url: &str, metadata: &Value, archive: &[u8], token: &str) -> Result<()> {
    if token.trim().is_empty() || token.contains(['\r', '\n']) {
        bail!("registry token is missing or invalid");
    }
    let mut body = Vec::new();
    write_publish_body(&mut body, metadata, archive)?;
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let response = agent
        .put(url)
        .set("Authorization", token)
        .set("Content-Type", "application/octet-stream")
        .set("Accept", "application/json")
        .set("User-Agent", "mitts")
        .send_bytes(&body)
        .map_err(|_| anyhow::anyhow!("registry upload failed"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("registry response is too large");
    }
    let result: Value = serde_json::from_slice(&bytes).context("invalid registry response")?;
    if !result.is_object() || result.get("errors").is_some_and(|v| !v.is_null()) {
        bail!("registry rejected the package");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::upload_to;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    #[test]
    fn upload_sends_expected_protocol_and_authorization() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line);
            }
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
            (headers, body)
        });
        upload_to(
            &format!("http://{address}/api/v1/crates/new"),
            &serde_json::json!({"name":"demo"}),
            b"crate",
            "test-token",
        )
        .unwrap();
        let (headers, body) = server.join().unwrap();
        assert!(headers.starts_with("PUT /api/v1/crates/new HTTP/1.1"));
        assert!(headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: test-token")));
        let json_len = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
        assert_eq!(
            u32::from_le_bytes(body[4 + json_len..8 + json_len].try_into().unwrap()),
            5
        );
        assert_eq!(&body[8 + json_len..], b"crate");
    }

    #[test]
    fn registry_error_in_success_response_is_rejected_without_echoing_details() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            let body = r#"{"errors":[{"detail":"private registry detail"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let error = upload_to(
            &format!("http://{address}/"),
            &serde_json::json!({}),
            b"crate",
            "test-token",
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("registry rejected"));
        assert!(!error.to_string().contains("private registry detail"));
        assert!(!error.to_string().contains("test-token"));
    }

    #[test]
    fn upload_does_not_follow_redirect_with_token() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_address = target.local_addr().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: http://{target_address}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert!(upload_to(
            &format!("http://{address}/"),
            &serde_json::json!({}),
            b"crate",
            "test-token"
        )
        .is_err());
        server.join().unwrap();
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
