//! minimal HTTP plumbing: request-head parsing and replies —
//! routing only, no parser library. The head is hand-routed
//! because tungstenite's handshake rejects plain GETs before any
//! callback runs.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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
pub(super) fn split_head(buf: &[u8]) -> usize {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(buf.len(), |at| at + 4)
}

pub(super) fn method(head: &[u8]) -> &str {
    request_head(head)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or_default()
}

/// a header value by case-insensitive name (head part only)
pub(super) fn header<'a>(head: &'a [u8], name: &str) -> Option<&'a str> {
    request_head(head).lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

pub(super) fn content_length(head: &[u8]) -> Option<u64> {
    let value = header(head, "content-length")?;
    value
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| value.parse().ok())?
}

// the text of the head, never the body bytes behind it
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
    reply_typed(
        stream,
        status,
        "text/html; charset=utf-8",
        "Cache-Control: no-store",
        &[],
        body.as_bytes(),
    )
    .await
}

#[cfg(feature = "ota")]
pub(super) async fn reply_json(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> Result<(), String> {
    reply_typed(
        stream,
        status,
        "application/json",
        "Cache-Control: no-store",
        &[],
        body.as_bytes(),
    )
    .await
}

/// a byte reply with its own cache policy and optional extra
/// headers (Set-Cookie): the asset/cover path
pub(super) async fn reply_with_headers(
    stream: &mut TcpStream,
    status: &str,
    extra: &[(&str, &str)],
    content_type: &str,
    cache_control: &str,
    body: &[u8],
) -> Result<(), String> {
    reply_typed(stream, status, content_type, cache_control, extra, body).await
}

pub(super) async fn reply_typed(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    cache_control: &str,
    extra: &[(&str, &str)],
    body: &[u8],
) -> Result<(), String> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{cache_control}\r\n",
        body.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .await
        .map(|_| ())
        .map_err(|e| format!("write: {e}"))?;
    stream
        .write_all(body)
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

/// one query parameter's URL-decoded value (the preview endpoints'
/// skin/style/v — skin names are plain, but decode properly)
pub(super) fn query_param(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key != name {
            continue;
        }
        let mut out = Vec::with_capacity(value.len());
        let bytes = value.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 3 <= bytes.len() => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                    out.push(u8::from_str_radix(hex, 16).ok()?);
                    i += 2;
                }
                byte => out.push(byte),
            }
            i += 1;
        }
        return Some(String::from_utf8_lossy(&out).into_owned());
    }
    None
}

/// PNG bytes as hex over the JSON bridge (keeps the internal
/// preview op's reply a plain string — no base64 dependency)
pub(super) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

pub(super) fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    let bytes = text.as_bytes();
    let digit = |b: u8| -> Result<u8, String> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err("not a hex digit".into()),
        }
    };
    bytes
        .chunks(2)
        .map(|pair| Ok(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod query_tests {
    use super::*;

    #[test]
    fn query_params_decode_percent_and_plus() {
        assert_eq!(
            query_param("skin=round&v=3", "skin").as_deref(),
            Some("round")
        );
        assert_eq!(query_param("skin=round&v=3", "v").as_deref(), Some("3"));
        assert_eq!(
            query_param("skin=my%20skin&v=3", "skin").as_deref(),
            Some("my skin")
        );
        assert_eq!(query_param("skin=a+b&v=3", "skin").as_deref(), Some("a b"));
        assert_eq!(query_param("v=%e2%9c%93", "v").as_deref(), Some("✓"));
        assert_eq!(query_param("skin=round", "missing"), None);
        assert_eq!(query_param("skin", "skin"), None);
    }

    #[test]
    fn hex_roundtrips_every_byte() {
        let bytes: Vec<u8> = (0..=255).collect();
        let hex = hex_encode(&bytes);
        assert_eq!(hex.len(), 512);
        assert_eq!(hex_decode(&hex).unwrap(), bytes);
        assert!(hex_decode("0f0").is_err());
        assert!(hex_decode("zz").is_err());
    }
}
