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
    let bypasses = |rules: &NoProxy, url: &str| rules.bypasses(&url::Url::parse(url).unwrap());
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
        assert!(bypasses(&rules, url), "{url}");
    }
    for url in [
        "https://notexample.com/",
        "https://index.crates.io/",
        "http://11.0.0.1/",
        "http://192.168.1.6/",
        "http://127.0.0.2/",
    ] {
        assert!(!bypasses(&rules, url), "{url}");
    }
    assert!(bypasses(&NoProxy::parse("*"), "https://index.crates.io/"));
    assert!(!bypasses(&NoProxy::parse(""), "https://index.crates.io/"));
}

#[test]
fn routing_uses_the_host_the_url_parser_connects_to() {
    // A backslash ends the authority for http(s), as in the URL parser ureq uses: the host
    // is attacker.example, so a no_proxy entry for trusted.example does not apply.
    let rules = NoProxy::parse("trusted.example");
    let target = join(
        "https://index.crates.io/",
        "https://attacker.example\\@trusted.example/path",
    );
    let parsed = url::Url::parse(&target).unwrap();
    assert_eq!(parsed.host_str(), Some("attacker.example"));
    assert!(!rules.bypasses(&parsed));
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

#[test]
fn query_fragment_and_dot_segment_references_follow_rfc_3986() {
    let base = "https://h/api/v1/download?a=1#x";
    assert_eq!(join(base, "?new=1"), "https://h/api/v1/download?new=1");
    assert_eq!(join(base, "#top"), "https://h/api/v1/download?a=1#top");
    assert_eq!(join(base, "../other?b=2"), "https://h/api/other?b=2");
    assert_eq!(join(base, "./same"), "https://h/api/v1/same");
    assert_eq!(join(base, "../../../up"), "https://h/up");
    assert_eq!(join(base, "HTTPS://Other/x"), "https://other/x");
    // An absolute target is normalized, so a chained relative redirect resolves soundly.
    let hop = join(base, "https://example.com/a/..");
    assert_eq!(hop, "https://example.com/");
    assert_eq!(join(&hop, "next"), "https://example.com/next");
    // Percent-encoded dot segments count as dot segments.
    assert_eq!(join(base, "https://h/a/b/%2e/../next"), "https://h/a/next");
    assert_eq!(join(base, "https://h/a/b/%2E%2e/c"), "https://h/a/c");
}

#[test]
fn too_many_redirects_is_an_error_not_the_redirect_body() {
    let (url, _seen) = serve(vec![
        "HTTP/1.1 302 Found\r\nLocation: /a\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            .into(),
        "HTTP/1.1 302 Found\r\nLocation: /b\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            .into(),
    ]);
    let agent = Builder::new(ureq::AgentBuilder::new, None)
        .redirects(1)
        .build();
    match agent.get(&format!("{url}/start")).call() {
        Err(ureq::Error::Status(302, _)) => {}
        other => panic!(
            "expected a redirect-limit error, got {:?}",
            other.map(|r| r.status())
        ),
    }
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
        "HTTP/1.1 303 See Other\r\nLocation: /next\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".into(),
        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok".into(),
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
        "HTTP/1.1 302 Found\r\nLocation: /next\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            .into(),
    ]);
    let agent = Builder::new(ureq::AgentBuilder::new, None)
        .redirects(0)
        .build();
    let response = agent.get(&format!("{url}/first")).call().unwrap();
    assert_eq!(response.status(), 302);
}
