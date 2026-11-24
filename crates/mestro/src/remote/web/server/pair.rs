//! Pairing: `POST /pair` trades the PIN for a long-lived cookie,
//! `GET /pair/status` reports whether the caller holds one. The
//! cookie carries a derived token, not the PIN — a browser
//! replays cookie headers everywhere, the PIN itself must stay
//! in the URL hash the user scanned. Brute force is throttled
//! in-memory: five failures close the door for 30 s.

use super::{auth, http};
use auth::ct_eq;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// how many wrong PINs lock the endpoint, and for how long
const MAX_FAILURES: u32 = 5;
const BLOCK: std::time::Duration = std::time::Duration::from_secs(30);
/// the pairing body is one JSON object, never more
const MAX_BODY: usize = 1024;

/// the per-server pairing state: PIN + derived token + the
/// failure gate (shared by every connection)
pub(crate) struct Pairing {
    pin: String,
    token: String,
    gate: parking_lot::Mutex<Gate>,
}

#[derive(Default)]
struct Gate {
    failures: u32,
    blocked_until: Option<std::time::Instant>,
    block: std::time::Duration,
}

impl Pairing {
    pub(crate) fn new(pin: &str) -> Self {
        Self {
            pin: pin.to_string(),
            token: auth::pair_token(pin),
            gate: parking_lot::Mutex::new(Gate {
                block: BLOCK,
                ..Gate::default()
            }),
        }
    }

    #[cfg(test)]
    fn new_test(pin: &str, block: std::time::Duration) -> Self {
        Self {
            pin: pin.to_string(),
            token: auth::pair_token(pin),
            gate: parking_lot::Mutex::new(Gate {
                block,
                ..Gate::default()
            }),
        }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn pin(&self) -> &str {
        &self.pin
    }

    /// 429 while the gate is closed
    fn blocked(&self) -> bool {
        self.gate
            .lock()
            .blocked_until
            .is_some_and(|until| until > std::time::Instant::now())
    }

    /// one wrong PIN counts; returns true when this failure closed
    /// the gate (the attempt itself is already 429 material)
    fn fail(&self) -> bool {
        self.gate.lock().fail()
    }

    fn clear(&self) {
        let mut gate = self.gate.lock();
        gate.failures = 0;
        gate.blocked_until = None;
    }
}

impl Gate {
    fn fail(&mut self) -> bool {
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            self.blocked_until = Some(std::time::Instant::now() + self.block);
            self.failures = 0;
            return true;
        }
        false
    }
}

/// `POST /pair` with body `{"pin":"…"}` → 204 + Set-Cookie, 401 on
/// a wrong PIN, 429 while the gate is closed
pub(super) async fn handle(
    stream: &mut TcpStream,
    request: &[u8],
    pairing: &Pairing,
) -> Result<(), String> {
    if pairing.blocked() {
        return http::reply(
            stream,
            "429 Too Many Requests",
            "too many attempts, try later",
        )
        .await;
    }
    let body = match read_body(stream, request).await {
        Ok(body) => body,
        Err(failure) => {
            let (status, message) = (failure.0, failure.1);
            return http::reply(stream, status, message).await;
        }
    };
    let pin = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("pin")?.as_str().map(str::to_string))
        .ok_or_else(|| "bad pairing request".to_string());
    let pin = match pin {
        Ok(pin) => pin,
        Err(message) => {
            return http::reply(stream, "400 Bad Request", &message).await;
        }
    };
    // again at the comparison: requests that passed the first check
    // while their bodies trickled in must not keep guessing
    if pairing.blocked() {
        return http::reply(
            stream,
            "429 Too Many Requests",
            "too many attempts, try later",
        )
        .await;
    }
    if !ct_eq(&pin, &pairing.pin) {
        return if pairing.fail() {
            http::reply(
                stream,
                "429 Too Many Requests",
                "too many attempts, try later",
            )
            .await
        } else {
            http::reply(stream, "401 Unauthorized", "wrong pin").await
        };
    }
    pairing.clear();
    let cookie = format!(
        "{name}={token}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Strict",
        name = auth::COOKIE,
        token = pairing.token()
    );
    http::reply_with_headers(
        stream,
        "204 No Content",
        &[("Set-Cookie", &cookie)],
        "text/plain; charset=utf-8",
        "Cache-Control: no-store",
        b"",
    )
    .await
}

/// `GET /pair/status` → `{"paired": true|false}`, cookie only
pub(super) async fn status(
    stream: &mut TcpStream,
    request: &[u8],
    pairing: &Pairing,
) -> Result<(), String> {
    let paired = http::header(request, "cookie")
        .and_then(|cookies| auth::cookie_value(cookies, auth::COOKIE))
        .is_some_and(|token| ct_eq(&token, pairing.token()));
    let body = format!("{{\"paired\":{paired}}}");
    http::reply_typed(
        stream,
        "200 OK",
        "application/json",
        "Cache-Control: no-store",
        &[],
        body.as_bytes(),
    )
    .await
}

/// the request body: the bytes read_head already pulled in plus
/// the rest, capped at MAX_BODY — errors carry an HTTP status
async fn read_body(
    stream: &mut TcpStream,
    request: &[u8],
) -> Result<Vec<u8>, (&'static str, &'static str)> {
    let want =
        http::content_length(request).ok_or(("411 Length Required", "Content-Length required"))?;
    if want > MAX_BODY as u64 {
        return Err(("413 Payload Too Large", "not a pairing request"));
    }
    let mut body = request[http::split_head(request)..].to_vec();
    body.truncate(want as usize);
    while body.len() < want as usize {
        let mut more = [0u8; 512];
        let n = stream
            .read(&mut more)
            .await
            .map_err(|_| ("400 Bad Request", "upload interrupted"))?;
        if n == 0 {
            return Err(("400 Bad Request", "incomplete body"));
        }
        body.extend_from_slice(&more[..n]);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "ABCD2345";

    #[test]
    fn five_failures_close_the_gate_and_the_block_reopens_it() {
        let pairing = Pairing::new_test(PIN, std::time::Duration::from_millis(20));
        for _ in 0..4 {
            assert!(!pairing.fail(), "the first four only count");
            assert!(!pairing.blocked());
        }
        assert!(pairing.fail(), "the fifth failure closes the gate");
        assert!(pairing.blocked());
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(!pairing.blocked(), "the block expires");
        // the gate re-arms: failures counted from zero again
        assert!(!pairing.fail());
        assert!(!pairing.blocked());
    }

    #[test]
    fn a_success_clears_the_failures() {
        let pairing = Pairing::new_test(PIN, BLOCK);
        for _ in 0..4 {
            pairing.fail();
        }
        pairing.clear();
        pairing.fail();
        assert!(
            !pairing.blocked(),
            "four failures then a success then one more"
        );
    }
}
