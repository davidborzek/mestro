//! The wpa_supplicant control interface: a unix datagram client
//! (bind a local path, connect the ctrl socket, request/response
//! with a timeout) plus the tolerant reply parsers — coded against
//! the documented ctrl formats; real device captures pin them later.
//!
//! - replies are `OK` / `FAIL`, or a payload
//! - `SCAN_RESULTS`: one row per BSS, tab-separated
//!   `bssid / frequency / signal level / flags / ssid`; the first
//!   line is the row count (raw ctrl) or the human header (wpa_cli)
//!   — the parser skips anything that is not a data row
//! - `STATUS`: `key=value` lines; unknown keys ignored
//! - unsolicited event frames start with `<` and never carry a
//!   reply: a client that does not ATTACH gets none, but be tolerant

#![deny(clippy::indexing_slicing)]

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// one ctrl roundtrip may take this long before the socket is
/// considered dead
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

pub struct WpaCtrl {
    socket: UnixDatagram,
    local: PathBuf,
}

impl WpaCtrl {
    /// connect the ctrl socket (`/run/wpa_supplicant/<iface>`)
    pub fn connect(ctrl_path: &Path) -> Result<Self, String> {
        let local = local_path();
        let socket = UnixDatagram::bind(&local).map_err(|e| format!("wpa ctrl bind: {e}"))?;
        // the bound path is a file: a failed connect must not leave it
        // behind (the policy tick retries every few seconds)
        if let Err(e) = socket.connect(ctrl_path) {
            let _ = std::fs::remove_file(&local);
            return Err(format!("wpa ctrl connect {}: {e}", ctrl_path.display()));
        }
        Ok(Self { socket, local })
    }

    /// one command → its reply. Unsolicited event frames (`<…`) are
    /// dropped until the reply arrives (an attached client reads them
    /// with [`Self::wait_event`])
    pub fn request(&mut self, cmd: &str) -> Result<String, String> {
        self.socket
            .set_read_timeout(Some(REPLY_TIMEOUT))
            .map_err(|e| format!("wpa ctrl timeout: {e}"))?;
        self.socket
            .send(cmd.as_bytes())
            .map_err(|e| format!("wpa ctrl send {cmd}: {e}"))?;
        let mut buf = [0u8; 4096];
        loop {
            let n = self
                .socket
                .recv(&mut buf)
                .map_err(|e| format!("wpa ctrl recv {cmd}: {e}"))?;
            let reply = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[])).into_owned();
            if reply.starts_with('<') {
                continue;
            }
            return Ok(reply);
        }
    }
}

/// the events that end a scan: results are ready, or the scan
/// failed (the cached results are all there is)
pub const SCAN_FINISHED_EVENTS: &[&str] = &["CTRL-EVENT-SCAN-RESULTS", "CTRL-EVENT-SCAN-FAILED"];

impl WpaCtrl {
    /// subscribe to the unsolicited event stream (`ATTACH`)
    pub fn attach(&mut self) -> Result<(), String> {
        expect_ok(&self.request("ATTACH")?)
    }

    /// wait up to `timeout` for an event frame naming one of `wanted`;
    /// other events are skipped. false = none came in time
    pub fn wait_event(&mut self, wanted: &[&str], timeout: Duration) -> Result<bool, String> {
        let deadline = std::time::Instant::now() + timeout;
        let mut buf = [0u8; 4096];
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            self.socket
                .set_read_timeout(Some(left))
                .map_err(|e| format!("wpa ctrl timeout: {e}"))?;
            let n = match self.socket.recv(&mut buf) {
                Ok(n) => n,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(false)
                }
                Err(e) => return Err(format!("wpa ctrl recv event: {e}")),
            };
            let frame = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[]));
            if frame.starts_with('<') && wanted.iter().any(|event| frame.contains(event)) {
                return Ok(true);
            }
        }
    }
}

impl Drop for WpaCtrl {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.local);
    }
}

/// a unique local socket per client — /run/wpa_supplicant is only
/// writable by root, /tmp always works
fn local_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("mestro-wpa-{}-{n}.sock", std::process::id()))
}

/// `OK`/`FAIL` → Result
pub fn expect_ok(reply: &str) -> Result<(), String> {
    match reply.trim() {
        "OK" => Ok(()),
        other => Err(format!("wpa_supplicant replied {other:?}")),
    }
}

/// one scan row
#[derive(Debug, Clone, PartialEq)]
pub struct ScanEntry {
    pub bssid: String,
    pub frequency: u32,
    pub signal: i32,
    pub flags: String,
    pub ssid: String,
}

impl ScanEntry {
    /// a PSK/WPA/WEP-capable network (open hotspots have flags
    /// like `[ESS]` or none)
    pub fn secured(&self) -> bool {
        ["WPA", "WEP", "RSN", "PSK", "802.1X"]
            .iter()
            .any(|k| self.flags.contains(k))
    }
}

/// parse a SCAN_RESULTS reply: data rows only — the count line, the
/// header and noise are skipped; SSIDs unescape `\xNN`
pub fn parse_scan_results(reply: &str) -> Vec<ScanEntry> {
    let mut out = Vec::new();
    for line in reply.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        let (Some(bssid), Some(frequency), Some(signal), Some(flags), Some(ssid)) = (
            cols.first().copied(),
            cols.get(1).copied(),
            cols.get(2).copied(),
            cols.get(3).copied(),
            cols.get(4).copied(),
        ) else {
            continue;
        };
        let (Some(frequency), Ok(signal)) = (
            frequency.trim().parse::<u32>().ok(),
            signal.trim().parse::<i32>(),
        ) else {
            continue;
        };
        out.push(ScanEntry {
            bssid: bssid.trim().to_string(),
            frequency,
            signal,
            flags: flags.trim().to_string(),
            ssid: String::from_utf8_lossy(&unescape(ssid)).into_owned(),
        });
    }
    out
}

/// `\xNN` escapes back to bytes; unknown escapes stay literal
fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        // `\\xNN` with two valid hex nibbles → the escaped byte;
        // anything else stays literal
        if byte == b'\\' && bytes.get(i + 1) == Some(&b'x') {
            let (Some(hi), Some(lo)) = (
                bytes.get(i + 2).and_then(|b| hex_digit(*b)),
                bytes.get(i + 3).and_then(|b| hex_digit(*b)),
            ) else {
                out.push(byte);
                i += 1;
                continue;
            };
            out.push(hi << 4 | lo);
            i += 4;
            continue;
        }
        out.push(byte);
        i += 1;
    }
    out
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// the STATUS fields the state machine reads
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusInfo {
    pub wpa_state: String,
    pub ssid: String,
    /// the associated BSSID — the association identity (power-save
    /// re-applies once per association)
    pub bssid: String,
}

/// parse a STATUS reply: `key=value` lines, unknown keys ignored,
/// empty values kept as empty
pub fn parse_status(reply: &str) -> StatusInfo {
    let mut info = StatusInfo::default();
    for line in reply.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "wpa_state" => info.wpa_state = value.to_string(),
            "ssid" => info.ssid = String::from_utf8_lossy(&unescape(value)).into_owned(),
            "bssid" => info.bssid = value.to_string(),
            _ => {}
        }
    }
    info
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)] // the fake supplicant slices the datagrams it just received
mod tests {
    use super::*;

    fn own_sockets() -> usize {
        let prefix = format!("mestro-wpa-{}-", std::process::id());
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .count()
    }

    #[test]
    fn local_sockets_never_pile_up() {
        let before = own_sockets();
        // no supplicant behind this path: every attempt fails
        for _ in 0..5 {
            assert!(WpaCtrl::connect(Path::new("/nonexistent/wlan0")).is_err());
        }
        assert_eq!(own_sockets(), before, "failed connects leaked socket files");

        // a connected client removes its path when dropped
        let dir = std::env::temp_dir().join(format!("mestro-wpa-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let server = UnixDatagram::bind(dir.join("wlan0")).unwrap();
        let client = WpaCtrl::connect(&dir.join("wlan0")).unwrap();
        assert_eq!(own_sockets(), before + 1);
        drop(client);
        assert_eq!(own_sockets(), before);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // recorded-shape captures: the raw ctrl reply (count first) and
    // the wpa_cli shape (header first) must both parse
    const RAW_REPLY: &str = "3\n\
        2c:91:ab:12:34:56\t2412\t-52\t[WPA2-PSK-CCMP][ESS]\tFritzBox\n\
        2c:91:ab:12:34:56\t2412\t-55\t[WPA2-PSK-CCMP][ESS]\tCaf\u{e9} Netz\n\
        00:11:22:33:44:55\t2437\t-61\t[ESS]\t\n\
        00:11:22:33:44:55\t2462\t-70\t[WPA-PSK-CCMP][ESS]\t";

    #[test]
    fn scan_rows_parse_and_noise_is_skipped() {
        let entries = parse_scan_results(RAW_REPLY);
        assert_eq!(entries.len(), 4, "{entries:?}");
        assert_eq!(entries.first().unwrap().bssid, "2c:91:ab:12:34:56");
        assert_eq!(entries.first().unwrap().frequency, 2412);
        assert_eq!(entries.first().unwrap().signal, -52);
        assert_eq!(entries.first().unwrap().ssid, "FritzBox");
        assert!(entries.first().unwrap().secured());
        // an escaped UTF-8 ssid comes back decoded
        assert_eq!(
            entries.get(1).unwrap().ssid,
            String::from_utf8_lossy(&b"Caf\xc3\xa9 Netz"[..])
        );
        // hidden networks: empty ssid, open flags
        assert_eq!(entries.get(2).unwrap().ssid, "");
        assert!(!entries.get(2).unwrap().secured());
        // and a hidden one that still carries a WPA flag
        assert_eq!(entries.get(3).unwrap().ssid, "");
        assert!(entries.get(3).unwrap().secured());
        // the header shape parses the same
        let header = "bssid / frequency / signal level / flags / ssid\n".to_string() + RAW_REPLY;
        assert_eq!(parse_scan_results(&header).len(), 4);
        // an empty reply is empty, not an error
        assert!(parse_scan_results("").is_empty());
    }

    #[test]
    fn status_parses_known_keys_and_ignores_the_rest() {
        let status = parse_status(
            "bssid=2c:91:ab:12:34:56\nfreq=2412\nssid=FritzBox\nwpa_state=COMPLETED\n\
             ip_address=192.168.178.20\nkey_mgmt=WPA2-PSK\n",
        );
        assert_eq!(status.wpa_state, "COMPLETED");
        assert_eq!(status.ssid, "FritzBox");
        assert_eq!(status.bssid, "2c:91:ab:12:34:56");
        // tolerant: unknown keys, missing keys, weird spacing
        let status = parse_status("future_key=1\nwpa_state=SCANNING\nssid=\n");
        assert_eq!(status.wpa_state, "SCANNING");
        assert_eq!(status.ssid, "");
        assert_eq!(parse_status("").wpa_state, "");
    }

    /// a fake supplicant on a socket in a temp dir: replies OK to
    /// ATTACH, then sends the given event frames to the client
    fn fake_supplicant(
        frames: Vec<&'static str>,
        pause: Duration,
    ) -> (PathBuf, std::thread::JoinHandle<()>) {
        let dir = std::env::temp_dir().join(format!(
            "mestro-wpa-events-{}-{}",
            std::process::id(),
            frames.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wlan0");
        let server = UnixDatagram::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            let (n, client) = server.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"ATTACH");
            let client = client.as_pathname().unwrap().to_path_buf();
            server.send_to(b"OK\n", &client).unwrap();
            for frame in frames {
                std::thread::sleep(pause);
                server.send_to(frame.as_bytes(), &client).unwrap();
            }
        });
        (path, handle)
    }

    #[test]
    fn attach_then_wait_returns_when_the_scan_results_event_arrives() {
        let (path, supplicant) = fake_supplicant(
            vec![
                "<3>CTRL-EVENT-BSS-ADDED 1 aa:bb:cc:dd:ee:ff",
                "<3>CTRL-EVENT-SCAN-STARTED ",
                "<3>CTRL-EVENT-SCAN-RESULTS ",
            ],
            Duration::from_millis(20),
        );
        let mut client = WpaCtrl::connect(&path).unwrap();
        client.attach().unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            client.wait_event(SCAN_FINISHED_EVENTS, Duration::from_secs(3)),
            Ok(true)
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "returned on the event, not the timeout"
        );
        supplicant.join().unwrap();
    }

    #[test]
    fn a_failed_scan_also_ends_the_wait() {
        let (path, supplicant) = fake_supplicant(
            vec![
                "<3>CTRL-EVENT-SCAN-FAILED ret=-16",
                "<3>CTRL-EVENT-SCAN-RESULTS ",
                "x",
            ],
            Duration::from_millis(10),
        );
        let mut client = WpaCtrl::connect(&path).unwrap();
        client.attach().unwrap();
        assert_eq!(
            client.wait_event(SCAN_FINISHED_EVENTS, Duration::from_secs(3)),
            Ok(true)
        );
        supplicant.join().unwrap();
    }

    #[test]
    fn without_the_event_the_wait_runs_out_of_time_and_says_so() {
        let (path, supplicant) = fake_supplicant(
            vec![
                "<3>CTRL-EVENT-BSS-ADDED 1 aa:bb:cc:dd:ee:ff",
                "<3>CTRL-EVENT-SCAN-STARTED ",
                "a",
                "b",
            ],
            Duration::from_millis(10),
        );
        let mut client = WpaCtrl::connect(&path).unwrap();
        client.attach().unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            client.wait_event(SCAN_FINISHED_EVENTS, Duration::from_millis(300)),
            Ok(false)
        );
        assert!(started.elapsed() >= Duration::from_millis(250));
        supplicant.join().unwrap();
    }

    #[test]
    fn a_refused_attach_is_an_error() {
        let dir = std::env::temp_dir().join(format!("mestro-wpa-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let server = UnixDatagram::bind(dir.join("wlan0")).unwrap();
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            let (_, client) = server.recv_from(&mut buf).unwrap();
            server
                .send_to(b"FAIL\n", client.as_pathname().unwrap())
                .unwrap();
        });
        let mut client = WpaCtrl::connect(&dir.join("wlan0")).unwrap();
        assert!(client.attach().is_err());
        handle.join().unwrap();
    }

    #[test]
    fn ok_fail_replies() {
        assert!(expect_ok("OK\n").is_ok());
        assert!(expect_ok("OK").is_ok());
        assert!(expect_ok("FAIL\n").is_err());
        assert!(expect_ok("2\nbssid\tfreq").is_err());
    }

    #[test]
    fn escaped_ssids_roundtrip_bytes() {
        // \xNN escapes decode to raw bytes (lossy as text)
        let entries =
            parse_scan_results("aa:bb:cc:dd:ee:ff\t2412\t-40\t[ESS]\tJust\\x20G\\xc3\\xa4ste");
        assert_eq!(entries.first().unwrap().ssid, "Just G\u{e4}ste");
        // unknown escapes stay literal
        let entries = parse_scan_results("aa:bb:cc:dd:ee:ff\t2412\t-40\t[ESS]\ta\\zb");
        assert_eq!(entries.first().unwrap().ssid, "a\\zb");
    }
}
