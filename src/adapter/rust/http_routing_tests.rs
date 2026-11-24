/*
 * Mitts Build — Rust-first build and package tooling with shared artifact cache
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn hosts_subdomains_addresses_ranges_and_ports_go_direct() {
    let rules = NoProxy::parse(
        " Example.com, .internal ,*.corp, 10.0.0.0/8, ::1, 192.168.1.5,host:8080, 127.0.0.1:3000, [fe80::1]:443",
    );
    for url in [
        "https://example.com/x",
        "https://index.EXAMPLE.com/config.json",
        "http://registry.internal:8080/",
        "https://a.b.corp",
        "http://10.20.30.40/",
        "http://[::1]:3000/",
        "http://user:pw@192.168.1.5/",
        "http://host/",
        "http://127.0.0.1:9999/",
        "https://[fe80::1]/",
    ] {
        assert!(rules.bypasses(url), "{url}");
    }
    for url in [
        "https://notexample.com/",
        "https://index.crates.io/",
        "http://11.0.0.1/",
        "http://192.168.1.6/",
        "http://127.0.0.2/",
    ] {
        assert!(!rules.bypasses(url), "{url}");
    }
    assert!(NoProxy::parse("*").bypasses("https://index.crates.io/"));
    assert!(!NoProxy::parse("").bypasses("https://index.crates.io/"));
}

#[test]
fn redirect_locations_resolve_against_the_request() {
    let base = "https://crates.io/api/v1/crates/x/1.0.0/download?a=1";
    assert_eq!(
        join(base, "https://static.crates.io/x.crate"),
        "https://static.crates.io/x.crate"
    );
    assert_eq!(join(base, "//cdn.example/x"), "https://cdn.example/x");
    assert_eq!(join(base, "/other"), "https://crates.io/other");
    assert_eq!(
        join(base, "file"),
        "https://crates.io/api/v1/crates/x/1.0.0/file"
    );
}

/// A one-shot HTTP server: answers each connection with the next canned response and
/// records the request lines and headers it got.
fn serve(responses: Vec<String>) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            tx.send(String::from_utf8_lossy(&buf[..n]).into_owned())
                .unwrap();
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (url, rx)
}

#[test]
fn redirects_are_followed_without_the_token_and_303_becomes_get() {
    let (url, seen) = serve(vec![
        "HTTP/1.1 303 See Other\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n".into(),
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".into(),
    ]);
    let agent = Builder::new(ureq::AgentBuilder::new, None).build();
    let response = agent
        .put(&format!("{url}/first"))
        .set("Authorization", "secret")
        .send_string("{}")
        .unwrap();
    assert_eq!(response.into_string().unwrap(), "ok");
    let first = seen.recv().unwrap();
    let second = seen.recv().unwrap();
    assert!(first.starts_with("PUT /first"), "{first}");
    assert!(first.to_ascii_lowercase().contains("authorization: secret"));
    assert!(second.starts_with("GET /next"), "{second}");
    assert!(
        !second.to_ascii_lowercase().contains("authorization"),
        "{second}"
    );
}

#[test]
fn with_no_redirects_the_3xx_comes_back() {
    let (url, _seen) = serve(vec![
        "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n".into(),
    ]);
    let agent = Builder::new(ureq::AgentBuilder::new, None)
        .redirects(0)
        .build();
    let response = agent.get(&format!("{url}/first")).call().unwrap();
    assert_eq!(response.status(), 302);
}
