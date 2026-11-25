//! `POST /ota`: stream an uploaded system image into the inactive
//! boot slot through `mestro_update`'s boot control (in process —
//! mestro runs as root on the image, so web upload and SSH update
//! share one writer), answer, then start the trial boot through the
//! same backend.
//!
//! One actor owns the update. The async handler only does the cheap
//! checks (length, size, digest format, magic, the busy flag, the
//! cross-process lock), then hands the socket to a single blocking
//! task. That task reads the body straight from the socket into the
//! installer (never buffered), answers on the same socket and, on
//! success, reboots. Every blocking operation in it is a local file
//! operation or a socket read/write with a timeout, so a client
//! cannot hang it; the busy flag and the lock belong to it until the
//! update is over.
//!
//! The handler knows nothing about the UI: progress leaves through
//! the `notify` callback ([`OtaPhase`]), which keeps it testable
//! headless. [`Ota::detect`] builds the endpoint from whatever boot
//! chain the board has — no boot control, no OTA (the tests inject a
//! boot chain on a temp dir and a reboot that only touches a file).

use super::http;
use mestro_update::{self as update, BootControl};
use parking_lot::Mutex;
use serde_json::json;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream as StdTcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

const MIN_IMAGE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 192 * 1024 * 1024;
// the arm64 Image header: magic "ARMd" at offset 0x38
const MAGIC_OFFSET: usize = 0x38;
const MAGIC: &[u8; 4] = b"ARMd";
const MAGIC_WINDOW: usize = 64;
const DRAIN_CHUNK_BYTES: usize = 64 * 1024;
const PROGRESS_STEP_PERCENT: u64 = 2;
// a rejected client may still be sending: read it out for a moment
// so the reply is not lost to a connection reset
const REJECT_DRAIN: Duration = Duration::from_secs(2);

// one upload at a time: the installer owns a single inactive slot
static BUSY: AtomicBool = AtomicBool::new(false);

struct Busy;

impl Busy {
    fn acquire() -> Option<Self> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            // lazy: a Busy built on a lost race would drop and unlock
            .then(|| Busy)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// the update's right to run: the process-wide busy flag and the
/// cross-process lock. The blocking task owns it until the update is
/// over — on success through reply, delay and reboot, so a second
/// upload cannot remove the slot the first one just verified
struct Guard {
    _busy: Busy,
    _lock: update::UpdateLock,
}

/// what the device overlay shows
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OtaPhase {
    Receiving { done: u64, total: u64 },
    Verifying,
    Rebooting,
    Failed(String),
}

impl OtaPhase {
    pub(crate) fn status_text(&self) -> String {
        match self {
            Self::Receiving { done, total } => {
                format!("Receiving update … {} %", done * 100 / (*total).max(1))
            }
            Self::Verifying => "Verifying …".into(),
            Self::Rebooting => "Restarting …".into(),
            Self::Failed(reason) => format!("Update failed: {reason}"),
        }
    }

    pub(crate) fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// how the reboot is started
type RebootFn = Arc<dyn Fn() -> io::Result<()> + Send + Sync>;

/// the board's boot chain; only the update task that holds the
/// [`Guard`] works with it
type SharedBackend = Arc<Mutex<Box<dyn BootControl + Send>>>;

/// where progress goes: called on the blocking task
type Notify = Arc<dyn Fn(OtaPhase) + Send + Sync>;

/// the OTA endpoint: the boot control it updates through, how
/// long it waits for the client, and where the cross-process
/// update lock lives
pub(crate) struct Ota {
    backend: SharedBackend,
    reboot: RebootFn,
    lock_path: PathBuf,
    reboot_delay: Duration,
    read_timeout: Duration,
}

impl Ota {
    /// the image's OTA endpoint, when this board has a boot control
    /// the update supports — None answers unavailable
    pub(crate) fn detect() -> Option<Self> {
        let backend: SharedBackend = match update::detect() {
            update::Detection::Backend(backend) => Arc::new(Mutex::new(backend)),
            _ => return None,
        };
        let reboot = {
            let backend = backend.clone();
            Arc::new(move || backend.lock().reboot(true)) as RebootFn
        };
        Some(Self {
            backend,
            reboot,
            lock_path: PathBuf::from("/run/mestro-update.lock"),
            reboot_delay: Duration::from_secs(2),
            read_timeout: Duration::from_secs(30),
        })
    }
}

struct Failure {
    status: &'static str,
    message: String,
    /// the client's body was never fully read: the reply goes out
    /// first, then the rest is drained — unread bytes would turn
    /// the closing socket into a reset, and the client would see a
    /// write error instead of the answer
    unread_body: bool,
}

impl Failure {
    fn new(status: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            unread_body: false,
        }
    }
}

/// everything the blocking task owns for the update
struct Job {
    stream: StdTcpStream,
    /// the body bytes the handler already read (the magic window)
    first: Vec<u8>,
    total: u64,
    sha256: Option<String>,
    backend: SharedBackend,
    reboot: RebootFn,
    reboot_delay: Duration,
    notify: Notify,
    guard: Guard,
}

/// `request` is what `read_head` buffered: the head plus any body
/// bytes that came in with it. The caller has authenticated.
pub(super) async fn handle(
    mut stream: TcpStream,
    request: &[u8],
    ota: &Ota,
    notify: impl Fn(OtaPhase) + Send + Sync + 'static,
) -> Result<(), String> {
    let (total, sha256) = match check_request(request) {
        Ok(checked) => checked,
        Err(failure) => return reject(&mut stream, failure).await,
    };
    let Some(busy) = Busy::acquire() else {
        return reject(
            &mut stream,
            Failure::new("409 Conflict", "an update is already running"),
        )
        .await;
    };
    // refusals before the update starts leave the overlay alone
    let first = match read_first_bytes(&mut stream, request, total, ota.read_timeout).await {
        Ok(first) => first,
        Err(failure) => {
            // the drain in reject must not keep the next upload out
            drop(busy);
            return reject(&mut stream, failure).await;
        }
    };
    // the cross-process lock, taken only now — headers and magic
    // passed: a client that never sends its magic must not keep the
    // CLI and S99ota out of an update (never a wait, same 409)
    let lock = match update::UpdateLock::try_acquire_at(&ota.lock_path) {
        Ok(lock) => lock,
        Err(e) => {
            drop(busy);
            let failure = if e.kind() == io::ErrorKind::WouldBlock {
                Failure::new("409 Conflict", "an update is already running")
            } else {
                Failure::new(
                    "500 Internal Server Error",
                    format!("cannot lock the update: {e}"),
                )
            };
            return reject(&mut stream, failure).await;
        }
    };
    // from here the guard belongs to the blocking task — the
    // undecided-trial refusal runs there too, after the mount and
    // under this lock: an install can never start during an
    // undecided trial, so S99ota's commit cannot collide with one
    let guard = Guard {
        _busy: busy,
        _lock: lock,
    };
    let stream = stream
        .into_std()
        .and_then(|std| {
            std.set_nonblocking(false)?;
            std.set_read_timeout(Some(ota.read_timeout))?;
            std.set_write_timeout(Some(ota.read_timeout))?;
            Ok(std)
        })
        .map_err(|e| format!("cannot hand the socket over: {e}"))?;
    let notify: Notify = Arc::new(notify);
    let job = Job {
        stream,
        first,
        total,
        sha256,
        backend: ota.backend.clone(),
        reboot: ota.reboot.clone(),
        reboot_delay: ota.reboot_delay,
        notify: notify.clone(),
        guard,
    };
    match tokio::task::spawn_blocking(move || run_update(job)).await {
        Ok(result) => result,
        Err(e) => {
            // a panic: the guard and the socket went down with the task
            tracing::error!("the update task failed: {e}");
            (*notify)(OtaPhase::Failed("the update failed".into()));
            Err(format!("the update task failed: {e}"))
        }
    }
}

/// the whole update on the blocking task: stream the socket into the
/// boot control, answer, and on success start the trial boot
fn run_update(job: Job) -> Result<(), String> {
    let Job {
        mut stream,
        first,
        total,
        sha256,
        backend,
        reboot,
        reboot_delay,
        notify,
        guard,
    } = job;
    (*notify)(OtaPhase::Receiving { done: 0, total });
    let mut body = Body {
        first: io::Cursor::new(first),
        stream: &mut stream,
        remaining: total,
        total,
        notified_percent: 0,
        notify: &*notify,
        failure: None,
    };
    let outcome = {
        let mut backend = backend.lock();
        let outcome = install_from(&mut **backend, &mut body, sha256.as_deref());
        backend.release();
        outcome
    };
    let socket_failure = body.failure.take();
    let unread_body = body.remaining > 0;
    match outcome {
        Ok(installed) => {
            // held through reply, delay and reboot
            let _guard = guard;
            // the slot is ready: reboot even if the client is gone
            let reply = json!({ "sha256": installed.sha256 }).to_string();
            let _ = answer(&mut stream, "200 OK", "application/json", &reply);
            (*notify)(OtaPhase::Rebooting);
            std::thread::sleep(reboot_delay);
            (*reboot)()
                .map_err(|e| format!("reboot failed: {e}"))
                .inspect_err(|e| (*notify)(OtaPhase::Failed(e.clone())))
        }
        Err(e) => {
            // nothing was armed: the next upload may start while
            // this client is still being answered
            drop(guard);
            let failure = socket_failure.unwrap_or_else(|| install_failure(e, unread_body));
            (*notify)(OtaPhase::Failed(failure.message.clone()));
            // the reply goes out before the rest of the body is
            // drained: a refusal must not wait for a silent client
            let _ = answer(
                &mut stream,
                failure.status,
                "text/html; charset=utf-8",
                &failure.message,
            );
            if failure.unread_body {
                drain(&mut stream);
            }
            Err(format!("{}: {}", failure.status, failure.message))
        }
    }
}

/// mount, refuse an undecided trial, prepare the inactive slot and
/// stream the body into it
fn install_from(
    backend: &mut dyn BootControl,
    body: &mut Body<'_>,
    sha256: Option<&str>,
) -> Result<update::Installed, update::Error> {
    backend.mount()?;
    // this boot may be a trial whose decision is still pending —
    // visible only on the mounted store. An install would delete
    // the tryboot.txt its decision (commit or abandon) still needs
    if update::undecided_trial(backend)? {
        return Err(update::Error::UndecidedTrial);
    }
    let mut target = backend.begin_update()?;
    update::install(backend, &mut target, body, sha256)
}

/// the body as a blocking `Read` for the installer: the bytes the
/// handler already read, then the socket, up to Content-Length. A
/// socket that stalls or ends early is an error, never an EOF — a
/// short body is not a complete image. The reply that error becomes
/// is kept in `failure`
struct Body<'a> {
    first: io::Cursor<Vec<u8>>,
    stream: &'a mut StdTcpStream,
    remaining: u64,
    total: u64,
    notified_percent: u64,
    notify: &'a dyn Fn(OtaPhase),
    failure: Option<Failure>,
}

impl Body<'_> {
    fn read_socket(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = loop {
            match self.stream.read(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                other => break other,
            }
        };
        match read {
            Ok(0) => {
                self.failure = Some(Failure::new("400 Bad Request", "upload interrupted"));
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "upload ended early",
                ))
            }
            Ok(n) => Ok(n),
            // the socket's read timeout
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                self.failure = Some(Failure::new("408 Request Timeout", "upload stalled"));
                Err(e)
            }
            Err(e) => {
                self.failure = Some(Failure::new(
                    "400 Bad Request",
                    format!("upload interrupted: {e}"),
                ));
                Err(e)
            }
        }
    }
}

impl Read for Body<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let want = buf
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        if want == 0 {
            return Ok(0);
        }
        let buf = &mut buf[..want];
        let mut n = Read::read(&mut self.first, buf)?;
        if n == 0 {
            n = self.read_socket(buf)?;
        }
        self.remaining -= n as u64;
        let received = self.total - self.remaining;
        let percent = received * 100 / self.total;
        if percent >= self.notified_percent + PROGRESS_STEP_PERCENT {
            self.notified_percent = percent;
            (self.notify)(OtaPhase::Receiving {
                done: received,
                total: self.total,
            });
        }
        if self.remaining == 0 {
            // the installer re-reads the written slot off the card
            // and hashes it now
            (self.notify)(OtaPhase::Verifying);
        }
        Ok(n)
    }
}

/// the reply an installer failure becomes: the trial refusal is a
/// refusal, everything else is the server's own trouble
fn install_failure(e: update::Error, unread_body: bool) -> Failure {
    let (status, message) = match e {
        update::Error::UndecidedTrial => (
            "409 Conflict",
            "this boot is an undecided trial — commit or abandon it first".to_string(),
        ),
        e => ("500 Internal Server Error", e.to_string()),
    };
    Failure {
        status,
        message,
        unread_body,
    }
}

/// the reply on the blocking socket; its write half is done after it
fn answer(
    stream: &mut StdTcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    let reply = http::render(
        status,
        content_type,
        "Cache-Control: no-store",
        &[],
        body.as_bytes(),
    );
    stream.write_all(&reply)?;
    stream.shutdown(Shutdown::Write)
}

/// read whatever the client still owes after a refusal, for a
/// moment: unread bytes would turn the closing socket into a reset,
/// and the client would see a write error instead of the refusal
fn drain(stream: &mut StdTcpStream) {
    let deadline = Instant::now() + REJECT_DRAIN;
    let mut sink = vec![0u8; DRAIN_CHUNK_BYTES];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if stream.set_read_timeout(Some(left)).is_err() {
            return;
        }
        if !matches!(stream.read(&mut sink), Ok(n) if n > 0) {
            return;
        }
    }
}

/// the body length and the optional expected digest, or the refusal
fn check_request(request: &[u8]) -> Result<(u64, Option<String>), Failure> {
    let total = http::content_length(request)
        .ok_or_else(|| Failure::new("411 Length Required", "Content-Length required"))?;
    if !(MIN_IMAGE_BYTES..=MAX_IMAGE_BYTES).contains(&total) {
        return Err(Failure::new(
            "413 Payload Too Large",
            "not a mestro system image (size)",
        ));
    }
    let sha256 = match http::header(request, "x-sha256") {
        None => None,
        Some(digest) if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) => {
            // the installer compares against lowercase sha256sum output
            Some(digest.to_ascii_lowercase())
        }
        Some(_) => {
            return Err(Failure::new(
                "400 Bad Request",
                "X-Sha256 must be 64 hex digits",
            ))
        }
    };
    Ok((total, sha256))
}

/// refuse before the update starts: reply, then read out what the
/// client still sends (the same drain the update task does)
async fn reject(stream: &mut TcpStream, failure: Failure) -> Result<(), String> {
    let _ = http::reply(stream, failure.status, &failure.message).await;
    let mut sink = vec![0u8; DRAIN_CHUNK_BYTES];
    let _ = tokio::time::timeout(REJECT_DRAIN, async {
        while matches!(stream.read(&mut sink).await, Ok(n) if n > 0) {}
    })
    .await;
    Err(format!("{}: {}", failure.status, failure.message))
}

/// the first body bytes (those `read_head` already pulled in plus
/// whatever completes the magic window), checked for the arm64
/// Image magic
async fn read_first_bytes(
    stream: &mut TcpStream,
    request: &[u8],
    total: u64,
    timeout: Duration,
) -> Result<Vec<u8>, Failure> {
    let mut buf = request[http::split_head(request)..].to_vec();
    buf.truncate(total as usize);
    while buf.len() < MAGIC_WINDOW {
        let mut more = [0u8; 4096];
        let want = more.len().min(total as usize - buf.len());
        let n = match tokio::time::timeout(timeout, stream.read(&mut more[..want])).await {
            Err(_) => return Err(Failure::new("408 Request Timeout", "upload stalled")),
            Ok(Err(e)) => {
                return Err(Failure::new(
                    "400 Bad Request",
                    format!("upload interrupted: {e}"),
                ))
            }
            Ok(Ok(0)) => return Err(Failure::new("400 Bad Request", "upload interrupted")),
            Ok(Ok(n)) => n,
        };
        buf.extend_from_slice(&more[..n]);
    }
    if &buf[MAGIC_OFFSET..MAGIC_OFFSET + MAGIC.len()] != MAGIC {
        return Err(Failure::new("400 Bad Request", "not a mestro system image"));
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mestro_update::{Commit, UpdateTarget};
    use std::future::Future;
    use std::path::Path;
    use tokio::io::AsyncWriteExt as _;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    const MIB: usize = 1 << 20;

    /// what a test learns from its boot chain
    #[derive(Default)]
    struct Probe {
        card: Vec<u8>,
        mounted: bool,
        begin_update_calls: usize,
        armed: Vec<String>,
        discarded: Vec<String>,
        rebooted: bool,
    }

    /// the ways a boot chain can misbehave
    #[derive(Default, Clone)]
    struct Quirks {
        /// this boot is a trial whose tryboot.txt is only visible on
        /// the mounted store — like the real boot partition
        undecided_trial_behind_mount: bool,
        /// the read-back cannot drop the page cache
        failing_cache_drop: bool,
        /// the boot chain dies while preparing the slot
        panics_at_begin: bool,
        /// letting go of the store never finishes — the real backend
        /// bounds its external sync/umount, and so must this fake
        stall_in_release: Option<stall::Wait>,
    }

    /// the release stall of [`Quirks`]: a gate the update thread
    /// waits on until the test opens it
    mod stall {
        use std::sync::{Arc, Condvar, Mutex};

        /// a gate the release thread holds shut until the test opens
        /// it — the stand-in for the real backend's bounded sync and
        /// umount, which return after their budgets even when the
        /// command never exits
        #[derive(Clone, Default)]
        pub(super) struct Gate {
            state: Arc<(Mutex<bool>, Condvar)>,
        }

        impl Gate {
            pub(super) fn open(&self) {
                let (open, notify) = &*self.state;
                *open.lock().unwrap() = true;
                notify.notify_all();
            }
        }

        /// the gate as the boot chain sees it
        #[derive(Clone, Default)]
        pub(super) struct Wait(Gate);

        pub(super) fn gate() -> (Gate, Wait) {
            let gate = Gate::default();
            (gate.clone(), Wait(gate))
        }

        impl Wait {
            /// block until the test opens the gate
            pub(super) fn hold(&self) {
                let (open, notify) = &*self.0.state;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = notify.wait(open).unwrap();
                }
            }
        }
    }

    /// a boot chain that keeps its "card" in memory
    struct FakeBoot {
        probe: Arc<Mutex<Probe>>,
        quirks: Quirks,
    }

    /// the store end of [`FakeBoot`]: the probe's card
    struct CardSink(Arc<Mutex<Probe>>);

    impl Write for CardSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().card.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl BootControl for FakeBoot {
        fn mount(&mut self) -> Result<(), update::Error> {
            self.probe.lock().mounted = true;
            Ok(())
        }
        fn release(&mut self) {
            if let Some(wait) = &self.quirks.stall_in_release {
                wait.hold();
            }
        }
        fn active(&self) -> io::Result<String> {
            Ok("slot-a".into())
        }
        fn pending(&self) -> io::Result<Option<String>> {
            let visible = self.quirks.undecided_trial_behind_mount && self.probe.lock().mounted;
            Ok(visible.then(|| "slot-b".into()))
        }
        fn trial_boot(&self) -> bool {
            self.quirks.undecided_trial_behind_mount
        }
        fn begin_update(&mut self) -> Result<UpdateTarget, update::Error> {
            assert!(!self.quirks.panics_at_begin, "the boot chain died");
            let mut probe = self.probe.lock();
            probe.begin_update_calls += 1;
            probe.card.clear();
            Ok(UpdateTarget {
                slot: "slot-b".into(),
                running: "slot-a".into(),
                sink: Box::new(CardSink(self.probe.clone())),
            })
        }
        fn read_back(&mut self, _target: &UpdateTarget) -> io::Result<String> {
            if self.quirks.failing_cache_drop {
                return Err(io::Error::other("cannot drop the page cache"));
            }
            Ok(sha_of(&self.probe.lock().card))
        }
        fn discard(&mut self, target: &UpdateTarget) {
            let mut probe = self.probe.lock();
            probe.card.clear();
            probe.discarded.push(target.slot.clone());
        }
        fn arm_trial(&mut self, target: &UpdateTarget) -> io::Result<()> {
            self.probe.lock().armed.push(target.slot.clone());
            Ok(())
        }
        fn commit(&mut self) -> Result<Commit, update::Error> {
            Ok(Commit::NoTrial)
        }
        fn abandon(&mut self) -> io::Result<Option<String>> {
            Ok(None)
        }
        fn reboot(&mut self, _into_trial: bool) -> io::Result<()> {
            Ok(())
        }
    }

    /// the endpoint around a [`FakeBoot`]; the lock lives in a temp
    /// dir, the reboot only records itself
    struct Rig {
        dir: tempfile::TempDir,
        probe: Arc<Mutex<Probe>>,
        ota: Arc<Ota>,
    }

    impl Rig {
        fn new() -> Self {
            Self::with(Quirks::default())
        }

        fn with(quirks: Quirks) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let probe = Arc::new(Mutex::new(Probe::default()));
            let backend: SharedBackend = Arc::new(Mutex::new(Box::new(FakeBoot {
                probe: probe.clone(),
                quirks,
            })));
            let rebooted = probe.clone();
            let ota = Arc::new(Ota {
                backend,
                reboot: Arc::new(move || {
                    rebooted.lock().rebooted = true;
                    Ok(())
                }),
                lock_path: dir.path().join("mestro-update.lock"),
                reboot_delay: Duration::from_millis(10),
                read_timeout: Duration::from_secs(30),
            });
            Self { dir, probe, ota }
        }

        fn lock_path(&self) -> PathBuf {
            self.dir.path().join("mestro-update.lock")
        }

        fn read_timeout(&mut self, timeout: Duration) {
            Arc::get_mut(&mut self.ota).unwrap().read_timeout = timeout;
        }

        fn reboot_delay(&mut self, delay: Duration) {
            Arc::get_mut(&mut self.ota).unwrap().reboot_delay = delay;
        }

        fn stalled_release() -> (Self, stall::Gate) {
            // the store takes a long time letting go — like the real
            // backend when its sync hangs until its budget runs out.
            // The gate stays shut until the test opens it; the
            // reply can only come once the store let go
            let (gate, wait) = stall::gate();
            (
                Self::with(Quirks {
                    stall_in_release: Some(wait),
                    ..Quirks::default()
                }),
                gate,
            )
        }
    }

    // the busy flag is process-global: tests take turns
    static SERIAL: Mutex<()> = Mutex::new(());

    type Phases = Arc<Mutex<Vec<OtaPhase>>>;

    struct Server {
        addr: std::net::SocketAddr,
        done: mpsc::UnboundedReceiver<Result<(), String>>,
        phases: Phases,
    }

    // accepts connections and runs the handler on each, like `serve`
    async fn start(ota: Arc<Ota>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, done) = mpsc::unbounded_channel();
        let phases: Phases = Arc::default();
        let seen = phases.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (ota, tx, seen) = (ota.clone(), tx.clone(), seen.clone());
                tokio::spawn(async move {
                    let request = http::read_head(&mut stream).await.unwrap();
                    let result = handle(stream, &request, &ota, move |p| seen.lock().push(p)).await;
                    let _ = tx.send(result);
                });
            }
        });
        Server { addr, done, phases }
    }

    fn run<F: Future>(test: F) -> F::Output {
        let _turn = SERIAL.lock();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(test)
    }

    fn image(len: usize) -> Vec<u8> {
        let mut body: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        body[0x38..0x3c].copy_from_slice(b"ARMd");
        body
    }

    fn sha_of(body: &[u8]) -> String {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(body);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn request_head(content_length: Option<usize>, extra: &str) -> String {
        let length = content_length
            .map(|n| format!("Content-Length: {n}\r\n"))
            .unwrap_or_default();
        format!("POST /ota?pair=X HTTP/1.1\r\nHost: kiosk\r\n{length}{extra}\r\n")
    }

    async fn connect(addr: std::net::SocketAddr, head: &str, body: &[u8]) -> TcpStream {
        let mut client = TcpStream::connect(addr).await.unwrap();
        // one write: the head and the first body bytes arrive together
        let mut first = head.as_bytes().to_vec();
        first.extend_from_slice(body);
        client.write_all(&first).await.unwrap();
        client
    }

    async fn read_all(client: &mut (impl tokio::io::AsyncRead + Unpin)) -> String {
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut raw))
            .await
            .expect("response timeout")
            .unwrap();
        String::from_utf8_lossy(&raw).to_string()
    }

    // (status code, body) once the server closed the connection
    async fn response(client: &mut (impl tokio::io::AsyncRead + Unpin)) -> (u16, String) {
        let text = read_all(client).await;
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    async fn wait_until(satisfied: impl Fn() -> bool) {
        for _ in 0..1000 {
            if satisfied() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the condition was never met");
    }

    fn lock_is_free(path: &Path) -> bool {
        update::UpdateLock::try_acquire_at(path).is_ok()
    }

    #[test]
    fn upload_lands_in_the_slot_arms_the_trial_and_reboots_after_the_reply() {
        run(async {
            let mut rig = Rig::new();
            rig.reboot_delay(Duration::from_millis(300));
            let server = start(rig.ota.clone()).await;
            let body = image(5 * MIB);
            let sha = sha_of(&body);
            let head = request_head(
                Some(body.len()),
                &format!("X-Sha256: {}\r\n", sha.to_uppercase()),
            );
            let mut client = connect(server.addr, &head, &body).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 200);
            assert!(reply.contains(&sha), "{reply}");
            {
                let probe = rig.probe.lock();
                assert!(probe.card == body, "the slot differs from the upload");
                assert_eq!(probe.armed, ["slot-b"]);
                assert!(probe.discarded.is_empty());
                // the reply is out before the reboot
                assert!(!probe.rebooted, "rebooted before the delay ran out");
            }
            wait_until(|| rig.probe.lock().rebooted).await;
            let phases = server.phases.lock().clone();
            assert!(matches!(
                phases.first(),
                Some(OtaPhase::Receiving { done: 0, .. })
            ));
            assert!(phases.contains(&OtaPhase::Verifying));
            assert_eq!(phases.last(), Some(&OtaPhase::Rebooting));
        });
    }

    #[test]
    fn wrong_magic_is_rejected_before_anything_starts() {
        run(async {
            let rig = Rig::new();
            let mut server = start(rig.ota.clone()).await;
            let head = request_head(Some(5 * MIB), "");
            let mut client = connect(server.addr, &head, &vec![0u8; 4096]).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 400);
            assert!(reply.contains("not a mestro system image"), "{reply}");
            drop(client);
            assert!(server.done.recv().await.unwrap().is_err());
            assert!(!rig.probe.lock().mounted, "the update ran");
            assert!(
                server.phases.lock().is_empty(),
                "a rejected request must not touch the overlay"
            );
            assert!(!BUSY.load(Ordering::Acquire));
            assert!(lock_is_free(&rig.lock_path()));
        });
    }

    #[test]
    fn missing_content_length_is_411() {
        run(async {
            let rig = Rig::new();
            let server = start(rig.ota.clone()).await;
            let mut client = connect(server.addr, &request_head(None, ""), b"").await;
            assert_eq!(response(&mut client).await.0, 411);
            assert!(!rig.probe.lock().mounted);
        });
    }

    #[test]
    fn body_size_outside_the_limits_is_413() {
        run(async {
            let rig = Rig::new();
            let server = start(rig.ota.clone()).await;
            for len in [MIB, 193 * MIB] {
                let mut client =
                    connect(server.addr, &request_head(Some(len), ""), &image(4096)).await;
                assert_eq!(response(&mut client).await.0, 413, "{len} bytes");
            }
            assert!(!rig.probe.lock().mounted);
        });
    }

    #[test]
    fn malformed_sha256_header_is_400() {
        run(async {
            let rig = Rig::new();
            let server = start(rig.ota.clone()).await;
            let head = request_head(Some(5 * MIB), "X-Sha256: not-a-digest\r\n");
            let mut client = connect(server.addr, &head, &image(4096)).await;
            assert_eq!(response(&mut client).await.0, 400);
            assert!(!rig.probe.lock().mounted);
        });
    }

    #[test]
    fn a_sha_mismatch_is_a_500_with_the_installer_message_and_no_reboot() {
        run(async {
            let rig = Rig::new();
            let mut server = start(rig.ota.clone()).await;
            let body = image(5 * MIB);
            let wrong = "0".repeat(64);
            let head = request_head(Some(body.len()), &format!("X-Sha256: {wrong}\r\n"));
            let mut client = connect(server.addr, &head, &body).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 500);
            assert!(reply.contains("sha256 mismatch"), "{reply}");
            assert!(server.done.recv().await.unwrap().is_err());
            {
                let probe = rig.probe.lock();
                assert_eq!(
                    probe.discarded,
                    ["slot-b"],
                    "the mismatched slot is removed"
                );
                assert!(probe.armed.is_empty());
                assert!(!probe.rebooted);
            }
            let phases = server.phases.lock().clone();
            assert!(
                matches!(phases.last(), Some(OtaPhase::Failed(m)) if m.contains("sha256 mismatch"))
            );
        });
    }

    #[test]
    fn a_failed_cache_drop_is_a_500_and_arms_no_trial() {
        run(async {
            let rig = Rig::with(Quirks {
                failing_cache_drop: true,
                ..Quirks::default()
            });
            let mut server = start(rig.ota.clone()).await;
            let body = image(5 * MIB);
            // no sha header: only the read-back can catch this
            let mut client = connect(server.addr, &request_head(Some(body.len()), ""), &body).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 500);
            assert!(reply.contains("cannot drop the page cache"), "{reply}");
            assert!(server.done.recv().await.unwrap().is_err());
            let probe = rig.probe.lock();
            assert_eq!(probe.discarded, ["slot-b"]);
            assert!(probe.armed.is_empty(), "no trial of an unverified write");
            assert!(!probe.rebooted);
        });
    }

    #[test]
    fn a_stalled_upload_is_a_408_discards_the_slot_and_arms_no_trial() {
        run(async {
            let mut rig = Rig::new();
            rig.read_timeout(Duration::from_millis(300));
            let mut server = start(rig.ota.clone()).await;
            // no sha header: a truncated upload has nothing to fail
            // its way out of
            let head = request_head(Some(8 * MIB), "");
            let mut client = connect(server.addr, &head, &image(MIB)).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 408);
            assert_eq!(reply, "upload stalled");
            drop(client);
            assert!(server
                .done
                .recv()
                .await
                .unwrap()
                .unwrap_err()
                .contains("stalled"));
            {
                let probe = rig.probe.lock();
                assert_eq!(probe.discarded, ["slot-b"]);
                assert!(probe.armed.is_empty(), "no trial of a stalled upload");
                assert!(!probe.rebooted);
            }
            assert!(!BUSY.load(Ordering::Acquire));
            assert!(lock_is_free(&rig.lock_path()));
        });
    }

    #[test]
    fn a_client_that_disconnects_mid_body_arms_no_trial_and_reboots_nothing() {
        run(async {
            let rig = Rig::new();
            let mut server = start(rig.ota.clone()).await;
            let head = request_head(Some(8 * MIB), "");
            let client = connect(server.addr, &head, &image(5 * MIB)).await;
            wait_until(|| rig.probe.lock().card.len() >= MIB).await;
            drop(client);
            let result = server.done.recv().await.unwrap();
            assert!(result.unwrap_err().contains("upload interrupted"));
            let probe = rig.probe.lock();
            assert_eq!(probe.discarded, ["slot-b"], "the half-written slot is gone");
            assert!(probe.armed.is_empty(), "no trial of a half-written slot");
            assert!(!probe.rebooted);
            assert!(matches!(
                server.phases.lock().last(),
                Some(OtaPhase::Failed(_))
            ));
        });
    }

    #[test]
    fn client_gone_after_the_body_still_reboots() {
        run(async {
            let rig = Rig::new();
            let mut server = start(rig.ota.clone()).await;
            let body = image(5 * MIB);
            let client = connect(server.addr, &request_head(Some(body.len()), ""), &body).await;
            drop(client);
            assert_eq!(server.done.recv().await.unwrap(), Ok(()));
            let probe = rig.probe.lock();
            assert_eq!(probe.armed, ["slot-b"]);
            assert!(probe.rebooted);
            assert_eq!(server.phases.lock().last(), Some(&OtaPhase::Rebooting));
        });
    }

    #[test]
    fn a_release_that_hangs_does_not_take_the_reply_with_it() {
        run(async {
            // the store hangs in its release — like the real backend
            // when its sync never exits — and lets go once its budget
            // would have run out. The reply waits for the store (the
            // task owns it until the update is over), then still
            // arrives — with the trial armed and the reboot after it
            let (rig, gate) = Rig::stalled_release();
            let mut server = start(rig.ota.clone()).await;
            let body = image(5 * MIB);
            let mut client = connect(server.addr, &request_head(Some(body.len()), ""), &body).await;
            // the store lets go after a moment — the stand-in for the
            // bounded sync/umount returning after their budgets
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                gate.open();
            });
            let (status, reply) =
                tokio::time::timeout(Duration::from_secs(10), response(&mut client))
                    .await
                    .expect("the reply arrives once the store let go");
            assert_eq!(status, 200, "{reply}");
            assert_eq!(server.done.recv().await.unwrap(), Ok(()));
            wait_until(|| rig.probe.lock().rebooted).await;
            assert_eq!(rig.probe.lock().armed, ["slot-b"], "the trial stays armed");
        });
    }

    #[test]
    fn second_upload_is_409_while_one_runs() {
        run(async {
            let rig = Rig::new();
            let mut server = start(rig.ota.clone()).await;
            let body = image(5 * MIB);
            let head = request_head(Some(body.len()), "");
            let mut first = connect(server.addr, &head, &body[..MIB]).await;
            wait_until(|| rig.probe.lock().card.len() >= MIB).await;

            // a refused upload must not release the running one's guard
            for _ in 0..2 {
                let mut other = connect(server.addr, &head, &body[..4096]).await;
                assert_eq!(response(&mut other).await.0, 409);
                drop(other);
                assert!(server.done.recv().await.unwrap().is_err());
            }

            first.write_all(&body[MIB..]).await.unwrap();
            assert_eq!(response(&mut first).await.0, 200);
            assert_eq!(server.done.recv().await.unwrap(), Ok(()));
        });
    }

    #[test]
    fn a_held_cross_process_lock_is_409_and_nothing_starts() {
        run(async {
            let rig = Rig::new();
            let mut server = start(rig.ota.clone()).await;
            // the CLI or S99ota is mid-update
            let other_process = update::UpdateLock::try_acquire_at(rig.lock_path()).unwrap();
            let head = request_head(Some(5 * MIB), "");
            let mut client = connect(server.addr, &head, &image(4096)).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 409);
            assert_eq!(reply, "an update is already running");
            drop(client);
            assert!(server.done.recv().await.unwrap().is_err());
            assert!(!rig.probe.lock().mounted, "nothing started");
            assert!(!BUSY.load(Ordering::Acquire), "the busy flag is not kept");
            drop(other_process);
            assert!(lock_is_free(&rig.lock_path()));
        });
    }

    #[test]
    fn the_cross_process_lock_stays_free_until_the_magic_arrives() {
        run(async {
            let mut rig = Rig::new();
            rig.read_timeout(Duration::from_millis(300));
            let mut server = start(rig.ota.clone()).await;
            // an upload that never completes its magic window:
            // headers are fine, the socket goes quiet
            let head = request_head(Some(5 * MIB), "");
            let client = connect(server.addr, &head, &[0u8; 32]).await;
            // the CLI or S99ota can still take the lock while this
            // upload idles — it never made it past the magic check
            assert!(lock_is_free(&rig.lock_path()), "held by the idle upload");
            drop(client);
            assert!(server.done.recv().await.unwrap().is_err());
            assert!(!rig.probe.lock().mounted, "nothing was started");
        });
    }

    #[test]
    fn the_door_stays_shut_until_the_trial_reboot() {
        run(async {
            let mut rig = Rig::new();
            rig.reboot_delay(Duration::from_millis(500));
            let mut server = start(rig.ota.clone()).await;
            let body = image(5 * MIB);
            let head = request_head(Some(body.len()), "");
            let mut first = connect(server.addr, &head, &body).await;
            assert_eq!(response(&mut first).await.0, 200);
            // the reply is out, the reboot waits: a second upload
            // must not remove what the first one verified
            let mut second = connect(server.addr, &head, &image(4096)).await;
            assert_eq!(response(&mut second).await.0, 409);
            // the refused upload and the rebooting one both report:
            // one failed, one succeeded — in whichever order
            let mut outcomes = [
                server.done.recv().await.unwrap(),
                server.done.recv().await.unwrap(),
            ];
            outcomes.sort_by_key(|outcome| outcome.is_ok());
            assert_eq!(
                outcomes,
                [
                    Err("409 Conflict: an update is already running".to_string()),
                    Ok(())
                ]
            );
            let probe = rig.probe.lock();
            assert_eq!(probe.begin_update_calls, 1, "the second upload never began");
            assert_eq!(probe.armed, ["slot-b"], "the armed slot is untouched");
            assert!(probe.discarded.is_empty());
            assert!(probe.card == body);
            // the trial boot ran, the door opens with it
            assert!(probe.rebooted);
            drop(probe);
            assert!(!BUSY.load(Ordering::Acquire));
        });
    }

    #[test]
    fn an_install_during_an_undecided_trial_is_a_409_even_to_a_silent_client() {
        run(async {
            // the pending trial sits on a store that is only
            // reachable once mounted, like tryboot.txt on the real
            // boot partition — a check before the mount would see
            // nothing and let the install delete it. The client sends
            // its headers and exactly the magic, then goes silent:
            // the refusal must not wait for the socket budget
            let rig = Rig::with(Quirks {
                undecided_trial_behind_mount: true,
                ..Quirks::default()
            });
            let mut server = start(rig.ota.clone()).await;
            let head = request_head(Some(5 * MIB), "");
            let mut client = connect(server.addr, &head, &image(64)).await;
            let (status, reply) =
                tokio::time::timeout(Duration::from_secs(1), response(&mut client))
                    .await
                    .expect("the refusal answers without the client");
            assert_eq!(status, 409);
            assert!(reply.contains("undecided trial"), "{reply}");
            // the update is over: the busy flag and the
            // cross-process lock are free right now, not after the
            // drain of the silent client
            assert!(!BUSY.load(Ordering::Acquire));
            assert!(lock_is_free(&rig.lock_path()), "the lock is free");
            {
                let probe = rig.probe.lock();
                assert!(probe.mounted, "the store was mounted for the check");
                assert_eq!(probe.begin_update_calls, 0, "no slot was opened");
                assert!(probe.card.is_empty(), "nothing was written");
                assert!(!probe.rebooted);
            }
            drop(client);
            assert!(server.done.recv().await.unwrap().is_err());
        });
    }

    #[test]
    fn a_panicking_boot_chain_gives_back_the_busy_flag_and_the_lock() {
        run(async {
            let rig = Rig::with(Quirks {
                panics_at_begin: true,
                ..Quirks::default()
            });
            let mut server = start(rig.ota.clone()).await;
            let head = request_head(Some(5 * MIB), "");
            let mut client = connect(server.addr, &head, &image(64)).await;
            // the task's socket went down with it: no reply
            assert_eq!(read_all(&mut client).await, "");
            let result = server.done.recv().await.unwrap();
            assert!(result.unwrap_err().contains("the update task failed"));
            assert!(!BUSY.load(Ordering::Acquire), "the busy flag is kept");
            assert!(lock_is_free(&rig.lock_path()), "the lock is kept");
            assert!(matches!(
                server.phases.lock().last(),
                Some(OtaPhase::Failed(_))
            ));
        });
    }

    #[test]
    fn phases_read_as_overlay_text() {
        assert_eq!(
            OtaPhase::Receiving {
                done: 50,
                total: 200
            }
            .status_text(),
            "Receiving update … 25 %"
        );
        assert_eq!(OtaPhase::Verifying.status_text(), "Verifying …");
        assert_eq!(OtaPhase::Rebooting.status_text(), "Restarting …");
        assert_eq!(
            OtaPhase::Failed("disk full".into()).status_text(),
            "Update failed: disk full"
        );
    }
}
