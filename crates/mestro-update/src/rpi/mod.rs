//! The Raspberry Pi firmware's boot chain: the A/B slots are two
//! kernel files on the boot partition (`Image.a`/`Image.b`), the
//! `kernel=` line of `config.txt` names the one the firmware boots,
//! and a trial boot is the firmware's one-shot *tryboot* — that boot
//! reads `tryboot.txt` instead of config.txt, and any boot after it
//! is back on config.txt: without a commit (a crash, a hang, a
//! failed health check) the previous slot boots again on its own.
//! Everything this board does for an update lives in this folder:
//! the slot handling and the mount/commit flow here, the boot config
//! as text in config.rs, the restart of the board in reboot.rs.

use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::external::run_bounded;
use crate::install::Error;
use crate::{sha256_hex, BootControl, Commit, UpdateTarget};
#[cfg(test)]
use std::sync::Mutex;

mod config;
mod reboot;

use config::Config;

/// the mounted boot partition of the image
const BOOT_DIR: &str = "/boot";
/// writing "3\n" here drops the page cache (needs root — the image has it)
const DROP_CACHES: &str = "/proc/sys/vm/drop_caches";
/// the firmware's one-shot tryboot flag for this boot
const TRYBOOT_FLAG: &str = "/proc/device-tree/chosen/bootloader/tryboot";
/// where the Pi firmware announces itself in the device tree — and
/// where its tryboot flag lives
const BOOTLOADER_TREE: &str = "/proc/device-tree/chosen/bootloader";

/// slot file names on the boot partition
const SLOT_A: &str = "Image.a";
const SLOT_B: &str = "Image.b";
/// what a card before its first A/B update boots; that update takes
/// Image.b, and the commit here promotes it to slot A — the
/// fallback of the new slot
const PLAIN_IMAGE: &str = "Image";

/// the devices the boot partition can live on, in mount order (one
/// binary for every board)
const BOOT_DEVICES: &[&str] = &["/dev/mmcblk0p1", "/dev/nvme0n1p1", "/dev/sda1"];

/// how long one boot device may take to mount
const MOUNT_TIMEOUT: Duration = Duration::from_secs(10);
/// the sync before letting go of the store
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);
/// the unmount of the store this control mounted itself
const UMOUNT_TIMEOUT: Duration = Duration::from_secs(10);

/// the external commands the real backend runs, by slot (fixed
/// paths would be the hard way — these are root's PATH on the
/// image). Tests replace a slot's entry with a fixture
const DEFAULT_EXTERNAL: &[&str] = &["mount", "sync", "umount"];
/// the slots [`RpiFirmware`] runs its external commands through
const EXTERNAL_MOUNT: usize = 0;
const EXTERNAL_SYNC: usize = 1;
const EXTERNAL_UMOUNT: usize = 2;

/// the Raspberry Pi firmware's A/B boot control
pub struct RpiFirmware {
    boot_dir: PathBuf,
    drop_caches: PathBuf,
    tryboot_flag: PathBuf,
    /// true when this control mounted the boot partition itself (and
    /// unmounts it again)
    mounted_here: bool,
    /// the external mount/umount/sync commands — tests replace
    /// entries with a fixture that never exits
    external: Vec<OsString>,
}

impl RpiFirmware {
    /// the firmware boot chain of this board, if the firmware
    /// announced itself in the device tree
    pub fn detect() -> Option<Self> {
        Self::detect_boot(BOOT_DIR, DROP_CACHES, TRYBOOT_FLAG, BOOTLOADER_TREE)
    }

    /// the same detection at explicit paths — the test seam (the
    /// fixtures point these at a temp dir)
    pub fn detect_boot(
        boot_dir: impl Into<PathBuf>,
        drop_caches: impl Into<PathBuf>,
        tryboot_flag: impl Into<PathBuf>,
        bootloader_tree: impl Into<PathBuf>,
    ) -> Option<Self> {
        bootloader_tree.into().exists().then(|| Self {
            boot_dir: boot_dir.into(),
            drop_caches: drop_caches.into(),
            tryboot_flag: tryboot_flag.into(),
            mounted_here: false,
            external: DEFAULT_EXTERNAL.iter().map(OsString::from).collect(),
        })
    }

    /// the config the next committed boot reads
    fn config(&self) -> io::Result<Config> {
        Config::read(&self.boot_dir.join("config.txt"))
    }

    /// the kernel a config file boots, or `None` when there is no
    /// such file
    fn kernel_of(&self, path: &Path) -> io::Result<Option<String>> {
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(Config::read(path)?.kernel()))
    }

    /// the slot an update writes to, and whether this card still
    /// boots the un-slotted `Image` (before its first A/B update)
    fn update_target(&self, running: &str) -> (String, bool) {
        match running {
            PLAIN_IMAGE => (SLOT_B.to_string(), true),
            SLOT_A => (SLOT_B.to_string(), false),
            // slot B, an unknown kernel or no kernel line at all:
            // the A slot is the free one
            _ => (SLOT_A.to_string(), false),
        }
    }

    /// is the slot store already reachable: the boot partition is
    /// mounted, or the store is sitting right there (the test
    /// fixtures, or a card mounted by something else)
    fn boot_reachable(&self) -> bool {
        boot_is_mounted(&self.boot_dir) || self.boot_dir.join("config.txt").exists()
    }

    fn trial_path(&self) -> PathBuf {
        self.boot_dir.join("tryboot.txt")
    }

    /// best effort: take an already published trial file back —
    /// `Err` when the take-back itself failed, which leaves the
    /// trial state unknown
    fn take_back_trial(&self) -> io::Result<()> {
        match fs::remove_file(self.trial_path()) {
            Ok(()) => sync_dir(&self.boot_dir),
            // already gone: nothing to take back
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn slot_path(&self, slot: &str) -> PathBuf {
        self.boot_dir.join(slot)
    }

    /// run one of this backend's external commands (mount, sync,
    /// umount) under its deadline — the command's slot is the test
    /// seam
    fn external_run(
        &self,
        at: usize,
        args: impl FnOnce(&mut Command) -> &mut Command,
        timeout: Duration,
    ) -> io::Result<std::process::ExitStatus> {
        run_bounded(&self.external[at], args, timeout)
    }

    /// a card that still boots the un-slotted `Image` gets its A/B
    /// layout with this commit: `Image` becomes slot A
    fn promote_plain_image(&self, config: &Config) -> Result<(), Error> {
        if config.kernel() == PLAIN_IMAGE && !self.slot_path(SLOT_A).exists() {
            fs::rename(self.slot_path(PLAIN_IMAGE), self.slot_path(SLOT_A)).map_err(Error::from)?;
        }
        Ok(())
    }
}

impl BootControl for RpiFirmware {
    fn mount(&mut self) -> Result<(), Error> {
        if self.boot_reachable() {
            return Ok(());
        }
        fs::create_dir_all(&self.boot_dir).map_err(Error::from)?;
        for device in BOOT_DEVICES {
            let mounted = self
                .external_run(
                    EXTERNAL_MOUNT,
                    |command| command.args(["-t", "vfat", device]).arg(&self.boot_dir),
                    MOUNT_TIMEOUT,
                )
                .is_ok_and(|status| status.success());
            if mounted {
                self.mounted_here = true;
                return Ok(());
            }
        }
        Err(Error::Io(io::Error::other(format!(
            "cannot mount the boot partition at {}",
            self.boot_dir.display()
        ))))
    }

    fn release(&mut self) {
        // an armed trial stays a success: the slot file and the boot
        // dir were already fsynced (install + arm_trial's sync_dir),
        // so a hung global sync cannot change what is on the card
        if self
            .external_run(EXTERNAL_SYNC, |command| command, SYNC_TIMEOUT)
            .is_err()
        {
            eprintln!("the sync before releasing the store did not finish in time");
        }
        if self.mounted_here
            && self
                .external_run(
                    EXTERNAL_UMOUNT,
                    |command| command.arg(&self.boot_dir),
                    UMOUNT_TIMEOUT,
                )
                .is_err()
        {
            eprintln!("the unmount of the store did not finish in time");
        }
        self.mounted_here = false;
    }

    fn active(&self) -> io::Result<String> {
        Ok(self.config()?.kernel())
    }

    fn pending(&self) -> io::Result<Option<String>> {
        self.kernel_of(&self.trial_path())
    }

    fn trial_boot(&self) -> bool {
        let mut flag = [0u8; 4];
        File::open(&self.tryboot_flag)
            .and_then(|mut file| file.read_exact(&mut flag))
            .is_ok()
            && u32::from_be_bytes(flag) == 1
    }

    fn begin_update(&mut self) -> Result<UpdateTarget, Error> {
        let running = self.active()?;
        let (slot, plain_image) = self.update_target(&running);
        if plain_image {
            // the manual fallback of the un-slotted Image has no
            // slot to fall back into — make room for the new one
            let _ = fs::remove_file(self.slot_path("Image.old"));
        }
        if slot == running {
            return Err(Error::RunningSlot(running));
        }
        // a stale trial and a stale slot must not survive into the
        // new update
        let _ = fs::remove_file(self.trial_path());
        let _ = fs::remove_file(self.slot_path(&slot));
        let sink = File::create(self.slot_path(&slot)).map_err(Error::from)?;
        Ok(UpdateTarget {
            slot,
            running,
            sink: Box::new(sink),
        })
    }

    fn read_back(&mut self, target: &UpdateTarget) -> io::Result<String> {
        let slot = self.slot_path(&target.slot);
        // what was written must be on the card before it is trusted
        File::open(&slot)?.sync_all()?;
        // drop the page cache so the hash sees the card, not RAM. A
        // failed drop would make the read-back see the cache and
        // lie — the update fails instead of verifying nothing
        fs::write(&self.drop_caches, b"3\n")?;
        hash_of(&slot)
    }

    fn discard(&mut self, target: &UpdateTarget) {
        let _ = fs::remove_file(self.slot_path(&target.slot));
    }

    fn arm_trial(&mut self, target: &UpdateTarget) -> io::Result<()> {
        let config = self.config()?;
        let armed = fs::write(self.trial_path(), config.trial(&target.slot))
            .and_then(|()| sync_dir(&self.boot_dir));
        armed.map_err(|failure| match self.take_back_trial() {
            Ok(()) => failure,
            // the trial file may or may not survive a crash now —
            // the next boot could be one nobody decided on
            Err(removal) => io::Error::other(format!(
                "trial state unknown: the arm failed ({failure}) and \
                 taking the trial back failed ({removal}) — \
                 check `mestro-update status`"
            )),
        })
    }

    fn commit(&mut self) -> Result<Commit, Error> {
        let trial_path = self.trial_path();
        if !self.trial_boot() || !trial_path.exists() {
            return Ok(Commit::NoTrial);
        }
        let kernel = Config::read(&trial_path)?.kernel();
        let config = self.config()?;
        // one FAT rename over config.txt: a power cut leaves the old
        // or the new config, never a half-written one
        let staged = self.boot_dir.join("config.txt.new");
        fs::write(&staged, config.with_kernel(&kernel)).map_err(Error::from)?;
        File::open(&staged)?.sync_all().map_err(Error::from)?;
        fs::rename(&staged, self.boot_dir.join("config.txt")).map_err(Error::from)?;
        fs::remove_file(&trial_path).map_err(Error::from)?;
        self.promote_plain_image(&config)?;
        sync_dir(&self.boot_dir).map_err(Error::from)?;
        Ok(Commit::Committed { kernel })
    }

    fn abandon(&mut self) -> io::Result<Option<String>> {
        match self.kernel_of(&self.trial_path())? {
            Some(kernel) => {
                fs::remove_file(self.trial_path())?;
                Ok(Some(kernel))
            }
            None => Ok(None),
        }
    }

    fn reboot(&mut self, into_trial: bool) -> io::Result<()> {
        reboot::reboot(into_trial)
    }
}

/// the sha256 of a file, streaming
fn hash_of(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; 128 * 1024];
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            return Ok(sha256_hex(&hasher.finalize()));
        }
        hasher.update(&chunk[..n]);
    }
}

/// one FAT-level sync of the directory, so a power cut leaves whole
/// files, not halves
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(test)]
    if dir_sync_injection(dir) {
        return Err(io::Error::other("directory sync failed (injected)"));
    }
    File::open(dir)?.sync_all()
}

/// tests make the sync of one directory fail a set number of
/// times — the fault injection for failures after the trial file
/// is written. Keyed by directory: tests run in parallel, each
/// against its own fixture
#[cfg(test)]
static FAILING_DIR_SYNCS: Mutex<Vec<(PathBuf, usize)>> = Mutex::new(Vec::new());

#[cfg(test)]
fn dir_sync_injection(dir: &Path) -> bool {
    let mut failing = FAILING_DIR_SYNCS.lock().unwrap();
    let Some((_, left)) = failing.iter_mut().find(|(fail_dir, _)| fail_dir == dir) else {
        return false;
    };
    if *left == 0 {
        return false;
    }
    *left -= 1;
    true
}

/// whether /proc/mounts lists `dir` as a mount point
fn boot_is_mounted(dir: &Path) -> bool {
    fs::read_to_string("/proc/mounts")
        .map(|mounts| {
            mounts
                .lines()
                .any(|line| line.split(' ').nth(1) == Some(&dir.to_string_lossy()))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::Instant;

    /// a boot-partition fixture with the given files
    fn rig(files: &[(&str, &[u8])]) -> (tempfile::TempDir, RpiFirmware) {
        let dir = tempfile::tempdir().unwrap();
        let boot = dir.path().join("boot");
        fs::create_dir_all(&boot).unwrap();
        for (name, bytes) in files {
            fs::write(boot.join(name), bytes).unwrap();
        }
        // the device tree marks the Pi firmware
        let tree = dir.path().join("bootloader");
        fs::create_dir(&tree).unwrap();
        let firmware = RpiFirmware::detect_boot(
            &boot,
            dir.path().join("drop-caches"),
            dir.path().join("tryboot-flag"),
            &tree,
        )
        .expect("the fixture tree marks the Pi firmware");
        (dir, firmware)
    }

    fn flag(firmware: &RpiFirmware, bytes: &[u8]) {
        fs::write(&firmware.tryboot_flag, bytes).unwrap();
    }

    /// replace one of the backend's external commands for the test
    fn fake_command(firmware: &mut RpiFirmware, at: usize, command: impl AsRef<OsStr>) {
        firmware.external[at] = OsString::from(command.as_ref());
    }

    #[test]
    fn detection_follows_the_device_tree() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("bootloader");
        assert!(
            RpiFirmware::detect_boot(dir.path(), dir.path(), dir.path(), &tree).is_none(),
            "no bootloader tree, no Pi firmware"
        );
        fs::create_dir(&tree).unwrap();
        assert!(RpiFirmware::detect_boot(dir.path(), dir.path(), dir.path(), &tree).is_some());
    }

    #[test]
    fn the_tryboot_flag_is_four_big_endian_bytes_of_one() {
        let (_dir, firmware) = rig(&[("config.txt", b"kernel=Image.a\n")]);
        flag(&firmware, &[0, 0, 0, 1]);
        assert!(firmware.trial_boot());
        flag(&firmware, &[0, 0, 0, 0]);
        assert!(!firmware.trial_boot());
        flag(&firmware, &[0, 0, 1]);
        assert!(!firmware.trial_boot(), "a short flag is not 1");
        fs::remove_file(&firmware.tryboot_flag).unwrap();
        assert!(!firmware.trial_boot());
    }

    #[test]
    fn mount_leaves_an_already_reachable_store_alone() {
        let (_dir, mut firmware) = rig(&[("config.txt", b"kernel=Image.a\n")]);
        // the fixture store is reachable: mounting must not try to
        // mount anything (there is no device to mount)
        firmware.mount().unwrap();
        assert!(!firmware.mounted_here, "nothing was mounted");
    }

    #[test]
    fn begin_update_targets_the_other_slot() {
        let (_dir, mut firmware) = rig(&[
            ("config.txt", b"kernel=Image.a\n"),
            ("Image.a", b"the running slot"),
        ]);
        let target = firmware.begin_update().unwrap();
        assert_eq!(target.slot, "Image.b");
        assert_eq!(target.running, "Image.a");

        let (_dir, mut firmware) = rig(&[
            ("config.txt", b"kernel=Image.b\n"),
            ("Image.b", b"the running slot"),
        ]);
        assert_eq!(firmware.begin_update().unwrap().slot, "Image.a");

        // no kernel line at all: the A slot is the free one
        let (_dir, mut firmware) = rig(&[("config.txt", b"arm_64bit=1\n")]);
        assert_eq!(firmware.begin_update().unwrap().slot, "Image.a");
    }

    #[test]
    fn a_pre_ab_card_updates_to_image_b_and_drops_its_fallback() {
        let (_dir, mut firmware) = rig(&[
            ("config.txt", b"kernel=Image\n"),
            ("Image", b"the original"),
            ("Image.old", b"the manual fallback"),
            ("tryboot.txt", b"kernel=Image.b\n"),
        ]);
        assert!(firmware.boot_dir.join("Image.old").exists());
        let target = firmware.begin_update().unwrap();
        assert_eq!(target.slot, "Image.b");
        // the fallback is gone, the original stays until the commit
        assert!(!firmware.boot_dir.join("Image.old").exists());
        assert!(firmware.boot_dir.join("Image").exists());
        // the stale trial is gone too
        assert!(!firmware.trial_path().exists());
    }

    #[test]
    fn commit_promotes_a_plain_image_to_slot_a() {
        let (_dir, mut firmware) = rig(&[
            ("config.txt", b"kernel=Image\n"),
            ("Image", b"the original"),
            ("Image.b", b"the trial"),
            ("tryboot.txt", b"kernel=Image.b\n"),
        ]);
        flag(&firmware, &[0, 0, 0, 1]);
        assert_eq!(
            firmware.commit().unwrap(),
            Commit::Committed {
                kernel: "Image.b".into()
            }
        );
        assert_eq!(
            fs::read(firmware.slot_path("Image.a")).unwrap(),
            b"the original"
        );
        assert!(!firmware.slot_path(PLAIN_IMAGE).exists());
    }

    #[test]
    fn commit_without_a_trial_boot_changes_nothing() {
        let trial: &[u8] = b"kernel=Image.b\n\n[all]\nkernel_watchdog_timeout=60\n";
        let (_dir, mut firmware) = rig(&[
            ("config.txt", b"kernel=Image.a\n"),
            ("Image.a", b"running"),
            ("tryboot.txt", trial),
        ]);
        assert_eq!(firmware.commit().unwrap(), Commit::NoTrial);
        assert_eq!(fs::read(firmware.trial_path()).unwrap(), trial);
        assert_eq!(
            fs::read_to_string(firmware.boot_dir.join("config.txt")).unwrap(),
            "kernel=Image.a\n"
        );

        // a trial boot without a pending trial
        let (_dir, mut firmware) = rig(&[("config.txt", b"kernel=Image.a\n")]);
        flag(&firmware, &[0, 0, 0, 1]);
        assert_eq!(firmware.commit().unwrap(), Commit::NoTrial);
    }

    #[test]
    fn read_back_hashes_the_file_behind_the_cache_drop() {
        let (_dir, mut firmware) = rig(&[("config.txt", b"kernel=Image.a\n")]);
        let mut target = firmware.begin_update().unwrap();
        target.sink.write_all(b"an image").unwrap();
        // close the slot file, like the install loop does at its end
        target.sink = Box::new(std::io::sink());
        let sha = firmware.read_back(&target).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"an image");
        assert_eq!(sha, sha256_hex(&hasher.finalize()));
        // the page-cache drop went through the injected path
        assert_eq!(fs::read(&firmware.drop_caches).unwrap(), b"3\n");
    }

    #[test]
    fn a_failed_cache_drop_fails_the_install_and_discards_the_slot() {
        // a drop-caches path that cannot be written (a directory)
        // would make the read-back hash the page cache instead of
        // the card — the update must fail, not verify nothing
        let (_dir, mut firmware) = rig(&[("config.txt", b"kernel=Image.a\n")]);
        fs::create_dir_all(&firmware.drop_caches).unwrap();
        let mut target = firmware.begin_update().unwrap();
        let err =
            crate::install(&mut firmware, &mut target, &mut &b"an image"[..], None).unwrap_err();
        assert!(matches!(err, crate::Error::Io(_)), "{err}");
        assert!(
            !firmware.slot_path(&target.slot).exists(),
            "the slot is discarded"
        );
        assert!(
            !firmware.trial_path().exists(),
            "no trial of an unverified write"
        );
    }

    /// the next `left` directory syncs of `dir` fail
    fn fail_dir_syncs(dir: &Path, left: usize) {
        FAILING_DIR_SYNCS
            .lock()
            .unwrap()
            .push((dir.to_path_buf(), left));
    }

    fn stop_failing_dir_syncs(dir: &Path) {
        FAILING_DIR_SYNCS
            .lock()
            .unwrap()
            .retain(|(fail_dir, _)| fail_dir != dir);
    }

    #[test]
    fn a_failed_sync_takes_a_published_trial_file_back() {
        // the trial file is already on the card when its directory
        // sync fails: the refusal must not leave it behind — a
        // reboot would start a trial nobody decided on
        let (_dir, mut firmware) = rig(&[
            ("config.txt", b"kernel=Image.a\n"),
            ("Image.a", b"the running slot"),
        ]);
        let target = firmware.begin_update().unwrap();
        fail_dir_syncs(&firmware.boot_dir, 1);
        let err = firmware.arm_trial(&target).unwrap_err();
        assert_eq!(err.to_string(), "directory sync failed (injected)");
        assert!(
            !firmware.trial_path().exists(),
            "the published trial is taken back"
        );
        stop_failing_dir_syncs(&firmware.boot_dir);
    }

    #[test]
    fn a_failed_take_back_reports_an_unknown_trial_state() {
        // the arm fails and so does taking the trial back: the
        // answer must not pretend to know the state
        let (_dir, mut firmware) = rig(&[
            ("config.txt", b"kernel=Image.a\n"),
            ("Image.a", b"the running slot"),
        ]);
        let target = firmware.begin_update().unwrap();
        fail_dir_syncs(&firmware.boot_dir, 2);
        let err = firmware.arm_trial(&target).unwrap_err();
        assert!(err.to_string().contains("trial state unknown"), "{err}");
        assert!(
            err.to_string().contains("mestro-update status"),
            "the answer points at the truth: {err}"
        );
        stop_failing_dir_syncs(&firmware.boot_dir);
    }

    /// a command that ignores its arguments and never exits — the
    /// hung external process of the fixtures
    fn never_exits(dir: &Path) -> PathBuf {
        let script = dir.join("never-exits");
        fs::write(&script, "#!/bin/sh\nexec sleep 3600\n").unwrap();
        let mut mode = fs::metadata(&script).unwrap().permissions();
        mode.set_mode(0o755);
        fs::set_permissions(&script, mode).unwrap();
        script
    }

    #[test]
    fn a_mount_that_never_finishes_fails_within_its_bound() {
        // an external command that never exits must not hang the
        // update task: the mount fails once its budget is up, and
        // the caller answers its own error instead of waiting forever
        let (dir, mut firmware) = rig(&[]);
        // the fixture store would count as reachable: drop it, so
        // mount() runs its real path
        fs::remove_dir(&firmware.boot_dir).unwrap();
        fake_command(&mut firmware, EXTERNAL_MOUNT, never_exits(dir.path()));
        let started = Instant::now();
        let err = firmware.mount().unwrap_err();
        assert!(started.elapsed() >= MOUNT_TIMEOUT, "the budget ran");
        assert!(
            started.elapsed() < BOOT_DEVICES.len() as u32 * MOUNT_TIMEOUT + Duration::from_secs(5),
            "the bound holds"
        );
        assert!(err.to_string().contains("cannot mount"), "{err}");
        assert!(!firmware.mounted_here);
    }

    #[test]
    fn a_release_that_never_finishes_returns_within_its_bound() {
        // the sync and the umount never exit: release still returns
        // once their budgets are up. What is on the card does not
        // change — the slot file and the boot dir were fsynced
        // before (install + arm_trial's sync_dir)
        let (dir, mut firmware) = rig(&[("config.txt", b"kernel=Image.a\n")]);
        firmware.mounted_here = true;
        let hanging = never_exits(dir.path());
        fake_command(&mut firmware, EXTERNAL_SYNC, &hanging);
        fake_command(&mut firmware, EXTERNAL_UMOUNT, &hanging);
        let started = Instant::now();
        firmware.release();
        assert!(started.elapsed() >= SYNC_TIMEOUT, "the budget ran");
        assert!(
            started.elapsed() < SYNC_TIMEOUT + UMOUNT_TIMEOUT + Duration::from_secs(5),
            "the bound holds"
        );
    }
}
