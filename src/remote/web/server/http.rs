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
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
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
