//! The captive portal: while the kiosk runs its setup hotspot, the
//! OS connectivity probes (Android `/generate_204`, Apple
//! `/hotspot-detect.html`, Windows `/ncsi.txt`, …) and any request
//! that dials another host by name bounce into the setup page.
//! Modern clients need neither: dnsmasq's DHCP option 114 (RFC
//! 8910) tells them the portal URI up front. Outside the hotspot
//! (per connection: the peer-subnet rule, `Ingress::of_peer`) none
//! of this exists — the paths keep their regular behavior.
//! There is no second listener: the probes arrive on the one main
//! port and the main `handle` routes them when the connection is
//! AP-trusted.

use super::http;
use tokio::net::TcpStream;

/// the setup page's own hostname — the hotspot's wildcard DNS
/// resolves every name (including this one) to the player
const SETUP_HOST: &str = "mestro.setup";

/// does this GET belong to the portal? The probe paths, or any
/// request whose Host header names something that is not the
/// kiosk itself (captive checks fetch their vendor's URL, the
/// hotspot answers for it)
pub(super) fn redirects(path: &str, host: Option<&str>, gateway: &str) -> bool {
    PROBE_PATHS.contains(&path)
        || host.is_some_and(|h| {
            let name = host_name(h);
            !own_host(name, gateway)
        })
}

/// the connectivity probes the OS vendors bake in — reserved for
/// the portal, never served as content
const PROBE_PATHS: &[&str] = &[
    "/generate_204",
    "/gen_204",
    "/hotspot-detect.html",
    "/library/test/success.html",
    "/connecttest.txt",
    "/ncsi.txt",
];

/// the host without its port — bracketed IPv6 forms included
/// (`[::1]:80` and `[::1]` both name `::1`)
fn host_name(host: &str) -> &str {
    if let Some((name, _)) = host.split_once(']') {
        return name.trim_start_matches('[');
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// a host the kiosk answers for: the hotspot's gateway (an IP in
/// the redirect avoids DNS-cache trouble on clients), the setup
/// name, its current LAN address and the loopback names
fn own_host(name: &str, gateway: &str) -> bool {
    matches!(name, "127.0.0.1" | "localhost" | "::1")
        || name == gateway
        || name == SETUP_HOST
        || name == mestro_os::lan_ip()
}

/// the 302 itself — a Location the caller composes: the probe
/// redirect uses the gateway IP (a client may have cached a DNS
/// answer from another network), the app-route landing a
/// relative "/setup/wifi" (whatever host the client dialed keeps
/// working)
pub(super) async fn redirect(stream: &mut TcpStream, target: &str) -> Result<(), String> {
    http::reply_typed(
        stream,
        "302 Found",
        "text/plain; charset=utf-8",
        "Cache-Control: no-store",
        &[("Location", target)],
        b"the setup page lives at /setup/wifi\n",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATEWAY: &str = "10.42.0.1";

    #[test]
    fn the_probe_paths_are_reserved() {
        for path in PROBE_PATHS {
            assert!(redirects(path, Some(GATEWAY), GATEWAY), "{path}");
        }
        // and nothing else is
        assert!(!redirects("/", Some(GATEWAY), GATEWAY));
        assert!(!redirects("/setup/wifi", Some(GATEWAY), GATEWAY));
        assert!(!redirects("/generate_2042", Some(GATEWAY), GATEWAY));
    }

    #[test]
    fn a_foreign_host_redirects_an_own_one_does_not() {
        assert!(redirects("/", Some("captive.apple.com"), GATEWAY));
        assert!(redirects(
            "/",
            Some("connectivitycheck.gstatic.com:80"),
            GATEWAY
        ));
        assert!(redirects("/setup/wifi", Some("192.168.178.1"), GATEWAY));
        // own names, with and without a port: the gateway, the
        // setup name, the loopback names
        for host in [
            "10.42.0.1",
            "10.42.0.1:8787",
            "mestro.setup",
            "localhost:8787",
            "127.0.0.1",
            "[::1]",
            "[::1]:80",
        ] {
            assert!(!redirects("/", Some(host), GATEWAY), "{host}");
        }
        // another gateway config moves the own-name boundary
        assert!(redirects("/", Some("10.42.0.1"), "10.99.0.1"));
        assert!(!redirects("/", Some("10.99.0.1"), "10.99.0.1"));
        // no Host header at all: serve, do not guess
        assert!(!redirects("/", None, GATEWAY));
    }
}
