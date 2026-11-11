//! minimal HTTP plumbing: request-head parsing and replies —
//! routing only, no parser library. The head is hand-routed
//! because tungstenite's handshake rejects plain GETs before any
//! callback runs.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// the wizard SPA, built from web/ by the build/release process
// (build.rs embeds web/dist/index.html / $MESTRO_WEB_DIST — the
// Vite single-file build, generated, never committed)
pub(super) const SPA: &str = include_str!(concat!(env!("OUT_DIR"), "/mestro-web.html"));

/// read until the end of the request head (\r\n\r\n)
pub(super) async fn read_head(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut head: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Err("empty request".into());
        }
        head.extend_from_slice(&chunk[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(head);
        }
        if head.len() > 16 * 1024 {
            return Err("request head too large".into());
        }
    }
}

/// request target → (path, query) from the request line
pub(super) fn target(head: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(head);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.split_whitespace();
    let _method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    (path.to_string(), query.to_string())
}

/// length of the head incl. the blank line: `read_head` may have
/// pulled body bytes in too, they start at `buf[split_head(buf)..]`
#[cfg(any(feature = "ota", test))]
pub(super) fn split_head(buf: &[u8]) -> usize {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(buf.len(), |at| at + 4)
}

#[cfg(any(feature = "ota", test))]
pub(super) fn method(head: &[u8]) -> &str {
    request_head(head)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or_default()
}

/// a header value by case-insensitive name (head part only)
#[cfg(any(feature = "ota", test))]
pub(super) fn header<'a>(head: &'a [u8], name: &str) -> Option<&'a str> {
    request_head(head).lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

#[cfg(any(feature = "ota", test))]
pub(super) fn content_length(head: &[u8]) -> Option<u64> {
    let value = header(head, "content-length")?;
    value
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| value.parse().ok())?
}

// the text of the head, never the body bytes behind it
#[cfg(any(feature = "ota", test))]
fn request_head(buf: &[u8]) -> &str {
    std::str::from_utf8(&buf[..split_head(buf)]).unwrap_or_default()
}

pub(super) fn is_upgrade(head: &[u8]) -> bool {
    String::from_utf8_lossy(head)
        .to_ascii_lowercase()
        .contains("upgrade: websocket")
}

pub(super) async fn reply(stream: &mut TcpStream, status: &str, body: &str) -> Result<(), String> {
    reply_html(stream, status, body).await
}

pub(super) async fn reply_html(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> Result<(), String> {
    reply_typed(stream, status, "text/html; charset=utf-8", body).await
}

#[cfg(feature = "ota")]
pub(super) async fn reply_json(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> Result<(), String> {
    reply_typed(stream, status, "application/json", body).await
}

async fn reply_typed(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map(|_| ())
        .map_err(|e| format!("write: {e}"))?;
    stream
        .write_all(body.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;
    stream.shutdown().await.ok();
    Ok(())
}

/// serialize a protocol response (the error fallback never fails)
pub(super) fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|e| format!(r#"{{"id":0,"ok":false,"error":"{e}"}}"#))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &[u8] = b"POST /ota?pair=ABC HTTP/1.1\r\nHost: kiosk\r\ncontent-length: 4096\r\nX-Sha256: ab12\r\n\r\nARMd-body";

    #[test]
    fn split_head_keeps_the_body_prefix_out() {
        let head_len = split_head(REQUEST);
        assert_eq!(&REQUEST[head_len..], b"ARMd-body");
        assert!(REQUEST[..head_len].ends_with(b"\r\n\r\n"));
    }

    #[test]
    fn split_head_without_terminator_takes_everything() {
        assert_eq!(split_head(b"GET / HTTP/1.1\r\n"), 16);
    }

    #[test]
    fn method_comes_from_the_request_line() {
        assert_eq!(method(REQUEST), "POST");
        assert_eq!(method(b"GET / HTTP/1.1\r\n\r\n"), "GET");
        assert_eq!(method(b""), "");
    }

    #[test]
    fn header_lookup_is_case_insensitive_and_trimmed() {
        assert_eq!(header(REQUEST, "Content-Length"), Some("4096"));
        assert_eq!(header(REQUEST, "x-sha256"), Some("ab12"));
        assert_eq!(header(REQUEST, "X-Missing"), None);
    }

    #[test]
    fn header_ignores_the_body_and_the_request_line() {
        let request = b"POST /x HTTP/1.1\r\nA: 1\r\n\r\nX-Sha256: in-body\xff\xfe";
        assert_eq!(header(request, "X-Sha256"), None);
        assert_eq!(header(request, "POST /x HTTP/1.1"), None);
        assert_eq!(header(request, "A"), Some("1"));
    }

    #[test]
    fn content_length_parses_digits_only() {
        assert_eq!(content_length(REQUEST), Some(4096));
        assert_eq!(content_length(b"POST / HTTP/1.1\r\n\r\n"), None);
        assert_eq!(
            content_length(b"POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n"),
            None
        );
        assert_eq!(
            content_length(b"POST / HTTP/1.1\r\nContent-Length: 12x\r\n\r\n"),
            None
        );
    }
}
