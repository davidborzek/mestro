//! mestro-update — the player image's A/B update tool: the CLI over
//! [`mestro_update`], the boot chain detected at runtime, on no UI
//! stack at all — it must work when the player cannot start. It
//! makes the slot store reachable, runs one update command and lets
//! go of it again; the update logic and its tests live in the
//! library.
//!
//! Exit codes like the shell script it replaces: 2 for a usage error
//! or a missing source file, 1 for a failed update, 0 otherwise.

use clap::{Parser, Subcommand};
use mestro_update::external::{run_bounded, run_bounded_output};
use mestro_update::trial::{self, GuardWorld, Outcome, TrialWorld};
use mestro_update::{self as update, BootControl, Commit, Detection, Error, UpdateLock};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "mestro-update",
    about = "the player image's A/B system update and reboot",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// write the inactive boot slot and arm a trial boot of it
    Install {
        /// start the trial boot right away
        #[arg(long)]
        reboot: bool,
        /// the new system image: a file, or - for stdin (streamed,
        /// never buffered in RAM)
        file: String,
        /// the expected sha256 of the image
        sha256: Option<String>,
    },
    /// make the trial slot permanent (the trial guard, once the
    /// trial boot came up healthy)
    Commit,
    /// drop a pending trial that never committed (a normal boot's
    /// trial command does this on its own)
    Abandon,
    /// the running and a pending slot
    Status,
    /// guard and decide a trial boot (S04trial starts it): feed
    /// the SoC watchdog, check the new image, commit it healthy or
    /// roll back — on a normal boot, drop a trial that never
    /// committed
    Trial,
    /// clean reboot (busybox reboot hangs the initramfs kiosk);
    /// --tryboot boots the pending trial once
    Reboot {
        /// one trial boot of the prepared slot
        #[arg(long)]
        tryboot: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let mut backend = match update::detect() {
        Detection::Backend(backend) => backend,
        Detection::NotCompiledIn => {
            eprintln!("no boot control compiled in (build with a backend feature, e.g. rpi)");
            std::process::exit(1);
        }
        Detection::UnsupportedBoard => {
            eprintln!("no supported boot control on this board");
            std::process::exit(1);
        }
    };
    std::process::exit(run(cli.command, &mut *backend));
}

fn run(command: Commands, backend: &mut dyn BootControl) -> i32 {
    match command {
        Commands::Status => {
            let mut out = std::io::stdout();
            mounted(backend, &mut out, status)
        }
        Commands::Reboot { tryboot } => reboot(backend, tryboot),
        Commands::Trial => trial(backend),
        // every write operation holds the cross-process update lock
        // to its end — the web OTA and the trial guard take the same
        // one. An install must not wait for a running update; a commit
        // or an abandon gets the trial guard's health budget
        Commands::Install {
            reboot,
            file,
            sha256,
        } => {
            let Some(_lock) = write_lock(Duration::ZERO) else {
                return 1;
            };
            install(backend, reboot, &file, sha256.as_deref())
        }
        Commands::Commit => {
            let Some(_lock) = write_lock(DECISION_LOCK_BOUND) else {
                return 1;
            };
            let mut out = std::io::stdout();
            mounted(backend, &mut out, commit)
        }
        Commands::Abandon => {
            let Some(_lock) = write_lock(DECISION_LOCK_BOUND) else {
                return 1;
            };
            let mut out = std::io::stdout();
            mounted(backend, &mut out, abandon)
        }
    }
}

/// how long a commit or an abandon waits for a running update to
/// finish: the trial guard decides within two minutes of boot, and
/// one upload must not roll a healthy trial back by starving it
const DECISION_LOCK_BOUND: std::time::Duration = std::time::Duration::from_secs(30);

/// the cross-process update lock — `None` (with the message out)
/// when it cannot be taken within `bound`
fn write_lock(bound: std::time::Duration) -> Option<UpdateLock> {
    match update::UpdateLock::wait_acquire(bound) {
        Ok(lock) => Some(lock),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            eprintln!("another update is running");
            None
        }
        Err(e) => {
            eprintln!("cannot lock the update: {e}");
            None
        }
    }
}

/// run one command with the slot store reachable
fn mounted(
    backend: &mut dyn BootControl,
    out: &mut dyn Write,
    run: fn(&mut dyn Write, &mut dyn BootControl) -> Result<(), Error>,
) -> i32 {
    if let Err(e) = backend.mount() {
        eprintln!("{e}");
        return 1;
    }
    let code = match run(out, backend) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    };
    backend.release();
    code
}

fn install(
    backend: &mut dyn BootControl,
    reboot_after: bool,
    file: &str,
    expected_sha256: Option<&str>,
) -> i32 {
    // the source is checked before anything is mounted
    let mut source = match Source::open(file) {
        Ok(source) => source,
        Err(()) => return 2,
    };
    let mut out = std::io::stdout();
    if let Err(e) = backend.mount() {
        eprintln!("{e}");
        return 1;
    }
    // this boot may be a trial that has not been decided yet. The
    // trial file lives on the slot store, so the check needs the
    // store mounted — an install would delete the tryboot.txt its
    // decision still needs
    let undecided = match update::undecided_trial(backend) {
        Ok(undecided) => undecided,
        Err(e) => {
            eprintln!("{e}");
            backend.release();
            return 1;
        }
    };
    if undecided {
        eprintln!("cannot update: this boot is an undecided trial (commit or abandon it first)");
        backend.release();
        return 1;
    }
    let result = install_image(&mut out, backend, &mut source, expected_sha256);
    backend.release();
    match result {
        Ok(()) if reboot_after => {
            println!("trial boot …");
            reboot(backend, true)
        }
        Ok(()) => {
            println!("trial boot with: mestro-reboot --tryboot");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// where the image comes from: a file, or stdin
struct Source(Box<dyn Read>);

impl Source {
    /// `Err(())`: the file does not exist (the message is printed)
    fn open(file: &str) -> Result<Self, ()> {
        if file == "-" {
            Ok(Self(Box::new(std::io::stdin())))
        } else if Path::new(file).is_file() {
            match std::fs::File::open(file) {
                Ok(f) => Ok(Self(Box::new(f))),
                Err(e) => {
                    eprintln!("cannot read {file}: {e}");
                    Err(())
                }
            }
        } else {
            eprintln!("no such file: {file}");
            Err(())
        }
    }
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

/// stream the image in, with the progress lines the image's logs know
fn install_image(
    out: &mut dyn Write,
    backend: &mut dyn BootControl,
    image: &mut dyn Read,
    expected_sha256: Option<&str>,
) -> Result<(), Error> {
    let mut target = backend.begin_update()?;
    writeln!(
        out,
        "writing {} (running: {}) …",
        target.slot, target.running
    )?;
    let installed = update::install(backend, &mut target, image, expected_sha256)?;
    if expected_sha256.is_none() {
        writeln!(out, "no sha256 given; written: {}", installed.sha256)?;
    }
    writeln!(out, "ready: {} ({})", installed.slot, installed.sha256)?;
    Ok(())
}

fn commit(out: &mut dyn Write, backend: &mut dyn BootControl) -> Result<(), Error> {
    match backend.commit()? {
        Commit::Committed { kernel } => writeln!(out, "committed: {kernel}")?,
        Commit::NoTrial => writeln!(out, "no trial boot to commit")?,
    }
    Ok(())
}

fn abandon(out: &mut dyn Write, backend: &mut dyn BootControl) -> Result<(), Error> {
    if let Some(kernel) = backend.abandon().map_err(Error::from)? {
        writeln!(out, "trial of {kernel} never committed, dropped")?;
    }
    Ok(())
}

fn status(out: &mut dyn Write, backend: &mut dyn BootControl) -> Result<(), Error> {
    write!(out, "{}", update::status(backend).map_err(Error::from)?)?;
    Ok(())
}

fn reboot(backend: &mut dyn BootControl, into_trial: bool) -> i32 {
    match backend.reboot(into_trial) {
        // never reached on the board: a successful reboot does not return
        Ok(()) => 0,
        Err(e) => {
            eprintln!("reboot: {e}");
            1
        }
    }
}

/// the trial boot's guard and decision — one process for both init
/// scripts it replaces (S04trial's watchdog guard, S99ota's health
/// check). On a normal boot only the leftover is dropped: a trial
/// that never committed (crash, hang, rollback)
fn trial(backend: &mut dyn BootControl) -> i32 {
    if !backend.trial_boot() {
        // a normal boot's leftover: a pending trial that never
        // committed (crash, hang, rollback) — dropped, under the
        // lock like every write operation
        let Some(_lock) = write_lock(DECISION_LOCK_BOUND) else {
            return 1;
        };
        let mut out = std::io::stdout();
        return normal_boot(backend, &mut out);
    }
    // the image keeps its logs where mestro-os says (XDG_STATE_HOME
    // on the boot scripts is /data/state)
    let trial_dir = mestro_os::logs::trial_boot_dir(&mestro_os::paths::state_dir());
    if std::env::var_os("XDG_STATE_HOME").is_none() {
        // a missing XDG_STATE_HOME puts the snapshots under $HOME,
        // in RAM on the image: a rolled-back trial would leave none
        eprintln!(
            "trial boot: XDG_STATE_HOME is not set, the trial logs go to {} — they won't survive a rollback",
            trial_dir.display()
        );
    }
    let log_dir = PathBuf::from(mestro_os::logs::LOG_DIR);
    // the last trial's leftovers: this boot replaces them
    let _ = std::fs::remove_dir_all(&trial_dir);
    // the log snapshots on their own worker thread: the feed thread
    // does nothing but feed, and the decision never copies a file —
    // it requests a copy at its decisions and waits bounded
    let snapshot = SnapshotWorker::start(
        {
            let (log_dir, trial_dir) = (log_dir.clone(), trial_dir.clone());
            Arc::new(move || snapshot_logs(&log_dir, &trial_dir))
        },
        SNAPSHOT_CADENCE,
        FINAL_SNAPSHOT_WAIT,
    );
    // the guard on its own thread — feeding every 5 s, nothing else
    let guard = trial::GuardThread::new(GuardRunner::new());
    let mut runner = TrialRunner {
        backend,
        trial_dir,
        snapshot,
    };
    match trial::decide(&mut runner, &guard) {
        Outcome::Committed | Outcome::RolledBack => 0,
        // the decision never came, or the guard died after a
        // commit: the guard stopped feeding with the watchdog open —
        // this thread parks with it, and the reset lands on
        // whatever slot config.txt names, the previous one or the
        // new one the commit already switched (the board resets
        // long before the hour is up)
        Outcome::WatchdogReset => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
    }
}

/// a normal boot: a pending trial never committed — drop it. The
/// lock is the caller's (the trial command holds it); this is the
/// core the tests run
fn normal_boot(backend: &mut dyn BootControl, out: &mut dyn Write) -> i32 {
    mounted(backend, out, abandon)
}

/// the SoC watchdog the trial feeds, and the kernel's uptime and
/// route tables
const WATCHDOG: &str = "/dev/watchdog";
const UPTIME: &str = "/proc/uptime";
const ROUTES: &str = "/proc/net/route";
/// the web endpoint the health check asks — the old script's URL
const WEB_URL: &str = "127.0.0.1:80";
/// how long one probe's command may take (pidof, ip, date)
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// how long dmesg and the snapshot's sync may take
const DUMP_TIMEOUT: Duration = Duration::from_secs(10);
/// the web endpoint's answer window (the script's wget -T 3)
const WEB_TIMEOUT: Duration = Duration::from_secs(3);
/// how often the snapshot worker copies the logs on its own —
/// the old guard's ten-second cadence
const SNAPSHOT_CADENCE: Duration = Duration::from_secs(10);
/// how long a decision waits for its final snapshot — a copy
/// still running finishes on its own, the decision goes on
const FINAL_SNAPSHOT_WAIT: Duration = Duration::from_secs(5);

/// seconds since the kernel started — the decision's clock (the
/// guard's world derives its own, so a feed reads no file)
fn uptime() -> u64 {
    std::fs::read_to_string(UPTIME)
        .ok()
        .and_then(|uptime| uptime.split('.').next().map(str::parse))
        .and_then(Result::ok)
        .unwrap_or(0)
}

/// copy the logs into the trial-boot dir — the snapshot worker's
/// whole job; nobody else copies a file during a trial
fn snapshot_logs(log_dir: &Path, trial_dir: &Path) {
    if std::fs::create_dir_all(trial_dir).is_err() {
        return;
    }
    let _ = std::fs::copy(log_dir.join("ota.log"), trial_dir.join("ota.log"));
    let _ = std::fs::copy(log_dir.join("mestro.log"), trial_dir.join("mestro.log"));
    let dmesg = run_bounded_output("dmesg", |command| command, DUMP_TIMEOUT)
        .map(|(_, out)| out)
        .unwrap_or_default();
    let _ = std::fs::write(trial_dir.join("dmesg.txt"), dmesg);
    let _ = run_bounded("sync", |command| command, DUMP_TIMEOUT);
}

/// the trial's log snapshots on their own worker thread: one copy
/// right at the start and then every [SNAPSHOT_CADENCE], plus one
/// on request at the decisions. Nobody waits on the worker
/// unbounded: a request that lands behind a running copy waits at
/// most [FINAL_SNAPSHOT_WAIT], then the decision goes on and the
/// copy finishes on its own
struct SnapshotWorker {
    requests: mpsc::Sender<SnapshotRequest>,
    /// how long a decision's final request waits (a test seam —
    /// [FINAL_SNAPSHOT_WAIT] on the image)
    final_wait: Duration,
}

/// one copy, and who wants to hear it finished
struct SnapshotRequest {
    reply: Option<mpsc::Sender<()>>,
}

impl SnapshotWorker {
    /// start the worker on its own thread, copying through `take`
    /// (the real one copies the logs; the tests gate it). A thread
    /// that cannot start is no worker: the requests below just
    /// fail, the decisions go on without their snapshots
    fn start(
        take: Arc<dyn Fn() + Send + Sync + 'static>,
        cadence: Duration,
        final_wait: Duration,
    ) -> Self {
        let (requests, incoming) = mpsc::channel::<SnapshotRequest>();
        let spawn = std::thread::Builder::new().spawn(move || {
            // the old guard's first snapshot was its loop's first
            // turn: a trial that dies early still leaves one
            take();
            loop {
                match incoming.recv_timeout(cadence) {
                    Ok(request) => {
                        take();
                        if let Some(reply) = request.reply {
                            let _ = reply.send(());
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => take(),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        // the handle drops here: the worker runs for the process's
        // life, detached — nobody waits for its end
        let _ = spawn;
        Self {
            requests,
            final_wait,
        }
    }

    /// the decisions' final snapshot: one copy now, waited for at
    /// most the worker's bound — then the decision goes on
    fn snapshot_now(&self) {
        let (reply, done) = mpsc::channel();
        if self
            .requests
            .send(SnapshotRequest { reply: Some(reply) })
            .is_ok()
        {
            let _ = done.recv_timeout(self.final_wait);
        }
        // a worker that never started, or a copy still running:
        // its snapshot happens whenever it happens
    }
}

/// the decision's world: the probes as bounded commands and /proc
/// reads, the summary where the image keeps its logs. The watchdog
/// is not here — the guard thread's world owns it — and neither
/// are the log copies: a snapshot at a decision is a bounded
/// request to the worker
struct TrialRunner<'a> {
    backend: &'a mut dyn BootControl,
    trial_dir: PathBuf,
    snapshot: SnapshotWorker,
}

/// the guard's world: the SoC watchdog as a std file. The file
/// sits behind the world's Arc — the guard thread's end at its
/// deadline must not close it, the parked process keeps it open
/// until the board's reset. The mutex is the feed thread's private
/// one: armed once before the thread starts, then touched by
/// nothing else — no lock in the feeding path is ever contended
struct GuardRunner {
    /// the trial's age when armed — captured once, the loop
    /// derives the rest from the monotonic clock, so a feed reads
    /// no file at all
    armed_at: AtomicU64,
    started: OnceLock<std::time::Instant>,
    watchdog: Mutex<Option<std::fs::File>>,
}

impl GuardRunner {
    fn new() -> Self {
        Self {
            armed_at: AtomicU64::new(0),
            started: OnceLock::new(),
            watchdog: Mutex::new(None),
        }
    }
}

impl GuardWorld for GuardRunner {
    fn uptime(&self) -> u64 {
        // the captured arm moment plus the monotonic clock: a feed
        // reads no file
        self.armed_at.load(Ordering::SeqCst)
            + self
                .started
                .get()
                .map_or(0, |started| started.elapsed().as_secs())
    }

    fn sleep(&self, step: Duration) {
        std::thread::sleep(step);
    }

    fn log(&self, line: &str) {
        // stdout is the process's ota.log (the init script appends
        // it there)
        println!("{line}");
    }

    fn arm(&self) {
        // capture the clock first, so the deadline starts here
        self.armed_at.store(uptime(), Ordering::SeqCst);
        let _ = self.started.set(std::time::Instant::now());
        match std::fs::OpenOptions::new().write(true).open(WATCHDOG) {
            Ok(file) => {
                if let Ok(mut watchdog) = self.watchdog.lock() {
                    *watchdog = Some(file);
                }
            }
            Err(_) => self.log("no /dev/watchdog, the trial runs unguarded"),
        }
    }

    fn feed(&self) {
        // any byte feeds it — the old script wrote a dot
        if let Ok(mut watchdog) = self.watchdog.lock() {
            if let Some(watchdog) = watchdog.as_mut() {
                let _ = watchdog.write_all(b".");
            }
        }
    }

    fn disarm(&self) {
        // the magic 'V': the driver disarms itself on this close
        if let Ok(mut watchdog) = self.watchdog.lock() {
            if let Some(mut watchdog) = watchdog.take() {
                let _ = watchdog.write_all(b"V");
            }
        }
    }
}

impl TrialWorld for TrialRunner<'_> {
    fn uptime(&mut self) -> u64 {
        uptime()
    }

    fn mestro_pid(&mut self) -> Option<u32> {
        let Ok((status, out)) =
            run_bounded_output("pidof", |command| command.arg("mestro"), PROBE_TIMEOUT)
        else {
            return None;
        };
        if !status.success() {
            return None;
        }
        String::from_utf8_lossy(&out)
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    fn network_up(&mut self) -> bool {
        // an IPv4 address on wlan0/eth0 plus a default route, or an
        // address on wlan0 alone (the setup hotspot hands out its
        // own subnet)
        let routes = std::fs::read_to_string(ROUTES).unwrap_or_default();
        for dev in ["wlan0", "eth0"] {
            if !ipv4_on(dev) {
                continue;
            }
            if trial::route_has_default(&routes, dev) {
                return true;
            }
            if dev == "wlan0" {
                return true;
            }
        }
        false
    }

    fn web_up(&mut self) -> bool {
        web_up(WEB_URL)
    }

    fn sleep(&mut self, step: Duration) {
        std::thread::sleep(step);
    }

    fn log(&mut self, line: &str) {
        // stdout is the process's ota.log (the init script appends
        // it there)
        println!("{line}");
    }

    fn snapshot(&mut self) {
        // the bounded request to the worker — the decision never
        // copies a file itself
        self.snapshot.snapshot_now();
    }

    fn write_summary(&mut self, result: &str) {
        if std::fs::create_dir_all(&self.trial_dir).is_err() {
            return;
        }
        let mut summary = String::new();
        if let Ok((status, date)) = run_bounded_output("date", |command| command, PROBE_TIMEOUT) {
            if status.success() {
                summary.push_str(&String::from_utf8_lossy(&date));
            }
        }
        if self.backend.mount().is_ok() {
            if let Ok(status) = update::status(self.backend) {
                summary.push_str(&status);
            }
            self.backend.release();
        }
        summary.push_str(&format!("result: {result}\n"));
        for output in [["addr"], ["route"]] {
            if let Ok((status, out)) = run_bounded_output(
                "ip",
                |command| command.arg("-4").args(output),
                PROBE_TIMEOUT,
            ) {
                if status.success() {
                    summary.push_str(&String::from_utf8_lossy(&out));
                }
            }
        }
        let _ = std::fs::write(self.trial_dir.join("summary.txt"), summary);
        let _ = run_bounded("sync", |command| command, DUMP_TIMEOUT);
    }

    fn commit(&mut self) -> bool {
        // the same bounded lock and the same lines as the commit
        // subcommand — one updater across processes
        let Some(_lock) = write_lock(DECISION_LOCK_BOUND) else {
            return false;
        };
        if let Err(e) = self.backend.mount() {
            eprintln!("{e}");
            return false;
        }
        let mut out = std::io::stdout();
        let result = commit(&mut out, self.backend);
        self.backend.release();
        match result {
            Ok(()) => true,
            Err(e) => {
                eprintln!("{e}");
                false
            }
        }
    }

    fn reboot_rollback(&mut self) -> bool {
        // a normal restart: the firmware's tryboot flag is gone,
        // the previous slot boots
        reboot(self.backend, false) == 0
    }
}

/// the health check: a short-deadline GET of the pairing status,
/// answered with HTTP 200. `url` stands in for the image's
/// [WEB_URL] so the tests can run a server
fn web_up(url: &str) -> bool {
    let Ok(addr) = url.parse::<std::net::SocketAddr>() else {
        return false;
    };
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, WEB_TIMEOUT) else {
        return false;
    };
    let request = b"GET /pair/status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(request).is_err() {
        return false;
    }
    // TCP may split the status line across reads: read until its
    // end (or 64 bytes), under the one deadline, then parse the code
    let deadline = std::time::Instant::now() + WEB_TIMEOUT;
    let mut status = Vec::with_capacity(64);
    let mut chunk = [0u8; 64];
    while !status.contains(&b'\n') && status.len() < 64 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        if stream.set_read_timeout(Some(left)).is_err() {
            return false;
        }
        match stream.read(&mut chunk) {
            // the server closed: parse what arrived
            Ok(0) => break,
            Ok(read) => status.extend_from_slice(&chunk[..read]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // the deadline ran out mid-line, or the socket broke
            Err(_) => return false,
        }
    }
    String::from_utf8_lossy(&status)
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        == Some("200")
}

/// whether the interface has an IPv4 address — the old script's
/// `ip -4 addr show dev X | grep "inet "`, bounded so a wedged
/// command cannot stop the watchdog feed
fn ipv4_on(dev: &str) -> bool {
    let Ok((status, out)) = run_bounded_output(
        "ip",
        |command| command.args(["-4", "addr", "show", "dev", dev]),
        PROBE_TIMEOUT,
    ) else {
        return false;
    };
    status.success() && String::from_utf8_lossy(&out).contains("inet ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mestro_update::UpdateTarget;
    use std::sync::{Arc, Mutex};

    /// a boot chain that records what the CLI asked of it — the
    /// CLI's own lines do not depend on the board
    struct FakeBoot {
        slot: String,
        running: String,
        card: Arc<Mutex<Vec<u8>>>,
        committed: Option<Commit>,
        abandoned: Option<String>,
        rebooted: Option<bool>,
        /// this boot is a trial whose decision is still pending
        undecided_trial: bool,
        /// whether the store is mounted — the trial is only
        /// visible then, like tryboot.txt on the real boot
        /// partition
        mounted: bool,
    }

    impl FakeBoot {
        fn new() -> Self {
            Self {
                slot: "Image.b".into(),
                running: "Image.a".into(),
                card: Arc::new(Mutex::new(Vec::new())),
                committed: None,
                abandoned: None,
                rebooted: None,
                undecided_trial: false,
                mounted: false,
            }
        }
    }

    /// the store end of [`FakeBoot`]
    struct CardSink(Arc<Mutex<Vec<u8>>>);

    impl Write for CardSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl BootControl for FakeBoot {
        fn mount(&mut self) -> Result<(), Error> {
            self.mounted = true;
            Ok(())
        }
        fn release(&mut self) {}
        fn active(&self) -> std::io::Result<String> {
            Ok(self.running.clone())
        }
        fn pending(&self) -> std::io::Result<Option<String>> {
            Ok((self.undecided_trial && self.mounted).then(|| self.slot.clone()))
        }
        fn trial_boot(&self) -> bool {
            self.undecided_trial
        }
        fn begin_update(&mut self) -> Result<UpdateTarget, Error> {
            Ok(UpdateTarget {
                slot: self.slot.clone(),
                running: self.running.clone(),
                sink: Box::new(CardSink(self.card.clone())),
            })
        }
        fn read_back(&mut self, _target: &UpdateTarget) -> std::io::Result<String> {
            Ok(sha_of(&self.card.lock().unwrap()))
        }
        fn discard(&mut self, _target: &UpdateTarget) {}
        fn arm_trial(&mut self, _target: &UpdateTarget) -> std::io::Result<()> {
            Ok(())
        }
        fn commit(&mut self) -> Result<Commit, Error> {
            Ok(self.committed.clone().unwrap_or(Commit::NoTrial))
        }
        fn abandon(&mut self) -> std::io::Result<Option<String>> {
            Ok(self.abandoned.take())
        }
        fn reboot(&mut self, into_trial: bool) -> std::io::Result<()> {
            self.rebooted = Some(into_trial);
            Ok(())
        }
    }

    fn sha_of(bytes: &[u8]) -> String {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn install_prints_the_progress_lines_with_a_sha() {
        let mut backend = FakeBoot::new();
        let mut out = Vec::new();
        let sha = sha_of(b"image");
        install_image(&mut out, &mut backend, &mut &b"image"[..], Some(&sha)).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("writing Image.b (running: Image.a) …\nready: Image.b ({sha})\n")
        );
    }

    #[test]
    fn install_without_a_sha_prints_the_written_hash() {
        let mut backend = FakeBoot::new();
        let mut out = Vec::new();
        install_image(&mut out, &mut backend, &mut &b"image"[..], None).unwrap();
        let sha = sha_of(b"image");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "writing Image.b (running: Image.a) …\nno sha256 given; written: {sha}\nready: Image.b ({sha})\n"
            )
        );
    }

    #[test]
    fn a_failed_install_prints_nothing_but_the_writing_line() {
        let mut backend = FakeBoot::new();
        let mut out = Vec::new();
        let err = install_image(&mut out, &mut backend, &mut &b"image"[..], Some("0")).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("sha256 mismatch: got {}, want 0", sha_of(b"image"))
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "writing Image.b (running: Image.a) …\n"
        );
    }

    #[test]
    fn an_install_during_an_undecided_trial_is_refused() {
        // the trial is only visible once the store is mounted, like
        // tryboot.txt on the real boot partition: the check must
        // run after the mount, or it sees nothing and lets the
        // install delete the pending trial
        let mut backend = FakeBoot {
            undecided_trial: true,
            ..FakeBoot::new()
        };
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("image");
        std::fs::write(&image, b"an image").unwrap();
        let code = install(&mut backend, false, image.to_str().unwrap(), None);
        assert_eq!(code, 1);
        assert!(backend.mounted, "the trial check ran on the mounted store");
        assert!(
            backend.card.lock().unwrap().is_empty(),
            "nothing was written"
        );
    }

    #[test]
    fn the_commit_lines() {
        let mut backend = FakeBoot {
            committed: Some(Commit::Committed {
                kernel: "Image.b".into(),
            }),
            ..FakeBoot::new()
        };
        let mut out = Vec::new();
        commit(&mut out, &mut backend).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "committed: Image.b\n");

        let mut backend = FakeBoot::new();
        let mut out = Vec::new();
        commit(&mut out, &mut backend).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "no trial boot to commit\n");
    }

    #[test]
    fn the_abandon_line() {
        let mut backend = FakeBoot {
            abandoned: Some("Image.b".into()),
            ..FakeBoot::new()
        };
        let mut out = Vec::new();
        abandon(&mut out, &mut backend).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "trial of Image.b never committed, dropped\n"
        );
        // nothing pending: nothing is said
        let mut out = Vec::new();
        abandon(&mut out, &mut backend).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "");
    }

    #[test]
    fn a_normal_boot_with_a_pending_trial_drops_it() {
        // a trial that never committed (crash, hang, rollback): the
        // trial command on a normal boot says what it dropped
        let mut backend = FakeBoot {
            abandoned: Some("Image.b".into()),
            ..FakeBoot::new()
        };
        let mut out = Vec::new();
        assert_eq!(normal_boot(&mut backend, &mut out), 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "trial of Image.b never committed, dropped\n"
        );
    }

    #[test]
    fn a_normal_boot_without_a_pending_trial_says_nothing() {
        let mut backend = FakeBoot::new();
        let mut out = Vec::new();
        assert_eq!(normal_boot(&mut backend, &mut out), 0);
        assert_eq!(out, Vec::<u8>::new());
    }

    #[test]
    fn a_status_line_split_across_reads_still_counts_as_up() {
        // TCP may deliver the status line in pieces — a healthy
        // server must not count as down for it (the old wget read
        // until its answer was complete)
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            conn.write_all(b"HTTP/1.1 ").unwrap();
            conn.flush().unwrap();
            // long enough that the client's first read ends without
            // the status code
            std::thread::sleep(Duration::from_millis(300));
            conn.write_all(b"200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            conn.flush().unwrap();
            // hold the connection until the client is done
            std::thread::sleep(Duration::from_millis(300));
        });
        assert!(web_up(&addr), "the split status line is an answer");
        server.join().unwrap();
    }

    #[test]
    fn an_endpoint_that_refuses_the_connection_is_down() {
        assert!(!web_up("127.0.0.1:1"));
    }

    #[test]
    fn a_final_snapshot_request_waits_for_its_copy() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let taken = Arc::new(AtomicU32::new(0));
        // a cadence far beyond the test: only the request may copy
        let worker = SnapshotWorker::start(
            {
                let taken = taken.clone();
                Arc::new(move || {
                    taken.fetch_add(1, Ordering::SeqCst);
                })
            },
            Duration::from_secs(3600),
            Duration::from_secs(5),
        );
        // the worker's own first copy runs right at its start
        for _ in 0..1000 {
            if taken.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let before = taken.load(Ordering::SeqCst);
        assert!(before >= 1, "the worker's first copy ran");
        worker.snapshot_now();
        assert_eq!(
            taken.load(Ordering::SeqCst),
            before + 1,
            "the request's copy ran before the bounded wait returned"
        );
    }

    #[test]
    fn a_blocking_snapshot_does_not_stall_the_feeding_or_the_commit() {
        use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

        // a copy stuck for minutes (the gate), hit from both
        // callers — the worker's own cadence and the decision's
        // final request: the feed thread keeps its cadence, the
        // decision goes on after its bound, and the commit still
        // disarms with the magic 'V'
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let take = {
            let gate = gate.clone();
            move || {
                let (open, notify) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = notify.wait(open).unwrap();
                }
            }
        };
        let worker = SnapshotWorker::start(
            Arc::new(take),
            Duration::from_millis(20),
            Duration::from_millis(50),
        );

        let feeds = Arc::new(AtomicU32::new(0));
        let disarmed = Arc::new(AtomicBool::new(false));
        struct FeedWorld {
            feeds: Arc<AtomicU32>,
            disarmed: Arc<AtomicBool>,
        }
        impl GuardWorld for FeedWorld {
            fn uptime(&self) -> u64 {
                // far from every deadline: only the commit ends this
                10
            }
            fn arm(&self) {}
            fn feed(&self) {
                self.feeds.fetch_add(1, Ordering::SeqCst);
            }
            fn disarm(&self) {
                self.disarmed.store(true, Ordering::SeqCst);
            }
            fn log(&self, _line: &str) {}
            fn sleep(&self, _step: Duration) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        let clock = Arc::new(AtomicU64::new(10));
        let feeds_around_snapshot = Arc::new(AtomicU32::new(0));
        struct DecideWorld {
            clock: Arc<AtomicU64>,
            feeds: Arc<AtomicU32>,
            feeds_around_snapshot: Arc<AtomicU32>,
            worker: SnapshotWorker,
        }
        impl TrialWorld for DecideWorld {
            fn uptime(&mut self) -> u64 {
                self.clock.load(Ordering::SeqCst)
            }
            fn mestro_pid(&mut self) -> Option<u32> {
                Some(100)
            }
            fn network_up(&mut self) -> bool {
                true
            }
            fn web_up(&mut self) -> bool {
                true
            }
            fn sleep(&mut self, step: Duration) {
                self.clock.fetch_add(step.as_secs(), Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(1));
            }
            fn log(&mut self, _line: &str) {}
            fn snapshot(&mut self) {
                let before = self.feeds.load(Ordering::SeqCst);
                self.worker.snapshot_now();
                let after = self.feeds.load(Ordering::SeqCst);
                self.feeds_around_snapshot
                    .fetch_add(after - before, Ordering::SeqCst);
            }
            fn write_summary(&mut self, _result: &str) {}
            fn commit(&mut self) -> bool {
                true
            }
            fn reboot_rollback(&mut self) -> bool {
                false
            }
        }

        let guard = trial::GuardThread::new(FeedWorld {
            feeds: feeds.clone(),
            disarmed: disarmed.clone(),
        });
        let mut world = DecideWorld {
            clock,
            feeds: feeds.clone(),
            feeds_around_snapshot: feeds_around_snapshot.clone(),
            worker,
        };
        assert_eq!(
            trial::decide(&mut world, &guard),
            trial::Outcome::Committed,
            "the stuck copy holds neither the decision nor the commit"
        );
        assert!(
            disarmed.load(Ordering::SeqCst),
            "the magic 'V' closed the watchdog"
        );
        let during = feeds_around_snapshot.load(Ordering::SeqCst);
        assert!(
            during >= 3,
            "the guard fed only {during} times while the stuck copy held the request"
        );
        // let the worker finish its copy and end with the channel
        let (open, notify) = &*gate;
        *open.lock().unwrap() = true;
        notify.notify_all();
    }

    #[test]
    fn the_status_lines() {
        let mut backend = FakeBoot::new();
        let mut out = Vec::new();
        status(&mut out, &mut backend).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "active:  Image.a\nthis boot: normal\n"
        );
    }
}
