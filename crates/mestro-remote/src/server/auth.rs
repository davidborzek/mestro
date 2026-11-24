//! Channel auth: the pairing PIN arrives as a query param or —
//! after POST /pair — as the `mestro_pair` cookie (a derived
//! token, so the PIN itself never sits in a header a browser
//! replays anywhere else). The PIN also travels in the URL hash
//! of the page the kiosk shows (plain HTTP logs never see it).

use sha2::{Digest, Sha256};

/// the pairing cookie's name
pub(super) const COOKIE: &str = "mestro_pair";

/// the cookie token: hex sha256("mestro-pair:" + PIN) — the PIN
/// stays the only secret, the cookie carries a derived value
pub(crate) fn pair_token(pin: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("mestro-pair:{pin}"));
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// auth check for /ws, /ota and /cover: EITHER `?pair=<PIN>` OR
/// the pairing cookie. `cookie` is the raw Cookie header value
pub(super) fn authorized(query: &str, cookie: Option<&str>, pin: &str, token: &str) -> bool {
    let pin_query = query
        .split('&')
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == "pair").then(|| v.to_string())
        })
        .is_some_and(|got| ct_eq(&got, pin));
    let cookie_ok = cookie
        .and_then(|header| cookie_value(header, COOKIE))
        .is_some_and(|got| ct_eq(&got, token));
    pin_query || cookie_ok
}

/// one cookie value by name ("a=1; b=2" → "2" for b)
pub(super) fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// constant-time compare (short-circuiting on length is fine —
/// that leaks only the PIN length, which is fixed)
pub(super) fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "ABCD2345";

    #[test]
    fn the_token_is_sha256_over_the_prefixed_pin() {
        assert_eq!(
            pair_token(PIN),
            "fade4fc183ed9e28da79aa80b4cd44b2d052a139d3110e94d8687e160c0c8271"
        );
        // the PIN never appears in the token
        assert!(!pair_token(PIN).contains(PIN));
    }

    #[test]
    fn pin_or_cookie_authorize() {
        let token = pair_token(PIN);
        assert!(authorized("pair=ABCD2345", None, PIN, &token));
        assert!(authorized(
            "",
            Some(&format!("{COOKIE}={token}")),
            PIN,
            &token
        ));
        assert!(authorized(
            "x=1",
            Some(&format!("theme=dark; {COOKIE}={token}; other=1")),
            PIN,
            &token
        ));
        assert!(!authorized("", None, PIN, &token));
        assert!(!authorized("pair=WRONG234", None, PIN, &token));
        // a cookie derived from a DIFFERENT pin must not pass
        let other = pair_token("MMMMMMMM");
        assert!(!authorized(
            "",
            Some(&format!("{COOKIE}={other}")),
            PIN,
            &token
        ));
        // neither must an unrelated cookie of the right name
        assert!(!authorized(
            "",
            Some(&format!("{COOKIE}=deadbeef")),
            PIN,
            &token
        ));
    }

    #[test]
    fn cookie_values_parse_by_name() {
        let header = "a=1; mestro_pair=tok; b=2";
        assert_eq!(cookie_value(header, COOKIE).as_deref(), Some("tok"));
        assert_eq!(cookie_value(header, "a").as_deref(), Some("1"));
        assert_eq!(cookie_value(header, "missing"), None);
    }
}
