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
use mestro_update::{self as update, BootControl, Commit, Detection, Error, UpdateLock};
use std::io::{Read, Write};
use std::path::Path;
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
    /// make the trial slot permanent (S99ota, once the trial boot
    /// came up healthy)
    Commit,
    /// drop a pending trial that never committed (S99ota, on a
    /// normal boot)
    Abandon,
    /// the running and a pending slot
    Status,
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
        // every write operation holds the cross-process update lock
        // to its end — the web OTA and S99ota take the same one.
        // An install must not wait for a running update; a commit or
        // an abandon gets S99ota's health budget
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
/// finish: S99ota decides within two minutes of boot, and one
/// upload must not roll a healthy trial back by starving it
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
