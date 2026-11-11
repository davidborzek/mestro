//! WS-channel auth: the pairing PIN arrives as a query param, the
//! PIN itself travels in the URL hash (plain HTTP logs never see
//! it).

/// auth check: the PIN is restart-stable (persisted 0600).
pub(super) fn authorized(query: &str, pin: &str) -> bool {
    query
        .split('&')
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == "pair").then(|| v.to_string())
        })
        .map(|got| ct_eq(&got, pin))
        .unwrap_or(false)
}

/// constant-time compare (short-circuiting on length is fine —
/// that leaks only the PIN length, which is fixed)
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
