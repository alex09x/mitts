/*
 * Mitts Build — retry transient network failures of registry requests
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/mitts
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A registry that answers one request with 500, drops a connection or times out should
//! not fail the whole build. Transient failures (transport errors, 408, 429 and 5xx, and a
//! body cut off while reading) are retried after 1, 3 and 5 seconds; anything else (401,
//! 403, 404, ...) is returned at once.
//!
//! `MITTS_NET_RETRY=N` sets the number of retries (default 3, `0` disables them), like
//! Cargo's `net.retry`. `MITTS_NET_RETRY_DELAY_MS` scales the delays (default 1000 ms per
//! step), which tests use to keep retries fast.

use anyhow::Result;
use std::io::Read;
use std::time::Duration;

const DEFAULT_RETRIES: usize = 3;
const DELAY_STEPS: [u64; 3] = [1, 3, 5];

fn retries() -> usize {
    std::env::var("MITTS_NET_RETRY")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_RETRIES)
}

fn delay_unit_ms() -> u64 {
    std::env::var("MITTS_NET_RETRY_DELAY_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(1000)
}

fn delay(attempt: usize, unit_ms: u64) -> Duration {
    let step = DELAY_STEPS[attempt.min(DELAY_STEPS.len() - 1)];
    Duration::from_millis(unit_ms.saturating_mul(step))
}

/// Whether a failed request may succeed when repeated.
fn is_transient(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::Status(code, _) => *code == 408 || *code == 429 || *code >= 500,
        ureq::Error::Transport(_) => true,
    }
}

fn describe(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        ureq::Error::Transport(transport) => transport.to_string(),
    }
}

enum Attempt {
    Done(Vec<u8>),
    /// A failure that is final: returned to the caller unchanged.
    Fatal(anyhow::Error),
    /// A failure worth repeating, with a short description for the warning.
    Transient(anyhow::Error, String),
}

fn attempt(request: &impl Fn() -> ureq::Request) -> Attempt {
    let response = match request().call() {
        Ok(response) => response,
        Err(error) if is_transient(&error) => {
            let what = describe(&error);
            return Attempt::Transient(error.into(), what);
        }
        Err(error) => return Attempt::Fatal(error.into()),
    };
    let mut bytes = Vec::new();
    match response.into_reader().read_to_end(&mut bytes) {
        Ok(_) => Attempt::Done(bytes),
        Err(error) => {
            let what = format!("reading the response failed: {error}");
            Attempt::Transient(error.into(), what)
        }
    }
}

/// Sends the GET built by `request` and reads the whole body, retrying transient
/// failures. A final HTTP error keeps its `ureq::Error` (callers match on the status via
/// [`status_code`]).
pub fn get_bytes(url: &str, request: impl Fn() -> ureq::Request) -> Result<Vec<u8>> {
    let retries = retries();
    let mut attempt_no = 0;
    loop {
        match attempt(&request) {
            Attempt::Done(bytes) => return Ok(bytes),
            Attempt::Fatal(error) => return Err(error),
            Attempt::Transient(error, what) => {
                if attempt_no >= retries {
                    return Err(error);
                }
                let wait = delay(attempt_no, delay_unit_ms());
                attempt_no += 1;
                eprintln!(
                    "mitts warning: {url}: {what}; retrying in {:.1}s ({attempt_no}/{retries})",
                    wait.as_secs_f64()
                );
                std::thread::sleep(wait);
            }
        }
    }
}

/// The HTTP status of a failed [`get_bytes`] call, if the server answered at all.
pub fn status_code(error: &anyhow::Error) -> Option<u16> {
    match error.downcast_ref::<ureq::Error>() {
        Some(ureq::Error::Status(code, _)) => Some(*code),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// Serves the given status codes in order, one per connection, then stops.
    fn serve(statuses: Vec<u16>) -> (String, std::thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/crate", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut served = 0;
            for status in statuses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                    line.clear();
                }
                let body = if status == 200 { "payload" } else { "" };
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                served += 1;
            }
            served
        });
        (url, handle)
    }

    fn fast_retries() {
        std::env::set_var("MITTS_NET_RETRY_DELAY_MS", "1");
    }

    #[test]
    fn a_transient_server_error_is_retried_until_it_succeeds() {
        fast_retries();
        let (url, server) = serve(vec![500, 503, 200]);
        let agent = ureq::agent();
        let bytes = get_bytes(&url, || agent.get(&url)).unwrap();
        assert_eq!(bytes, b"payload");
        assert_eq!(server.join().unwrap(), 3);
    }

    #[test]
    fn a_missing_crate_is_not_retried() {
        fast_retries();
        let (url, server) = serve(vec![404]);
        let agent = ureq::agent();
        let error = get_bytes(&url, || agent.get(&url)).unwrap_err();
        assert_eq!(status_code(&error), Some(404));
        assert_eq!(server.join().unwrap(), 1);
    }

    #[test]
    fn retries_stop_after_the_last_attempt() {
        fast_retries();
        let (url, server) = serve(vec![500, 500, 500, 500]);
        let agent = ureq::agent();
        let error = get_bytes(&url, || agent.get(&url)).unwrap_err();
        assert_eq!(status_code(&error), Some(500));
        // One request and three retries.
        assert_eq!(server.join().unwrap(), 4);
    }

    #[test]
    fn delays_grow_one_three_five_seconds() {
        assert_eq!(delay(0, 1000), Duration::from_secs(1));
        assert_eq!(delay(1, 1000), Duration::from_secs(3));
        assert_eq!(delay(2, 1000), Duration::from_secs(5));
        assert_eq!(delay(7, 1000), Duration::from_secs(5));
    }
}
