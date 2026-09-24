//! Minimal HTTP/1.1 client for the daemon API.
//!
//! Deliberately dependency-free (std only): the daemon is localhost-only and
//! speaks a tiny JSON subset, so a full HTTP client (and its TLS stack) would be
//! dead weight. Requests use `Connection: close`, so the body is read to EOF;
//! `Transfer-Encoding: chunked` is decoded when the server uses it.

use crate::api::{ErrorDto, Health, SearchResponseDto, StatusReport};
use crate::engine::{SearchResponse, Status};
use crate::matcher::Query;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Per-request socket timeout.
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// How often the background poller refreshes status.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A client for `everything-daemon`. Status is refreshed by a background poller
/// so the UI can read it every frame without issuing a request per frame.
pub struct Remote {
    addr: String,
    report: Arc<RwLock<Option<StatusReport>>>,
    connected: Arc<AtomicBool>,
}

impl Remote {
    pub fn new(addr: impl Into<String>) -> Remote {
        let addr = addr.into();
        let report = Arc::new(RwLock::new(None));
        let connected = Arc::new(AtomicBool::new(false));

        let poll_report = Arc::clone(&report);
        let poll_connected = Arc::clone(&connected);
        let poll_addr = addr.clone();
        std::thread::Builder::new()
            .name("daemon-poll".into())
            .spawn(move || loop {
                match request(&poll_addr, "GET", "/v1/status", None) {
                    Ok(body) => match serde_json::from_slice::<StatusReport>(&body) {
                        Ok(report) => {
                            *poll_report.write().unwrap() = Some(report);
                            poll_connected.store(true, Ordering::Relaxed);
                        }
                        Err(_) => poll_connected.store(false, Ordering::Relaxed),
                    },
                    Err(_) => poll_connected.store(false, Ordering::Relaxed),
                }
                std::thread::sleep(POLL_INTERVAL);
            })
            .ok();

        Remote {
            addr,
            report,
            connected,
        }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// True if the last status poll succeeded.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn health(&self) -> Result<Health, String> {
        let body = request(&self.addr, "GET", "/v1/health", None)?;
        serde_json::from_slice(&body).map_err(|e| format!("bad health response: {e}"))
    }

    pub fn search(&self, q: &Query) -> Result<SearchResponse, String> {
        let payload = serde_json::to_vec(q).map_err(|e| e.to_string())?;
        let body = request(&self.addr, "POST", "/v1/search", Some(&payload))?;
        let dto: SearchResponseDto =
            serde_json::from_slice(&body).map_err(|e| format!("bad search response: {e}"))?;
        Ok(dto.into_response())
    }

    pub fn rebuild(&self) -> Result<(), String> {
        request(&self.addr, "POST", "/v1/rebuild", Some(b"{}")).map(|_| ())
    }

    /// Last polled status report (`None` until the first successful poll).
    pub fn report(&self) -> Option<StatusReport> {
        self.report.read().unwrap().clone()
    }

    pub fn status(&self) -> Status {
        self.report().map(|r| r.status).unwrap_or_default()
    }
}

/// Quick TCP reachability probe, for auto-detecting a running daemon without
/// paying a full request timeout.
pub fn probe(addr: &str, timeout: Duration) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return false;
    };
    let Some(sock) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&sock, timeout).is_ok()
}

/// Percent-decode a URL path/query component (`%XX` and `+`).
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One HTTP/1.1 request; returns the response body. The connection is always
/// closed by the client (and by the daemon), so the body is read to EOF.
///
/// Exposed so any client (or test) can issue arbitrary requests against the
/// daemon without pulling in an HTTP stack.
pub fn request(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_nodelay(true).ok();

    let body = body.unwrap_or(b"");
    let head = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|e| format!("send {path}: {e}"))?;
    stream.flush().ok();

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read {path}: {e}"))?;

    let split = find(&raw, b"\r\n\r\n").ok_or("malformed HTTP response")?;
    let (headers, rest) = raw.split_at(split);
    let body_bytes = &rest[4..];
    let headers_text = String::from_utf8_lossy(headers);

    let status_line = headers_text.lines().next().unwrap_or("");
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);

    let chunked = headers_text
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding") && l.contains("chunked"));
    let body = if chunked {
        decode_chunked(body_bytes)?
    } else {
        body_bytes.to_vec()
    };

    if !(200..300).contains(&code) {
        let msg = serde_json::from_slice::<ErrorDto>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| String::from_utf8_lossy(&body).trim().to_string());
        return Err(format!("HTTP {code}: {msg}"));
    }
    Ok(body)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let pos = find(rest, b"\r\n").ok_or("bad chunk header")?;
        let head = std::str::from_utf8(&rest[..pos]).map_err(|_| "bad chunk header")?;
        let size = usize::from_str_radix(head.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| format!("bad chunk size {head:?}"))?;
        rest = &rest[pos + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        rest = rest.strip_prefix(b"\r\n".as_slice()).unwrap_or(rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%2A.pdf"), "*.pdf");
        assert_eq!(percent_decode("%2"), "%2"); // truncated escape is left alone
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn chunked_decoding() {
        let raw = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw).unwrap(), b"Wikipedia");
    }

    #[test]
    fn chunked_decoding_truncated() {
        assert!(decode_chunked(b"5\r\nab\r\n").is_err());
    }
}
