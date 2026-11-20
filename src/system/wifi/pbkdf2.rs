//! PBKDF2-HMAC-SHA1 — the WPA-PSK derivation (IEEE 802.11i: salt =
//! SSID, 4096 rounds, 256 bits). `hmac` + `sha1` are the tiny
//! RustCrypto siblings of the sha2 crate the pairing token already
//! uses; PBKDF2 itself is the short loop below, pinned by the
//! published 802.11i test vectors.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

pub const ROUNDS: u32 = 4096;
pub const KEY_LEN: usize = 32;

/// the passphrase → PSK step (wpa_passphrase's job, in-process):
/// validates the wpa_passphrase rules, then derives
pub fn wpa_psk(passphrase: &str, ssid: &str) -> Result<[u8; KEY_LEN], String> {
    let bytes = passphrase.as_bytes();
    if !(8..=63).contains(&bytes.len()) {
        return Err("passphrase must be 8 to 63 characters".into());
    }
    if !bytes.iter().all(|b| (0x20..=0x7e).contains(b)) {
        return Err("passphrase must be printable ASCII".into());
    }
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("ssid must be 1 to 32 bytes".into());
    }
    Ok(pbkdf2_sha1(bytes, ssid.as_bytes(), ROUNDS))
}

/// PBKDF2 with a fixed 32-byte output (two SHA-1 blocks — the
/// second one truncated to 12 bytes)
pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], rounds: u32) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    for (i, slot) in out.chunks_mut(20).enumerate() {
        // U1 = PRF(password, salt || INT(i+1) as one big-endian byte)
        let mut mac = HmacSha1::new_from_slice(password).expect("hmac takes any key size");
        mac.update(salt);
        // PBKDF2's INT(i): the block index as a 4-byte big-endian
        mac.update(&u32::try_from(i + 1).expect("two blocks").to_be_bytes());
        let mut u = mac.finalize().into_bytes();
        slot.copy_from_slice(&u[..slot.len()]);
        for _ in 1..rounds {
            let mut mac = HmacSha1::new_from_slice(password).expect("hmac takes any key size");
            mac.update(&u);
            u = mac.finalize().into_bytes();
            for (acc, byte) in slot.iter_mut().zip(u.iter()) {
                *acc ^= *byte;
            }
        }
    }
    out
}

/// lowercase hex, the conf's psk= form
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_80211i_vectors_hold() {
        // the published IEEE 802.11i (D.1) test vectors, independently
        // verified against a reference PBKDF2
        assert_eq!(
            hex(&wpa_psk("ThisIsAPassword", "ThisIsASSID").unwrap()),
            "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af"
        );
        assert_eq!(
            hex(&wpa_psk("password", "IEEE").unwrap()),
            "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"
        );
    }

    #[test]
    fn short_and_non_printable_passphrases_are_refused() {
        assert!(wpa_psk("short", "ssid").is_err());
        assert!(wpa_psk("exactly-8", "ssid").is_ok());
        assert!(wpa_psk("non-printable\u{00e4}passphrase", "ssid").is_err());
        assert!(wpa_psk("password", "").is_err());
        let long = "a".repeat(33);
        assert!(wpa_psk("password", &long).is_err());
        let ok = "a".repeat(32);
        assert!(wpa_psk("password", &ok).is_ok());
    }

    #[test]
    fn the_salt_matters() {
        assert_ne!(
            pbkdf2_sha1(b"password", b"one", 4096),
            pbkdf2_sha1(b"password", b"two", 4096)
        );
        // fewer rounds give a different key (the loop is live)
        assert_ne!(
            pbkdf2_sha1(b"password", b"one", 2),
            pbkdf2_sha1(b"password", b"one", 3)
        );
    }
}
