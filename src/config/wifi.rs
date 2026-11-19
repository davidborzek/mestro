//! [wifi]: the wifi setup lane's device-level knobs (feature
//! `wifi-setup`). Config-file only — deliberately NOT in the
//! settings schema: a kiosk user changes wifi from the setup
//! portal, not the settings editor. Values resolve at runtime
//! (interface detection, the derived DHCP range), so one binary
//! covers every board.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

use super::lenient::Problem;

/// the default setup hotspot address (NetworkManager's shared-mode
/// convention: private, rarely collides with home LANs)
pub const DEFAULT_AP_ADDRESS: &str = "10.42.0.1/24";

/// prefixes the range derivation supports — smaller pools than /29
/// cannot host a hotspot plus a client
const MIN_PREFIX: u8 = 16;
const MAX_PREFIX: u8 = 29;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Wifi {
    /// "auto" (the default) picks the first wireless netdev
    /// (an entry under /sys/class/net/<if>/wireless, an up one
    /// preferred); a name pins one explicitly
    pub interface: String,
    /// wpa_supplicant's control socket directory
    pub ctrl_dir: String,
    /// the persistent STA conf (must match what the boot scripts
    /// start wpa_supplicant with)
    pub conf: String,
    /// the setup hotspot's own address, a private "a.b.c.d/<prefix>"
    /// — the DHCP pool (10–50 hosts, clamped for small prefixes)
    /// and the captive portal derive from it
    pub ap_address: String,
}

impl Default for Wifi {
    fn default() -> Self {
        Self {
            interface: "auto".into(),
            ctrl_dir: "/run/wpa_supplicant".into(),
            conf: "/data/wpa_supplicant.conf".into(),
            ap_address: DEFAULT_AP_ADDRESS.into(),
        }
    }
}

/// one parsed ap-address: the hotspot's gateway, its subnet and
/// the derived DHCP pool
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApNet {
    /// the hotspot's own address (gateway + DNS wildcard target)
    pub ip: Ipv4Addr,
    pub prefix: u8,
    /// the subnet base (the DHCP pool derives from it)
    pub net: Ipv4Addr,
    /// inclusive bounds of the DHCP pool — hosts .10–.50 where
    /// the subnet allows, else its top
    pub dhcp_low: Ipv4Addr,
    pub dhcp_high: Ipv4Addr,
}

fn to_u32(ip: Ipv4Addr) -> u32 {
    u32::from(ip)
}

fn from_u32(v: u32) -> Ipv4Addr {
    Ipv4Addr::from(v)
}

/// is this a private range (RFC 1918)? A hotspot must never sit in
/// public space — the wildcard DNS would shadow real names
fn is_private(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
}

/// parse "a.b.c.d/<prefix>" into the AP plan. Rejected: garbage,
/// prefixes outside /16–/29, public addresses
pub fn parse_ap_address(text: &str) -> Option<ApNet> {
    let (ip, prefix) = text.split_once('/')?;
    let prefix: u8 = prefix.parse().ok()?;
    if !(MIN_PREFIX..=MAX_PREFIX).contains(&prefix) {
        return None;
    }
    let ip = ip.parse::<Ipv4Addr>().ok()?;
    if !is_private(ip) {
        return None;
    }
    let net_u = to_u32(ip) & mask(prefix);
    let usable = (1u32 << (32 - prefix)) - 2;
    // hosts .10–.50 where the subnet allows, else the top of the
    // pool — the gateway sits at the low end by convention
    let low = net_u + 10.min(usable - 2);
    let high = net_u + 50.min(usable);
    if high <= low {
        return None;
    }
    Some(ApNet {
        ip,
        prefix,
        net: from_u32(net_u),
        dhcp_low: from_u32(low),
        dhcp_high: from_u32(high),
    })
}

/// the prefix as a host-order bitmask
fn mask(prefix: u8) -> u32 {
    u32::MAX << (32 - prefix)
}

impl ApNet {
    /// does the subnet hold this address? (the web endpoint's
    /// peer check — an AP-trusted connection must come from here)
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        to_u32(ip) & mask(self.prefix) == to_u32(self.net)
    }
}

impl Wifi {
    /// the parsed AP address (the default when the configured one
    /// does not parse — validated at load, this stays cheap)
    pub fn ap_net(&self) -> ApNet {
        parse_ap_address(&self.ap_address)
            .unwrap_or_else(|| parse_ap_address(DEFAULT_AP_ADDRESS).expect("the default parses"))
    }

    /// semantic validation: garbage falls back to the default and
    /// says so — the kiosk always gets a usable wifi plan
    pub fn validate(&mut self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if self.interface.trim().is_empty() {
            problems.push(Problem {
                key: "wifi.interface".into(),
                reason: "empty — using auto".into(),
            });
            self.interface = "auto".into();
        }
        if !self.conf.starts_with('/') {
            problems.push(Problem {
                key: "wifi.conf".into(),
                reason: format!("{} is not absolute — using the default", self.conf),
            });
            self.conf = Self::default().conf;
        }
        if !self.ctrl_dir.starts_with('/') {
            problems.push(Problem {
                key: "wifi.ctrl-dir".into(),
                reason: format!("{} is not absolute — using the default", self.ctrl_dir),
            });
            self.ctrl_dir = Self::default().ctrl_dir;
        }
        if parse_ap_address(&self.ap_address).is_none() {
            problems.push(Problem {
                key: "wifi.ap-address".into(),
                reason: format!(
                    "{} is not a private a.b.c.d/16-29 — using the default",
                    self.ap_address
                ),
            });
            self.ap_address = DEFAULT_AP_ADDRESS.into();
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip() {
        let wifi = Wifi::default();
        assert_eq!(wifi.interface, "auto");
        assert_eq!(wifi.ctrl_dir, "/run/wpa_supplicant");
        assert_eq!(wifi.conf, "/data/wpa_supplicant.conf");
        assert_eq!(wifi.ap_address, DEFAULT_AP_ADDRESS);
        // the config-file form parses back (kebab-case keys)
        let text = toml::to_string(&wifi).unwrap();
        assert!(text.contains("ap-address = \"10.42.0.1/24\""), "{text}");
        let back: Wifi = toml::from_str(&text).unwrap();
        assert_eq!(back, wifi);
        // and an empty section is the default
        let empty: Wifi = toml::from_str("").unwrap();
        assert_eq!(empty, wifi);
    }

    #[test]
    fn a_24_derives_the_documented_pool() {
        let ap = parse_ap_address("10.42.0.1/24").unwrap();
        assert_eq!(ap.ip.to_string(), "10.42.0.1");
        assert_eq!(ap.net.to_string(), "10.42.0.0");
        assert_eq!(ap.dhcp_low.to_string(), "10.42.0.10");
        assert_eq!(ap.dhcp_high.to_string(), "10.42.0.50");
    }

    #[test]
    fn larger_prefixes_derive_pools_inside_their_subnet() {
        // a /16 still uses the low .10–.50 of its first /24
        let ap = parse_ap_address("172.20.0.1/16").unwrap();
        assert_eq!(ap.net.to_string(), "172.20.0.0");
        assert_eq!(ap.dhcp_low.to_string(), "172.20.0.10");
        assert_eq!(ap.dhcp_high.to_string(), "172.20.0.50");
        // a /29 has 6 hosts: the pool clamps into the top
        let ap = parse_ap_address("192.168.50.1/29").unwrap();
        assert_eq!(ap.dhcp_low.to_string(), "192.168.50.4");
        assert_eq!(ap.dhcp_high.to_string(), "192.168.50.6");
    }

    #[test]
    fn the_subnet_membership_is_the_masked_compare() {
        let ap = parse_ap_address("10.42.0.1/24").unwrap();
        for inside in ["10.42.0.1", "10.42.0.20", "10.42.0.255"] {
            assert!(ap.contains(inside.parse().unwrap()), "{inside}");
        }
        for outside in ["10.42.1.1", "10.43.0.20", "127.0.0.1", "192.168.178.20"] {
            assert!(!ap.contains(outside.parse().unwrap()), "{outside}");
        }
    }

    #[test]
    fn garbage_prefixes_and_public_addresses_are_rejected() {
        assert!(parse_ap_address("192.168.4.1").is_none());
        assert!(parse_ap_address("192.168.4.1/33").is_none());
        assert!(
            parse_ap_address("192.168.4.1/30").is_none(),
            "/30 is below the /29 floor"
        );
        assert!(
            parse_ap_address("192.168.4.1/8").is_none(),
            "/8 is above the /16 ceiling"
        );
        assert!(parse_ap_address("999.0.0.1/24").is_none());
        assert!(parse_ap_address("").is_none());
        // public space must not become a hotspot (wildcard DNS)
        for public in ["8.8.8.8/24", "1.1.1.1/16", "172.32.0.1/24"] {
            assert!(parse_ap_address(public).is_none(), "{public}");
        }
        // the private ranges pass
        for private in [
            "10.0.0.1/24",
            "172.16.0.1/24",
            "172.31.9.1/24",
            "192.168.178.1/24",
        ] {
            assert!(parse_ap_address(private).is_some(), "{private}");
        }
    }

    #[test]
    fn garbage_falls_back_with_problems() {
        let mut wifi = Wifi {
            interface: "  ".into(),
            conf: "relative.conf".into(),
            ctrl_dir: "run/wpa".into(),
            ap_address: "8.8.8.8/24".into(),
        };
        let problems = wifi.validate();
        assert_eq!(problems.len(), 4);
        let keys: Vec<&str> = problems.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "wifi.interface",
                "wifi.conf",
                "wifi.ctrl-dir",
                "wifi.ap-address"
            ]
        );
        assert_eq!(wifi, Wifi::default());
        // a valid section reports nothing and stays
        let mut wifi = Wifi {
            ap_address: "10.99.5.1/24".into(),
            ..Default::default()
        };
        assert!(wifi.validate().is_empty());
        assert_eq!(wifi.ap_net().dhcp_low.to_string(), "10.99.5.10");
    }
}
