//! `POST /ota`: stream an uploaded system image into
//! `mestro-upgrade -`, answer, then start the trial boot. The body
//! is never buffered — it flows from the socket into the
//! installer's stdin chunk by chunk. mestro never writes /boot
//! itself; the installer stays the only writer.
//!
//! The handler knows nothing about the UI: progress leaves through
//! the `notify` callback ([`OtaPhase`]), which keeps it testable
//! headless.

use super::http;
use serde_json::json;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;

const MIN_IMAGE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 192 * 1024 * 1024;
// the arm64 Image header: magic "ARMd" at offset 0x38
const MAGIC_OFFSET: usize = 0x38;
const MAGIC: &[u8; 4] = b"ARMd";
const MAGIC_WINDOW: usize = 64;
const CHUNK_BYTES: usize = 64 * 1024;
const PROGRESS_STEP_PERCENT: u64 = 2;
// the installer re-reads the written slot after the last byte
const FINISH_TIMEOUT: Duration = Duration::from_secs(300);
// a rejected client may still be sending: read it out for a moment
// so the reply is not lost to a connection reset
const REJECT_DRAIN: Duration = Duration::from_secs(2);
// the installer's tail is all the overlay and the reply need
const OUTPUT_KEEP: usize = 16 * 1024;

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

/// the installer and the reboot command (argv, program first)
pub(crate) struct OtaCmds {
    pub upgrade: Vec<String>,
    pub reboot: Vec<String>,
    pub reboot_delay: Duration,
    pub read_timeout: Duration,
}

impl OtaCmds {
    /// env overrides are for desktop testing with stub scripts
    pub(crate) fn from_env() -> Self {
        Self {
            upgrade: command_from_env("MESTRO_UPGRADE_CMD", "/usr/sbin/mestro-upgrade"),
            reboot: command_from_env("MESTRO_REBOOT_CMD", "/usr/sbin/mestro-reboot --tryboot"),
            // the reply must be out before the board goes down
            reboot_delay: Duration::from_secs(2),
            read_timeout: Duration::from_secs(30),
        }
    }
}

fn command_from_env(var: &str, default: &str) -> Vec<String> {
    std::env::var(var)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
        .split_whitespace()
        .map(String::from)
        .collect()
}

struct Failure {
    status: &'static str,
    message: String,
}

impl Failure {
    fn new(status: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// `request` is what `read_head` buffered: the head plus any body
/// bytes that came in with it. The caller has authenticated.
pub(super) async fn handle(
    stream: &mut TcpStream,
    request: &[u8],
    cmds: &OtaCmds,
    notify: impl Fn(OtaPhase),
) -> Result<(), String> {
    let (total, sha256) = match check_request(request) {
        Ok(checked) => checked,
        Err(failure) => return reject(stream, failure).await,
    };
    let Some(busy) = Busy::acquire() else {
        return reject(
            stream,
            Failure::new("409 Conflict", "an update is already running"),
        )
        .await;
    };
    // refusals before the installer starts leave the overlay alone
    let first = match read_first_bytes(stream, request, total, cmds.read_timeout).await {
        Ok(first) => first,
        Err(failure) => {
            // the drain in reject must not keep the next upload out
            drop(busy);
            return reject(stream, failure).await;
        }
    };
    match install(stream, first, cmds, total, sha256, &notify).await {
        Ok(written_sha256) => {
            // the slot is ready: reboot even if the client is gone
            let body = json!({ "sha256": written_sha256 }).to_string();
            let _ = http::reply_json(stream, "200 OK", &body).await;
            notify(OtaPhase::Rebooting);
            tokio::time::sleep(cmds.reboot_delay).await;
            reboot(cmds)
                .await
                .inspect_err(|e| notify(OtaPhase::Failed(e.clone())))
        }
        Err(failure) => {
            notify(OtaPhase::Failed(last_line(&failure.message).to_string()));
            let _ = http::reply(stream, failure.status, &failure.message).await;
            Err(format!("{}: {}", failure.status, failure.message))
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

async fn reject(stream: &mut TcpStream, failure: Failure) -> Result<(), String> {
    let _ = http::reply(stream, failure.status, &failure.message).await;
    let mut sink = vec![0u8; CHUNK_BYTES];
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
        let n = read_some(stream, &mut more[..want], timeout).await?;
        buf.extend_from_slice(&more[..n]);
    }
    if &buf[MAGIC_OFFSET..MAGIC_OFFSET + MAGIC.len()] != MAGIC {
        return Err(Failure::new("400 Bad Request", "not a mestro system image"));
    }
    Ok(buf)
}

/// spawn, stream, wait: the installer's sha256 on success
async fn install(
    stream: &mut TcpStream,
    buf: Vec<u8>,
    cmds: &OtaCmds,
    total: u64,
    sha256: Option<String>,
    notify: &impl Fn(OtaPhase),
) -> Result<String, Failure> {
    notify(OtaPhase::Receiving { done: 0, total });
    let mut installer = Command::new(&cmds.upgrade[0]);
    installer
        .args(&cmds.upgrade[1..])
        .arg("-")
        .args(&sha256)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = installer.spawn().map_err(|e| {
        Failure::new(
            "500 Internal Server Error",
            format!("cannot start the installer: {e}"),
        )
    })?;
    #[expect(clippy::expect_used, reason = "Stdio::piped was just set above")]
    let mut stdin = child.stdin.take().expect("stdin is piped");
    // drained while the body streams: a chatty installer must not
    // block on a full pipe while we block on its stdin
    #[expect(clippy::expect_used, reason = "Stdio::piped was just set above")]
    let stdout = tokio::spawn(collect(child.stdout.take().expect("stdout is piped")));
    #[expect(clippy::expect_used, reason = "Stdio::piped was just set above")]
    let stderr = tokio::spawn(collect(child.stderr.take().expect("stderr is piped")));

    let complete = match pipe_body(stream, &mut stdin, buf, total, cmds.read_timeout, notify).await
    {
        Ok(complete) => complete,
        Err(failure) => {
            // a partly written slot is harmless: tryboot.txt only
            // appears after a verified write
            let _ = child.kill().await;
            return Err(failure);
        }
    };
    drop(stdin);

    notify(OtaPhase::Verifying);
    let status = match tokio::time::timeout(FINISH_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            return Err(Failure::new(
                "500 Internal Server Error",
                format!("installer: {e}"),
            ))
        }
        Err(_) => {
            let _ = child.kill().await;
            return Err(Failure::new(
                "500 Internal Server Error",
                "the installer did not finish in time",
            ));
        }
    };
    let stdout = stdout.await.unwrap_or_default();
    let stderr = stderr.await.unwrap_or_default();
    if !status.success() || !complete {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("installer exited with {status}")
        } else {
            stderr
        };
        return Err(Failure::new("500 Internal Server Error", message));
    }
    Ok(written_sha256(&String::from_utf8_lossy(&stdout))
        .unwrap_or_default()
        .to_string())
}

/// reads a pipe to its end, keeping only the last OUTPUT_KEEP bytes
async fn collect(mut pipe: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n @ 1..) = pipe.read(&mut chunk).await {
        kept.extend_from_slice(&chunk[..n]);
        if kept.len() > OUTPUT_KEEP {
            kept.drain(..kept.len() - OUTPUT_KEEP);
        }
    }
    kept
}

/// socket → installer stdin. `Ok(false)`: the installer closed its
/// stdin early — its exit status says why
async fn pipe_body(
    stream: &mut TcpStream,
    stdin: &mut tokio::process::ChildStdin,
    first: Vec<u8>,
    total: u64,
    timeout: Duration,
    notify: &impl Fn(OtaPhase),
) -> Result<bool, Failure> {
    let mut filled = first.len();
    let mut buf = first;
    buf.resize(buf.len().max(CHUNK_BYTES), 0);
    let mut received = 0u64;
    let mut notified_percent = 0u64;
    loop {
        match tokio::time::timeout(timeout, stdin.write_all(&buf[..filled])).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Ok(false),
            Err(_) => {
                return Err(Failure::new(
                    "500 Internal Server Error",
                    "the installer stopped consuming the image",
                ))
            }
        }
        received += filled as u64;
        let percent = received * 100 / total;
        if percent >= notified_percent + PROGRESS_STEP_PERCENT {
            notified_percent = percent;
            notify(OtaPhase::Receiving {
                done: received,
                total,
            });
        }
        if received >= total {
            return Ok(true);
        }
        let want = buf.len().min((total - received) as usize);
        filled = read_some(stream, &mut buf[..want], timeout).await?;
    }
}

async fn read_some(
    stream: &mut TcpStream,
    buf: &mut [u8],
    timeout: Duration,
) -> Result<usize, Failure> {
    match tokio::time::timeout(timeout, stream.read(buf)).await {
        Err(_) => Err(Failure::new("408 Request Timeout", "upload stalled")),
        Ok(Err(e)) => Err(Failure::new(
            "400 Bad Request",
            format!("upload interrupted: {e}"),
        )),
        Ok(Ok(0)) => Err(Failure::new("400 Bad Request", "upload interrupted")),
        Ok(Ok(n)) => Ok(n),
    }
}

async fn reboot(cmds: &OtaCmds) -> Result<(), String> {
    let status = Command::new(&cmds.reboot[0])
        .args(&cmds.reboot[1..])
        .status()
        .await
        .map_err(|e| format!("cannot start the reboot: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("reboot command failed: {status}"))
}

// mestro-upgrade prints "ready: Image.b (<sha>)" or "written: <sha>"
fn written_sha256(stdout: &str) -> Option<&str> {
    stdout
        .split(|c: char| !c.is_ascii_hexdigit())
        .rfind(|word| word.len() == 64)
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    // the busy flag is process-global: tests take turns
    static SERIAL: Mutex<()> = Mutex::new(());

    const MIB: usize = 1 << 20;
    const STUB_SHA: &str = "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e";

    // stand-in for mestro-upgrade: records its pid and args, stores stdin
    fn upgrade_stub(dir: &Path) -> String {
        let d = dir.display();
        format!(
            "echo $$ > {d}/pid\necho \"$@\" > {d}/args\ncat > {d}/image\necho \"ready: Image.b ({STUB_SHA})\"\n"
        )
    }

    struct Rig {
        dir: PathBuf,
        cmds: Arc<OtaCmds>,
    }

    impl Rig {
        fn new(name: &str, upgrade_script: Option<&str>) -> Self {
            let dir =
                std::env::temp_dir().join(format!("mestro-ota-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let upgrade = dir.join("upgrade.sh");
            let script = upgrade_script
                .map(str::to_string)
                .unwrap_or_else(|| upgrade_stub(&dir));
            std::fs::write(&upgrade, script).unwrap();
            let reboot = dir.join("reboot.sh");
            std::fs::write(&reboot, format!("touch {}/rebooted\n", dir.display())).unwrap();
            let cmds = OtaCmds {
                upgrade: vec!["sh".into(), upgrade.display().to_string()],
                reboot: vec![
                    "sh".into(),
                    reboot.display().to_string(),
                    "--tryboot".into(),
                ],
                reboot_delay: Duration::from_millis(10),
                read_timeout: Duration::from_secs(30),
            };
            Self {
                dir,
                cmds: Arc::new(cmds),
            }
        }

        fn file(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    type Phases = Arc<Mutex<Vec<OtaPhase>>>;

    struct Server {
        addr: std::net::SocketAddr,
        done: mpsc::UnboundedReceiver<Result<(), String>>,
        phases: Phases,
    }

    // accepts connections and runs the handler on each, like `serve`
    async fn start(cmds: Arc<OtaCmds>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, done) = mpsc::unbounded_channel();
        let phases: Phases = Arc::default();
        let seen = phases.clone();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (cmds, tx, seen) = (cmds.clone(), tx.clone(), seen.clone());
                tokio::spawn(async move {
                    let request = http::read_head(&mut stream).await.unwrap();
                    let result = handle(&mut stream, &request, &cmds, |p| {
                        seen.lock().unwrap().push(p)
                    })
                    .await;
                    let _ = tx.send(result);
                });
            }
        });
        Server { addr, done, phases }
    }

    fn run<F: Future>(test: F) -> F::Output {
        let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
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

    // (status code, body) once the server closed the connection
    async fn response(client: &mut TcpStream) -> (u16, String) {
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut raw))
            .await
            .expect("response timeout")
            .unwrap();
        let text = String::from_utf8_lossy(&raw).to_string();
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    async fn wait_for(path: &Path) {
        for _ in 0..500 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{} never appeared", path.display());
    }

    #[test]
    fn upload_lands_in_the_installer_and_reboots() {
        run(async {
            let rig = Rig::new("happy", None);
            let mut server = start(rig.cmds.clone()).await;
            let body = image(5 * MIB);
            let sha = "ab".repeat(32);
            let head = request_head(
                Some(body.len()),
                &format!("X-Sha256: {}\r\n", sha.to_uppercase()),
            );
            let mut client = connect(server.addr, &head, &body).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 200);
            assert!(reply.contains(STUB_SHA), "{reply}");
            assert_eq!(server.done.recv().await.unwrap(), Ok(()));
            assert!(
                std::fs::read(rig.file("image")).unwrap() == body,
                "image differs from the upload"
            );
            assert_eq!(
                std::fs::read_to_string(rig.file("args")).unwrap().trim(),
                format!("- {sha}")
            );
            assert!(rig.file("rebooted").exists());
            let phases = server.phases.lock().unwrap().clone();
            assert!(matches!(
                phases.first(),
                Some(OtaPhase::Receiving { done: 0, .. })
            ));
            assert!(phases.contains(&OtaPhase::Verifying));
            assert_eq!(phases.last(), Some(&OtaPhase::Rebooting));
        });
    }

    #[test]
    fn wrong_magic_is_rejected_before_the_installer_starts() {
        run(async {
            let rig = Rig::new("magic", None);
            let mut server = start(rig.cmds.clone()).await;
            let head = request_head(Some(5 * MIB), "");
            let mut client = connect(server.addr, &head, &vec![0u8; 4096]).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 400);
            assert!(reply.contains("not a mestro system image"), "{reply}");
            drop(client);
            assert!(server.done.recv().await.unwrap().is_err());
            assert!(!rig.file("pid").exists(), "the installer ran");
            assert!(!rig.file("rebooted").exists());
            assert!(
                server.phases.lock().unwrap().is_empty(),
                "a rejected request must not touch the overlay"
            );
        });
    }

    #[test]
    fn missing_content_length_is_411() {
        run(async {
            let rig = Rig::new("length", None);
            let server = start(rig.cmds.clone()).await;
            let mut client = connect(server.addr, &request_head(None, ""), b"").await;
            assert_eq!(response(&mut client).await.0, 411);
            assert!(!rig.file("pid").exists());
        });
    }

    #[test]
    fn body_size_outside_the_limits_is_413() {
        run(async {
            let rig = Rig::new("limits", None);
            let server = start(rig.cmds.clone()).await;
            for len in [MIB, 193 * MIB] {
                let mut client =
                    connect(server.addr, &request_head(Some(len), ""), &image(4096)).await;
                assert_eq!(response(&mut client).await.0, 413, "{len} bytes");
            }
            assert!(!rig.file("pid").exists());
        });
    }

    #[test]
    fn malformed_sha256_header_is_400() {
        run(async {
            let rig = Rig::new("sha", None);
            let server = start(rig.cmds.clone()).await;
            let head = request_head(Some(5 * MIB), "X-Sha256: not-a-digest\r\n");
            let mut client = connect(server.addr, &head, &image(4096)).await;
            assert_eq!(response(&mut client).await.0, 400);
            assert!(!rig.file("pid").exists());
        });
    }

    #[test]
    fn client_disconnect_kills_the_installer_and_never_reboots() {
        run(async {
            let rig = Rig::new("disconnect", None);
            let mut server = start(rig.cmds.clone()).await;
            let body = image(5 * MIB);
            let head = request_head(Some(8 * MIB), "");
            let client = connect(server.addr, &head, &body).await;
            wait_for(&rig.file("pid")).await;
            drop(client);
            assert!(server.done.recv().await.unwrap().is_err());
            let pid = std::fs::read_to_string(rig.file("pid")).unwrap();
            assert!(
                !Path::new(&format!("/proc/{}", pid.trim())).exists(),
                "installer still running"
            );
            assert!(!rig.file("rebooted").exists());
            assert!(matches!(
                server.phases.lock().unwrap().last(),
                Some(OtaPhase::Failed(_))
            ));
        });
    }

    #[test]
    fn stalled_upload_times_out_and_kills_the_installer() {
        run(async {
            let mut rig = Rig::new("stall", None);
            Arc::get_mut(&mut rig.cmds).unwrap().read_timeout = Duration::from_millis(300);
            let mut server = start(rig.cmds.clone()).await;
            let head = request_head(Some(8 * MIB), "");
            let _client = connect(server.addr, &head, &image(MIB)).await;
            let result = server.done.recv().await.unwrap();
            assert!(result.unwrap_err().contains("stalled"));
            let pid = std::fs::read_to_string(rig.file("pid")).unwrap();
            assert!(
                !Path::new(&format!("/proc/{}", pid.trim())).exists(),
                "installer still running"
            );
            assert!(!rig.file("rebooted").exists());
        });
    }

    #[test]
    fn second_upload_is_409_while_one_runs() {
        run(async {
            let rig = Rig::new("busy", None);
            let mut server = start(rig.cmds.clone()).await;
            let body = image(5 * MIB);
            let head = request_head(Some(body.len()), "");
            let mut first = connect(server.addr, &head, &body[..MIB]).await;
            wait_for(&rig.file("pid")).await;

            // a refused upload must not release the running one's lock
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
    fn chatty_installer_does_not_stall_the_upload() {
        run(async {
            // 1 MiB on stderr before it reads stdin: more than any pipe buffer
            let script = format!(
                "head -c 1048576 /dev/zero | tr '\\0' x >&2\n{}",
                upgrade_stub(
                    &std::env::temp_dir().join(format!("mestro-ota-{}-chatty", std::process::id()))
                )
            );
            let mut rig = Rig::new("chatty", Some(&script));
            Arc::get_mut(&mut rig.cmds).unwrap().read_timeout = Duration::from_secs(3);
            let mut server = start(rig.cmds.clone()).await;
            let body = image(5 * MIB);
            let mut client = connect(server.addr, &request_head(Some(body.len()), ""), &body).await;
            assert_eq!(response(&mut client).await.0, 200);
            assert_eq!(server.done.recv().await.unwrap(), Ok(()));
            assert!(rig.file("rebooted").exists());
        });
    }

    #[test]
    fn client_gone_after_install_still_reboots() {
        run(async {
            // the installer takes a moment after EOF: the client leaves meanwhile
            let script = format!(
                "{}sleep 1\n",
                upgrade_stub(
                    &std::env::temp_dir().join(format!("mestro-ota-{}-gone", std::process::id()))
                )
            );
            let rig = Rig::new("gone", Some(&script));
            let mut server = start(rig.cmds.clone()).await;
            let body = image(5 * MIB);
            let client = connect(server.addr, &request_head(Some(body.len()), ""), &body).await;
            wait_for(&rig.file("image")).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(client);
            assert_eq!(server.done.recv().await.unwrap(), Ok(()));
            assert!(rig.file("rebooted").exists());
            assert_eq!(
                server.phases.lock().unwrap().last(),
                Some(&OtaPhase::Rebooting)
            );
        });
    }

    #[test]
    fn installer_failure_is_a_500_with_its_stderr_and_no_reboot() {
        run(async {
            let rig = Rig::new(
                "failure",
                Some("cat > /dev/null\necho 'sha256 mismatch: got a, want b' >&2\nexit 1\n"),
            );
            let mut server = start(rig.cmds.clone()).await;
            let body = image(5 * MIB);
            let mut client = connect(server.addr, &request_head(Some(body.len()), ""), &body).await;
            let (status, reply) = response(&mut client).await;
            assert_eq!(status, 500);
            assert!(reply.contains("sha256 mismatch: got a, want b"), "{reply}");
            assert!(server.done.recv().await.unwrap().is_err());
            assert!(!rig.file("rebooted").exists());
            let phases = server.phases.lock().unwrap().clone();
            assert!(
                matches!(phases.last(), Some(OtaPhase::Failed(m)) if m.contains("sha256 mismatch"))
            );
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
