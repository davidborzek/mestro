//! The hardware seam: everything the state machine needs from the
//! device — the wireless interface detection, the wpa_supplicant
//! ctrl socket, process lifecycle (wpa_supplicant, dnsmasq,
//! udhcpc — owned by mestro), interface queries and waiting.
//! [`RealBackend`] runs against the device, `FakeBackend`
//! (tests) records calls and answers canned data.

use super::wpa_ctrl::{WpaCtrl, SCAN_FINISHED_EVENTS};
use crate::locks;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// how long a SIGTERM'd foreign process gets to exit before the
/// SIGKILL escalation — wpa_supplicant needs a moment to deinit
/// its radio state
const FOREIGN_TERM_WAIT: Duration = Duration::from_secs(3);
/// how long a SIGKILL'd foreign process gets to disappear
const FOREIGN_KILL_WAIT: Duration = Duration::from_secs(1);
/// one poll step while waiting for /proc/<pid> to go
const FOREIGN_WAIT_STEP: Duration = Duration::from_millis(50);

/// the paths detection reads — real system paths in the app, a
/// fake tree in tests
pub struct Roots {
    /// /sys/class/net: one directory per netdev
    pub net_class: PathBuf,
    /// /proc: one directory per process (the exact kill matching)
    pub proc: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            net_class: "/sys/class/net".into(),
            proc: "/proc".into(),
        }
    }
}

/// the mestro-started supplicant (the boot scripts' one differs:
/// see [`WifiBackend::kill_foreign`])
pub const WPA_SUPPLICANT: &str = "/usr/sbin/wpa_supplicant";
/// busybox's udhcpc — an applet, `/proc/<pid>/exe` resolves to the
/// busybox binary
pub const BUSYBOX: &str = "/bin/busybox";
/// the dnsmasq mestro spawns (its identity for the startup stray
/// cleanup: our interface + our lease file in the argv)
pub const DNSMASQ: &str = "/usr/sbin/dnsmasq";
/// where dnsmasq (mestro's child) records its leases — the AP
/// retry path waits while a phone holds one
pub const LEASES: &str = "/run/mestro/dnsmasq.leases";

/// resolve `[wifi] interface`: "auto" picks the first wireless
/// netdev — one with a `wireless` subdirectory, an up one
/// preferred, directory order (deterministic). An explicit name
/// passes through (the radio decides whether it exists)
pub fn detect_wireless(roots: &Roots, requested: &str) -> Result<String, String> {
    if requested != "auto" {
        return Ok(requested.to_string());
    }
    let entries = std::fs::read_dir(&roots.net_class)
        .map_err(|e| format!("{}: {e}", roots.net_class.display()))?;
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let wireless: Vec<String> = names
        .iter()
        .filter(|name| roots.net_class.join(name).join("wireless").exists())
        .cloned()
        .collect();
    // an associated interface is the wifi to use (a USB stick
    // plugged later should not steal the onboard one)
    wireless
        .iter()
        .find(|name| {
            std::fs::read_to_string(roots.net_class.join(name).join("operstate"))
                .map(|state| state.trim() == "up")
                .unwrap_or(false)
        })
        .or_else(|| wireless.first())
        .cloned()
        .ok_or_else(|| "no wireless interface found".to_string())
}

/// every pid that runs `exe` — matched EXACTLY, never by a
/// substring scan:
/// - the `/proc/<pid>/exe` symlink resolves to `exe` (busybox
///   applets share one exe — the applet rides in argv)
/// - `argv[0]`'s basename is `applet` (so `busybox ifplugd` never
///   dies for `udhcpc`)
/// - every group in `argv` appears as CONSECUTIVE separate argv
///   entries (`["-i", "wlan0"]` matches two arguments; a glued
///   `-iwlan0` does not)
///
/// Used for processes mestro did not start (the boot scripts'
/// strays, a previous life's children after an exec) — mestro's
/// own die by their tracked handles
pub fn foreign_pids(proc_root: &Path, exe: &str, applet: &str, argv: &[&[&str]]) -> Vec<u32> {
    let entries = match std::fs::read_dir(proc_root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let applet = applet.as_bytes();
    let mut pids = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_string_lossy().parse::<u32>().ok() else {
            continue;
        };
        let dir = entry.path();
        // the exe symlink names the binary the kernel runs
        let Ok(link) = std::fs::read_link(dir.join("exe")) else {
            continue;
        };
        if link.to_string_lossy() != exe {
            continue;
        }
        // the argv is NUL-separated; argv[0] names the applet
        let Ok(cmdline) = std::fs::read(dir.join("cmdline")) else {
            continue;
        };
        let argv_read: Vec<&[u8]> = cmdline
            .split(|byte| *byte == 0)
            .filter(|a| !a.is_empty())
            .collect();
        let Some(argv0) = argv_read.first() else {
            continue;
        };
        let basename = argv0
            .rsplit(|byte| *byte == b'/')
            .find(|part| !part.is_empty())
            .unwrap_or(argv0);
        if basename != applet {
            continue;
        }
        // every group must appear as consecutive separate entries
        let group_present = |group: &[&str]| {
            let group: Vec<&[u8]> = group.iter().map(|g| g.as_bytes()).collect();
            argv_read
                .windows(group.len())
                .any(|w| w == group.as_slice())
        };
        if argv.iter().all(|group| group_present(group)) {
            pids.push(pid);
        }
    }
    pids
}

pub trait WifiBackend {
    /// one wpa_supplicant ctrl roundtrip; a dead socket errors and
    /// the next call reconnects
    fn ctrl(&mut self, cmd: &str) -> Result<String, String>;
    /// subscribe this ctrl client to the supplicant's event stream:
    /// call before `SCAN`, or the completion event can be missed
    fn events_attach(&mut self) -> Result<(), String>;
    /// wait up to `timeout` for the scan-finished event
    /// (`CTRL-EVENT-SCAN-RESULTS`, or `-SCAN-FAILED`); false = none
    /// came in time. A dead socket is an error
    fn wait_scan_finished(&mut self, timeout: Duration) -> Result<bool, String>;
    /// unsubscribe again — an attached client nobody reads from would
    /// fill its socket buffer and get dropped by the supplicant
    fn events_detach(&mut self);
    /// the interface's MAC (the AP ssid suffix)
    fn mac(&mut self) -> Result<String, String>;
    /// run a one-shot command, fail on a non-zero exit; the stdout
    /// feeds the interface queries (`ip -o -4 addr …`)
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String>;
    /// start a named long-running child — mestro owns it
    fn spawn(&mut self, name: &str, program: &str, args: &[&str]) -> Result<(), String>;
    /// stop one of mestro's own children — exact, by the tracked
    /// handle; other processes are never touched
    fn kill_owned(&mut self, name: &str);
    /// the named child's exit status when it already exited (None
    /// while it runs or nothing is tracked) — a supplicant that
    /// died on startup should say so, not just "did not come up"
    fn child_exit_status(&mut self, name: &str) -> Option<String>;
    /// how many processes match the exact foreign identity — the
    /// "is one already running" check (adoption) without killing
    fn count_foreign(&mut self, exe: &str, applet: &str, argv: &[&[&str]]) -> usize;
    /// stop processes mestro did not start (the boot scripts', a
    /// previous life's): the exact [`foreign_pids`] match — exe
    /// link, `argv[0]` applet basename, argv groups as separate
    /// consecutive entries. The call RETURNS ONLY WHEN THEY ARE
    /// GONE: SIGTERM, then SIGKILL after `FOREIGN_TERM_WAIT`
    /// (logged), gone (or abandoned as a zombie) after
    /// `FOREIGN_KILL_WAIT` — a fresh supplicant needs the iface
    /// and the ctrl socket free. The fake kills instantly
    fn kill_foreign(&mut self, exe: &str, applet: &str, argv: &[&[&str]]);
    /// remove the ctrl socket file `<ctrl dir>/<iface>` when no
    /// supplicant owns it — wpa_supplicant refuses to start over
    /// a socket a corpse left behind. Call it right before
    /// spawning one, after the old one is confirmed gone
    fn clear_stale_ctrl_socket(&mut self);
    /// a routable address exists (the boot screen's network check:
    /// the UDP-connect trick, no packets sent)
    fn routable(&mut self) -> bool;
    /// another (wired) interface has an address — a wired setup
    /// never needs the hotspot
    fn eth_up(&mut self) -> bool;
    /// a client currently holds a lease on the hotspot — the AP
    /// retry path waits while a phone uses the portal
    fn ap_has_client(&mut self) -> bool;
    /// wait; the fake advances instantly
    fn sleep(&mut self, duration: Duration);
}

pub struct RealBackend {
    ctrl: Option<WpaCtrl>,
    /// mestro's tracked children in the registry the restart paths
    /// share — a separate small lock, never the manager's op lock
    children: std::sync::Arc<super::Children>,
    /// the resolved wireless interface, the ctrl socket dir and
    /// the netdev tree — `[wifi]` resolved once at spawn, no board
    /// paths in code
    iface: String,
    ctrl_dir: PathBuf,
    net_class: PathBuf,
    proc: PathBuf,
}

impl RealBackend {
    pub fn new(
        iface: &str,
        ctrl_dir: &str,
        net_class: &Path,
        proc: &Path,
        children: std::sync::Arc<super::Children>,
    ) -> Self {
        Self {
            ctrl: None,
            children,
            iface: iface.to_string(),
            ctrl_dir: ctrl_dir.into(),
            net_class: net_class.into(),
            proc: proc.into(),
        }
    }

    /// the shared registry (the manager's shutdown path)
    pub fn children_arc(&self) -> std::sync::Arc<super::Children> {
        self.children.clone()
    }
}

/// the graceful pre-exec stop: ONLY what the hotspot owns —
/// dnsmasq, and the supplicant while the AP is up. A working STA
/// (the supplicant + the lease client) survives the exec and the
/// next start adopts it (see manager::ensure_sta)
pub fn stop_ap_processes(children: &super::Children, ap_up: bool) {
    let mut children = locks::lock(children);
    if let Some(mut child) = children.remove("dnsmasq") {
        let _ = child.kill();
        let _ = child.wait();
    }
    if ap_up {
        if let Some(mut child) = children.remove("wpa_supplicant") {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl RealBackend {
    /// one call on the ctrl socket, connected on first use. A call
    /// that fails drops the socket — never trusted again
    fn through<T>(
        &mut self,
        call: impl FnOnce(&mut WpaCtrl) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.ctrl.is_none() {
            let socket_path = self.ctrl_dir.join(&self.iface);
            self.ctrl = Some(WpaCtrl::connect(&socket_path)?);
        }
        // just connected above, or was still alive
        let Some(socket) = self.ctrl.as_mut() else {
            return Err("wpa ctrl socket vanished".into());
        };
        let result = call(socket);
        if result.is_err() {
            self.ctrl = None;
        }
        result
    }
}

impl WifiBackend for RealBackend {
    fn ctrl(&mut self, cmd: &str) -> Result<String, String> {
        self.through(|socket| socket.request(cmd))
    }

    fn events_attach(&mut self) -> Result<(), String> {
        self.through(WpaCtrl::attach)
    }

    fn wait_scan_finished(&mut self, timeout: Duration) -> Result<bool, String> {
        self.through(|socket| socket.wait_event(SCAN_FINISHED_EVENTS, timeout))
    }

    fn events_detach(&mut self) {
        // best effort: a dead socket is gone with its attachment
        let _ = self.ctrl("DETACH");
    }

    fn mac(&mut self) -> Result<String, String> {
        std::fs::read_to_string(self.net_class.join(&self.iface).join("address"))
            .map(|mac| mac.trim().to_lowercase())
            .map_err(|e| format!("{} mac: {e}", self.iface))
    }

    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| format!("{program} {args:?}: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "{program} {args:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn spawn(&mut self, name: &str, program: &str, args: &[&str]) -> Result<(), String> {
        let child = Command::new(program)
            .args(args)
            .spawn()
            .map_err(|e| format!("spawn {program} {args:?}: {e}"))?;
        // a respawn of the same name replaces the old child
        if let Some(mut old) = locks::lock(&self.children).insert(name.to_string(), child) {
            let _ = old.kill();
            let _ = old.wait();
        }
        Ok(())
    }

    fn kill_owned(&mut self, name: &str) {
        if let Some(mut child) = locks::lock(&self.children).remove(name) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn child_exit_status(&mut self, name: &str) -> Option<String> {
        locks::lock(&self.children)
            .get_mut(name)
            .and_then(|child| child.try_wait().ok().flatten())
            .map(|status| format!("exit status {status}"))
    }

    fn count_foreign(&mut self, exe: &str, applet: &str, argv: &[&[&str]]) -> usize {
        foreign_pids(&self.proc, exe, applet, argv).len()
    }

    fn kill_foreign(&mut self, exe: &str, applet: &str, argv: &[&[&str]]) {
        let pids: Vec<u32> = foreign_pids(&self.proc, exe, applet, argv)
            .into_iter()
            .filter(|pid| *pid != std::process::id())
            .collect();
        for pid in &pids {
            let _ = crate::signal(*pid as i32, libc::SIGTERM);
        }
        if pids.is_empty() || wait_for_exit(&self.proc, &pids, FOREIGN_TERM_WAIT) {
            return;
        }
        // SIGTERM did not end it (a deinit can hang on the radio):
        // the iface and the ctrl socket must be free for the next one
        let stuck: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| !process_gone(&self.proc, *pid))
            .collect();
        tracing::warn!("wifi: {applet} pids {stuck:?} ignored SIGTERM — SIGKILL");
        for pid in &stuck {
            let _ = crate::signal(*pid as i32, libc::SIGKILL);
        }
        wait_for_exit(&self.proc, &stuck, FOREIGN_KILL_WAIT);
    }

    fn clear_stale_ctrl_socket(&mut self) {
        let iface_arg = ["-i", self.iface.as_str()];
        let owned = foreign_pids(&self.proc, WPA_SUPPLICANT, "wpa_supplicant", &[&iface_arg]);
        if !owned.is_empty() {
            return;
        }
        let socket = self.ctrl_dir.join(&self.iface);
        match std::fs::remove_file(&socket) {
            Ok(()) => tracing::info!("wifi: stale ctrl socket {} removed", socket.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                "wifi: cannot remove the stale ctrl socket {}: {e}",
                socket.display()
            ),
        }
    }

    fn routable(&mut self) -> bool {
        crate::lan_ip() != "127.0.0.1"
    }

    fn eth_up(&mut self) -> bool {
        // any interface that is not loopback and not the wifi one
        // counts as the wired fallback (no eth0 assumption)
        self.run("ip", &["-o", "-4", "addr", "show"])
            .map(|out| {
                out.lines().any(|line| {
                    line.contains("inet ")
                        && !line.contains("inet 127.")
                        && !line.contains(&self.iface)
                })
            })
            .unwrap_or(false)
    }

    fn ap_has_client(&mut self) -> bool {
        std::fs::read_to_string(LEASES)
            .map(|leases| !leases.trim().is_empty())
            .unwrap_or(false)
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// gone: /proc/<pid> is missing, or the process is a zombie — a
/// corpse holds no ctrl socket, its parent just has not reaped it
fn process_gone(proc_root: &Path, pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")) else {
        return true;
    };
    // the state letter follows comm's closing paren (comm may hold spaces)
    stat.rsplit(')')
        .next()
        .is_some_and(|rest| rest.trim_start().starts_with('Z'))
}

/// poll until every pid is gone or the budget ends; true = all gone
fn wait_for_exit(proc_root: &Path, pids: &[u32], budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        let stuck: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| !process_gone(proc_root, *pid))
            .collect();
        if stuck.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(FOREIGN_WAIT_STEP);
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! the recording fake: canned ctrl/run replies plus flags for
    //! the interface queries. Everything else lands in `log`
    //! (assertions read the exact command sequences)

    use super::WifiBackend;
    use std::time::Duration;

    #[derive(Default)]
    pub struct FakeBackend {
        pub log: Vec<String>,
        /// ctrl "PING" answers PONG while true
        pub ping_ok: bool,
        /// the canned SCAN_RESULTS reply once the scan finished
        pub scan_results: String,
        /// what SCAN_RESULTS answers until the scan-finished event
        /// came: the supplicant's cache of an earlier, partial scan
        pub stale_results: Option<String>,
        /// the scan-finished event arrives (`new` sets it; false = the
        /// supplicant never reports and the budget runs out)
        pub scan_event: bool,
        /// ATTACH is refused
        pub attach_fails: bool,
        /// successive SCAN_RESULTS replies, consumed one per call (the
        /// last one repeats) — the polling fallback sees the list grow
        pub scan_sequence: Vec<String>,
        /// the canned `ip -o -4 addr show dev <iface>` output
        /// ("" = no address yet)
        pub wlan_addrs: String,
        /// the canned `ip -o -4 addr show` output (all interfaces,
        /// the hotspot-subnet guard parses it)
        pub all_addrs: String,
        pub routable: bool,
        pub eth_up: bool,
        pub ap_client: bool,
        /// spawn() fails while true (a broken radio path)
        pub spawn_fails: bool,
        /// a supplicant for our interface is already running (the
        /// boot's adopt check)
        pub supplicant_running: bool,
        /// a lease client for our interface is already running
        pub udhcpc_running: bool,
        /// the canned exit status of the spawned supplicant
        /// (None = it runs) — the startup-failure visibility
        pub wpa_exit: Option<String>,
        /// the canned STATUS reply (the power-save catch-up reads
        /// the association identity from it)
        pub status: String,
        pub mac: String,
        /// the mDNS gate to sample on every `run`/`spawn`, and the
        /// phase it showed each time
        pub watch: Option<std::sync::Arc<super::super::gate::MdnsGate>>,
        pub watched: Vec<super::super::gate::GatePhase>,
    }

    impl FakeBackend {
        pub fn new(mac: &str) -> Self {
            Self {
                mac: mac.to_string(),
                scan_event: true,
                status: "wpa_state=COMPLETED\nbssid=2c:91:ab:12:34:56\n".into(),
                ..Self::default()
            }
        }

        pub fn log(&self) -> Vec<String> {
            self.log.clone()
        }

        fn sample_watch(&mut self) {
            if let Some(gate) = &self.watch {
                self.watched.push(gate.phase());
            }
        }
    }

    impl WifiBackend for FakeBackend {
        fn ctrl(&mut self, cmd: &str) -> Result<String, String> {
            self.log.push(format!("ctrl {cmd}"));
            match cmd {
                "PING" => {
                    if self.ping_ok {
                        Ok("PONG\n".into())
                    } else {
                        Err("wpa ctrl recv PING: timed out".into())
                    }
                }
                "SCAN" | "RECONFIGURE" => Ok("OK\n".into()),
                "SCAN_RESULTS" => {
                    if self.scan_sequence.len() > 1 {
                        return Ok(self.scan_sequence.remove(0));
                    }
                    if let Some(last) = self.scan_sequence.first() {
                        return Ok(last.clone());
                    }
                    Ok(self
                        .stale_results
                        .clone()
                        .unwrap_or_else(|| self.scan_results.clone()))
                }
                "STATUS" => Ok(self.status.clone()),
                other => Err(format!("no canned reply for {other}")),
            }
        }

        fn events_attach(&mut self) -> Result<(), String> {
            self.log.push("attach".into());
            if self.attach_fails {
                Err("wpa ctrl ATTACH: refused".into())
            } else {
                Ok(())
            }
        }

        fn wait_scan_finished(&mut self, _: Duration) -> Result<bool, String> {
            self.log.push("wait scan finished".into());
            if self.scan_event {
                // the scan is over: the cache now holds the full list
                self.stale_results = None;
            }
            Ok(self.scan_event)
        }

        fn events_detach(&mut self) {
            self.log.push("detach".into());
        }

        fn mac(&mut self) -> Result<String, String> {
            Ok(self.mac.clone())
        }

        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            self.log.push(format!("run {program} {}", args.join(" ")));
            self.sample_watch();
            if program == "ip" && args.contains(&"show") && !args.contains(&"dev") {
                return Ok(self.all_addrs.clone());
            }
            if program == "ip" && args.contains(&"wlan0") {
                return Ok(self.wlan_addrs.clone());
            }
            Ok(String::new())
        }

        fn spawn(&mut self, name: &str, program: &str, args: &[&str]) -> Result<(), String> {
            self.log
                .push(format!("spawn {name} {program} {}", args.join(" ")));
            self.sample_watch();
            if self.spawn_fails {
                Err(format!("spawn {program}: no radio"))
            } else {
                Ok(())
            }
        }

        fn kill_owned(&mut self, name: &str) {
            self.log.push(format!("kill {name}"));
        }

        fn child_exit_status(&mut self, name: &str) -> Option<String> {
            self.log.push(format!("exit status {name}"));
            (name == "wpa_supplicant")
                .then(|| self.wpa_exit.clone())
                .flatten()
        }

        fn count_foreign(&mut self, _exe: &str, applet: &str, _argv: &[&[&str]]) -> usize {
            match applet {
                "wpa_supplicant" => usize::from(self.supplicant_running),
                "udhcpc" => usize::from(self.udhcpc_running),
                _ => 0,
            }
        }

        fn kill_foreign(&mut self, exe: &str, applet: &str, argv: &[&[&str]]) {
            let groups: Vec<String> = argv.iter().map(|g| g.join(" ")).collect();
            self.log.push(format!(
                "kill foreign {exe} {applet} {}",
                groups.join(" + ")
            ));
        }

        fn clear_stale_ctrl_socket(&mut self) {
            self.log.push("clear stale ctrl socket".into());
        }

        fn routable(&mut self) -> bool {
            self.routable
        }

        fn eth_up(&mut self) -> bool {
            self.eth_up
        }

        fn ap_has_client(&mut self) -> bool {
            self.ap_client
        }

        fn sleep(&mut self, _: Duration) {}
    }
}

#[cfg(test)]
mod detection {
    use super::*;

    /// a fake sysfs tree: (name, has the wireless dir, operstate)
    fn tree(devs: &[(&str, bool, &str)]) -> Roots {
        let dir = tempfile::tempdir().unwrap();
        let net = dir.path().join("class/net");
        for (name, wireless, state) in devs {
            let dev = net.join(name);
            std::fs::create_dir_all(&dev).unwrap();
            if *wireless {
                std::fs::create_dir_all(dev.join("wireless")).unwrap();
            }
            std::fs::write(dev.join("operstate"), state).unwrap();
        }
        std::mem::forget(dir);
        Roots {
            net_class: net,
            proc: "/nonexistent-proc".into(),
        }
    }

    #[test]
    fn no_wireless_interface_is_an_error() {
        let roots = tree(&[("eth0", false, "up"), ("lo", false, "unknown")]);
        assert!(detect_wireless(&roots, "auto").is_err());
    }

    #[test]
    fn the_first_wireless_netdev_wins_in_directory_order() {
        let roots = tree(&[
            ("eth0", false, "up"),
            ("wlan1", true, "down"),
            ("wlan_ap0", true, "down"),
        ]);
        assert_eq!(detect_wireless(&roots, "auto").unwrap(), "wlan1");
    }

    #[test]
    fn an_up_interface_beats_an_earlier_down_one() {
        let roots = tree(&[("wlan1", true, "down"), ("wlp3s0", true, "up")]);
        assert_eq!(detect_wireless(&roots, "auto").unwrap(), "wlp3s0");
    }

    #[test]
    fn an_explicit_name_passes_through_without_probing() {
        let roots = tree(&[]);
        assert_eq!(detect_wireless(&roots, "wlanX").unwrap(), "wlanX");
        // even when nothing exists — the radio path reports it
        assert_eq!(detect_wireless(&roots, "eth0").unwrap(), "eth0");
    }
}

#[cfg(test)]
mod foreign_kills {
    use super::*;

    /// one fake /proc entry: exe symlink + NUL-separated argv
    fn proc_entry(root: &Path, pid: &str, exe: &str, argv: &[&str]) {
        let dir = root.join(pid);
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
        let cmdline = argv
            .iter()
            .map(|a| a.as_bytes())
            .chain(std::iter::once(&b""[..]))
            .collect::<Vec<_>>()
            .join(&0u8);
        std::fs::write(dir.join("cmdline"), &cmdline).unwrap();
    }

    fn tree() -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proc");
        std::fs::create_dir_all(&root).unwrap();
        std::mem::forget(dir);
        root
    }

    #[test]
    fn an_exact_match_dies_joined_and_foreign_args_do_not() {
        let root = tree();
        // the boot-started supplicant on our interface — argv[0] may
        // be the full path (start-stop-daemon execs it that way)
        proc_entry(
            &root,
            "101",
            WPA_SUPPLICANT,
            &[WPA_SUPPLICANT, "-i", "wlan0", "-c", "/data/x"],
        );
        // `-i` and the interface glued into ONE arg: not a match
        proc_entry(&root, "102", WPA_SUPPLICANT, &["wpa_supplicant", "-iwlan0"]);
        // a different interface: not a match
        proc_entry(
            &root,
            "103",
            WPA_SUPPLICANT,
            &["wpa_supplicant", "-i", "wlan1"],
        );
        // the interface name inside another argument: not a match
        proc_entry(
            &root,
            "104",
            WPA_SUPPLICANT,
            &["wpa_supplicant", "--note", "wlan0"],
        );
        // a different binary with the same args: not a match
        proc_entry(&root, "105", "/usr/sbin/other", &["x", "-i", "wlan0"]);
        assert_eq!(
            foreign_pids(&root, WPA_SUPPLICANT, "wpa_supplicant", &[&["-i", "wlan0"]]),
            vec![101],
            "only the exact supplicant matches"
        );
    }

    #[test]
    fn a_busybox_applet_is_verified_by_argv0() {
        let root = tree();
        // udhcpc is a busybox applet: the exe link is the busybox
        // binary, the applet rides in argv[0]
        proc_entry(&root, "201", BUSYBOX, &["udhcpc", "-i", "wlan0", "-f"]);
        // ANOTHER applet with the same args must not die for it
        proc_entry(&root, "202", BUSYBOX, &["ifplugd", "-i", "wlan0"]);
        // busybox without the interface flag: never matched
        proc_entry(&root, "203", BUSYBOX, &["ntpd", "-n"]);
        assert_eq!(
            foreign_pids(&root, BUSYBOX, "udhcpc", &[&["-i", "wlan0"]]),
            vec![201],
            "ifplugd must not die for udhcpc"
        );
        assert!(foreign_pids(&root, BUSYBOX, "udhcpc", &[&["-i", "wlan1"]]).is_empty());
        // a missing proc root is empty, not an error
        assert!(foreign_pids(
            Path::new("/nonexistent-proc"),
            BUSYBOX,
            "udhcpc",
            &[&["-i", "wlan0"]]
        )
        .is_empty());
    }

    #[test]
    fn a_stray_dnsmasq_is_matched_by_its_exact_identity() {
        // the startup cleanup after an exec lost the child handles:
        // OUR dnsmasq is the one with our interface and our lease
        // file in the argv — no other dnsmasq dies
        let root = tree();
        let ours = [
            "--no-daemon",
            "--interface=wlan0",
            &format!("--dhcp-leasefile={LEASES}"),
        ];
        proc_entry(
            &root,
            "301",
            DNSMASQ,
            &["dnsmasq", ours[0], ours[1], ours[2]],
        );
        // a different interface: not ours
        proc_entry(
            &root,
            "302",
            DNSMASQ,
            &["dnsmasq", "--interface=wlan1", ours[2]],
        );
        // a different lease file (someone else's dnsmasq): not ours
        proc_entry(
            &root,
            "303",
            DNSMASQ,
            &["dnsmasq", ours[1], "--dhcp-leasefile=/tmp/other"],
        );
        // a glued interface arg: not ours
        proc_entry(
            &root,
            "304",
            DNSMASQ,
            &["dnsmasq", "--interfacewlan0", ours[2]],
        );
        assert_eq!(
            foreign_pids(&root, DNSMASQ, "dnsmasq", &[&[ours[1]], &[ours[2]]],),
            vec![301]
        );
    }

    #[test]
    fn the_mac_reads_the_netdev_tree_not_a_doubled_sysfs_path() {
        let dir = tempfile::tempdir().unwrap();
        let net = dir.path().join("class/net");
        std::fs::create_dir_all(net.join("wlan0")).unwrap();
        std::fs::write(net.join("wlan0").join("address"), "D8:3A:DD:77:34:56\n").unwrap();
        let mut backend = RealBackend::new(
            "wlan0",
            "/run/wpa_supplicant",
            &net,
            Path::new("/proc"),
            std::sync::Arc::new(super::super::Children::default()),
        );
        assert_eq!(backend.mac().unwrap(), "d8:3a:dd:77:34:56");
    }
}

#[cfg(test)]
mod shutdown_stop {
    use super::*;

    /// one registry with `sleep` children under the given names —
    /// real processes, so the kill semantics are real too
    fn registry(names: &[&str]) -> std::sync::Arc<super::super::Children> {
        let children = std::sync::Arc::new(super::super::Children::default());
        for name in names {
            let child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            locks::lock(&children).insert((*name).to_string(), child);
        }
        children
    }

    /// alive: still present in the registry and not exited
    fn alive(children: &super::super::Children, name: &str) -> bool {
        locks::lock(children)
            .get_mut(name)
            .is_some_and(|child| child.try_wait().unwrap().is_none())
    }

    #[test]
    fn the_pre_exec_stop_spares_a_working_sta() {
        // after a successful AP→STA join the restart must NOT take
        // the STA down: only dnsmasq dies, the supplicant and the
        // lease client survive the exec
        let children = registry(&["wpa_supplicant", "udhcpc"]);
        stop_ap_processes(&children, false);
        assert!(
            !alive(&children, "dnsmasq"),
            "dnsmasq was never tracked here"
        );
        assert!(
            alive(&children, "wpa_supplicant"),
            "the STA supplicant must survive"
        );
        assert!(alive(&children, "udhcpc"), "the lease client must survive");

        // in AP mode the tracked supplicant IS the AP one — it goes
        let ap = registry(&["dnsmasq", "wpa_supplicant", "udhcpc"]);
        stop_ap_processes(&ap, true);
        assert!(!alive(&ap, "dnsmasq"));
        assert!(!alive(&ap, "wpa_supplicant"));
        assert!(
            alive(&ap, "udhcpc"),
            "the lease client survives even in AP mode"
        );
        // cleanup: the survivors must not leak into the test machine
        for registry in [children, ap] {
            for (_, mut child) in locks::lock(&registry).drain() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[test]
    fn the_exec_fallback_kills_every_tracked_child() {
        // no confirmation in time: the restart path kills them all
        // by handle — the next start's ensure_sta rebuilds the STA
        let shutdown = super::super::WifiShutdown::default();
        for name in ["dnsmasq", "wpa_supplicant", "udhcpc"] {
            let child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            shutdown
                .children_arc()
                .lock()
                .unwrap()
                .insert(name.to_string(), child);
        }
        shutdown.stop_tracked();
        assert!(shutdown.children_arc().lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod kill_waits {
    //! kill_foreign's wait contract against REAL processes and the
    //! real /proc: SIGTERM'd, waited for, SIGKILL'd when stuck
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;
    use std::sync::Arc;

    fn backend(proc_root: &Path) -> RealBackend {
        RealBackend::new(
            "wlan0",
            "/run/wpa_supplicant",
            Path::new("/sys/class/net"),
            proc_root,
            Arc::new(super::super::Children::default()),
        )
    }

    /// the matched processes are `sh` running a script UNIQUE to
    /// the test — exe link + the script as an argv entry hit only
    /// ours (a copied binary races ETXTBSY under the parallel
    /// test threads on some machines)
    fn shell(pid: u32) -> PathBuf {
        std::fs::read_link(format!("/proc/{pid}/exe")).unwrap()
    }

    #[test]
    fn kill_foreign_waits_until_the_process_is_gone() {
        // a SIGTERM'd process may take a moment to wind down — the
        // call must not return while it lives (the fallback spawns
        // the next supplicant on the same iface right after)
        let script = "trap 'sleep 0.7; exit' TERM; echo ready; while :; do :; done";
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // the trap must be in place before the SIGTERM lands —
        // the script announces itself
        let mut ready = [0u8; 6];
        std::io::Read::read_exact(&mut child.stdout.take().expect("piped stdout"), &mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready\n", "the shell announced its trap");
        let mut backend = backend(Path::new("/proc"));
        let started = Instant::now();
        backend.kill_foreign(
            &shell(child.id()).to_string_lossy(),
            "sh",
            &[&["-c"], &[script]],
        );
        let status = child
            .try_wait()
            .expect("kill_foreign returned with the process gone")
            .expect("the shell exited");
        assert!(
            status.success(),
            "the shell wound down on its own: {status}"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "the call waited out the shell's slow exit"
        );
    }

    #[test]
    fn kill_foreign_escalates_to_sigkill_when_sigterm_is_ignored() {
        // `sh` with TERM trapped to empty ignores SIGTERM and
        // busy-loops — after the grace it must die by SIGKILL
        let script = "trap '' TERM; echo ready; while :; do :; done";
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // the trap must be in place before the SIGTERM lands —
        // the script announces itself
        let mut ready = [0u8; 6];
        std::io::Read::read_exact(&mut child.stdout.take().expect("piped stdout"), &mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready\n", "the shell announced its trap");
        let exe = shell(child.id());
        let mut backend = backend(Path::new("/proc"));
        let started = Instant::now();
        backend.kill_foreign(&exe.to_string_lossy(), "sh", &[&["-c"], &[script]]);
        let status = child
            .try_wait()
            .expect("kill_foreign returned with the process gone")
            .expect("the stubborn shell exited");
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert!(
            started.elapsed() >= FOREIGN_TERM_WAIT,
            "the SIGTERM grace elapsed before the escalation"
        );
    }

    #[test]
    fn a_live_process_is_not_gone_and_a_zombie_is() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(!process_gone(Path::new("/proc"), pid), "alive is not gone");
        child.kill().unwrap();
        // unreaped: a zombie holds no socket — it counts as gone
        let deadline = Instant::now() + Duration::from_secs(2);
        while !process_gone(Path::new("/proc"), pid) {
            assert!(
                Instant::now() < deadline,
                "the killed sleeper became a zombie"
            );
            std::thread::sleep(FOREIGN_WAIT_STEP);
        }
        child.wait().unwrap();
    }

    /// a fake /proc entry: exe symlink + NUL-separated argv
    fn proc_entry(root: &Path, pid: &str, exe: &str, argv: &[&str]) {
        let dir = root.join(pid);
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
        let cmdline = argv
            .iter()
            .map(|a| a.as_bytes())
            .chain(std::iter::once(&b""[..]))
            .collect::<Vec<_>>()
            .join(&0u8);
        std::fs::write(dir.join("cmdline"), &cmdline).unwrap();
    }

    #[test]
    fn the_stale_ctrl_socket_goes_only_when_no_supplicant_owns_it() {
        let dir = tempfile::tempdir().unwrap();
        let ctrl = dir.path().join("ctrl");
        std::fs::create_dir_all(&ctrl).unwrap();
        let socket = ctrl.join("wlan0");
        std::fs::write(&socket, b"left by a corpse").unwrap();
        let proc = dir.path().join("proc");
        std::fs::create_dir_all(&proc).unwrap();
        let mut backend = RealBackend::new(
            "wlan0",
            &ctrl.to_string_lossy(),
            Path::new("/sys/class/net"),
            &proc,
            Arc::new(super::super::Children::default()),
        );
        backend.clear_stale_ctrl_socket();
        assert!(!socket.exists(), "no owner — the stale socket is gone");

        // a supplicant holds the interface: its socket stays
        std::fs::write(&socket, b"live").unwrap();
        proc_entry(
            &proc,
            "500",
            WPA_SUPPLICANT,
            &["wpa_supplicant", "-i", "wlan0"],
        );
        backend.clear_stale_ctrl_socket();
        assert!(socket.exists(), "a live supplicant's socket is not stale");
    }
}
