//! [wifi]: the wifi setup lane's device-level knobs (feature
//! `wifi-setup`). Config-file only — deliberately NOT in the
//! settings schema: a kiosk user changes wifi from the setup
//! portal, not the settings editor. Values resolve at runtime
//! (interface detection, the derived DHCP range), so one binary
//! covers every board.
//!
//! The settings type itself (`WifiSettings`, `ApNet`, the
//! address parsing) lives with the wifi lane (`mestro-os`) —
//! this module embeds it as the `[wifi]` section and owns the
//! lenient validation (garbage falls back to a default and is
//! reported as a config problem).

use super::lenient::Problem;
use mestro_os::wifi::parse_ap_address;
pub use mestro_os::wifi::{ApNet, DEFAULT_AP_ADDRESS};

/// the `[wifi]` section — the wifi lane's `WifiSettings`, embedded
/// here so `config.wifi` keeps its historical name
pub type Wifi = mestro_os::wifi::WifiSettings;

/// is this an ISO 3166-1 alpha-2 code (two ASCII letters, case
/// insensitive — load uppercases it)?
fn is_country(code: &str) -> bool {
    code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic())
}

/// semantic validation: garbage falls back to the default and
/// says so — the kiosk always gets a usable wifi plan
pub fn validate(wifi: &mut Wifi) -> Vec<Problem> {
    let mut problems = Vec::new();
    if wifi.interface.trim().is_empty() {
        problems.push(Problem {
            key: "wifi.interface".into(),
            reason: "empty — using auto".into(),
        });
        wifi.interface = "auto".into();
    }
    if parse_ap_address(&wifi.ap_address).is_none() {
        problems.push(Problem {
            key: "wifi.ap-address".into(),
            reason: format!(
                "{} is not a private a.b.c.d/16-29 — using the default",
                wifi.ap_address
            ),
        });
        wifi.ap_address = DEFAULT_AP_ADDRESS.into();
    }
    if is_country(&wifi.country) {
        wifi.country = wifi.country.to_ascii_uppercase();
    } else if !wifi.country.is_empty() {
        problems.push(Problem {
            key: "wifi.country".into(),
            reason: format!(
                "{} is not an ISO 3166-1 alpha-2 code — leaving the regdom alone",
                wifi.country
            ),
        });
        wifi.country = String::new();
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_falls_back_with_problems() {
        let mut wifi = Wifi {
            interface: "  ".into(),
            ap_address: "8.8.8.8/24".into(),
            country: "DEU".into(),
            ..Default::default()
        };
        let problems = validate(&mut wifi);
        assert_eq!(problems.len(), 3);
        let keys: Vec<&str> = problems.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, ["wifi.interface", "wifi.ap-address", "wifi.country"]);
        assert_eq!(wifi, Wifi::default());
        // a valid section reports nothing and stays
        let mut wifi = Wifi {
            ap_address: "10.99.5.1/24".into(),
            ..Default::default()
        };
        assert!(validate(&mut wifi).is_empty());
        assert_eq!(wifi.ap_net().dhcp_low.to_string(), "10.99.5.10");
    }

    #[test]
    fn a_valid_country_is_uppercased_and_garbage_ignored() {
        // two ASCII letters, any case — the configured regdom
        let mut wifi = Wifi {
            country: "de".into(),
            ..Default::default()
        };
        assert!(validate(&mut wifi).is_empty());
        assert_eq!(wifi.country, "DE");
        let mut wifi = Wifi {
            country: "DE".into(),
            ..Default::default()
        };
        assert!(validate(&mut wifi).is_empty());
        assert_eq!(wifi.country, "DE");
        // anything else is a problem and leaves the regdom alone
        for bad in ["D", "DEU", "D1", "1D", "de-DE", "\u{4de}E"] {
            let mut wifi = Wifi {
                country: bad.into(),
                ..Default::default()
            };
            let problems = validate(&mut wifi);
            assert_eq!(problems.len(), 1, "{bad}");
            assert_eq!(problems[0].key, "wifi.country");
            assert_eq!(wifi.country, "", "{bad} must not reach the radio");
        }
    }
}
