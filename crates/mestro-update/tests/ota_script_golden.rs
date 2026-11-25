#![cfg(feature = "rpi")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! The golden/characterization test of the A/B update path: the
//! image's original POSIX `mestro-upgrade` script (frozen as
//! `tests/mestro-upgrade.sh` when this port replaced it — the
//! overlay ships a thin wrapper now) against the `RpiFirmware`
//! backend (through the neutral update code) on the same fixture
//! trees, tree for tree and status line for status line. The
//! script is the spec — this test fails the moment the port
//! drifts from it.
//!
//! The script's /proc paths and `/boot` are patched to fixture
//! files (the patch is pinned below), a stubbed `hexdump` decodes
//! the tryboot flag the way busybox's does, and a fake /proc/mounts
//! lists the boot dir so the script never mounts anything. The rest
//! (sed, sha256sum, tee, mv …) is the real toolchain, the same one
//! the image ships — so the comparison covers the actual bytes,
//! not an approximation. The CLI's own stdout lines ("writing …",
//! "ready: …") are pinned in the binary crate's unit tests; this
//! test pins the tree and the status output, the parts the
//! diagnostics download and S99ota parse.

use mestro_update::{install, status, BootControl, Commit, RpiFirmware};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// the script this port must match: the image's original
/// `mestro-upgrade`, frozen here byte for byte when the Rust port
/// replaced it (the rootfs overlay ships a thin wrapper now)
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mestro-upgrade.sh");

const HEXDUMP_STUB: &str = r#"#!/bin/sh
# golden-test stub of busybox `hexdump -v -e '4/1 "%02x"' <file>`:
# the file's first 4 bytes as big-endian hex
[ -r "$4" ] || exit 1
set -- $(od -An -tu1 "$4")
[ $# -ge 4 ] || exit 1
printf '%02x%02x%02x%02x\n' "$1" "$2" "$3" "$4"
"#;

fn sh() -> bool {
    Command::new("sh")
        .arg("-c")
        .arg("command -v sed sha256sum tee od")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn sha_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// one sandboxed boot partition: the fixture files, the patched
/// script and the stubbed environment around it
struct Rig {
    _dir: TempDir,
    boot: PathBuf,
    script: PathBuf,
    stub: PathBuf,
    flag: PathBuf,
}

impl Rig {
    /// `fixture`: (file name, bytes) in the boot dir; the tryboot
    /// flag starts as a normal boot
    fn new(fixture: &[(&str, &[u8])], trial_boot: bool) -> Self {
        let dir = TempDir::new().unwrap();
        let boot = dir.path().join("boot");
        std::fs::create_dir_all(&boot).unwrap();
        for (name, bytes) in fixture {
            std::fs::write(boot.join(name), bytes).unwrap();
        }
        let flag = dir.path().join("tryboot-flag");
        std::fs::write(
            &flag,
            if trial_boot {
                [0, 0, 0, 1]
            } else {
                [0, 0, 0, 0]
            },
        )
        .unwrap();
        // a fake /proc/mounts that lists the boot dir: the script's
        // mount_boot returns right away, exactly like on the image
        let mounts = dir.path().join("mounts");
        std::fs::write(
            &mounts,
            format!("/dev/mmcblk0p1 {} vfat rw,relatime 0 0\n", boot.display()),
        )
        .unwrap();
        let drop_caches = dir.path().join("drop-caches");
        std::fs::write(&drop_caches, b"").unwrap();
        // the device tree's chosen/bootloader marks the Pi firmware
        // (that is the detection rule of the real backend too)
        let bootloader_tree = dir.path().join("bootloader");
        std::fs::create_dir(&bootloader_tree).unwrap();

        let stub = dir.path().join("stub");
        std::fs::create_dir(&stub).unwrap();
        let hexdump = stub.join("hexdump");
        std::fs::write(&hexdump, HEXDUMP_STUB).unwrap();
        set_executable(&hexdump);

        let script = dir.path().join("mestro-upgrade");
        let original = std::fs::read_to_string(SCRIPT)
            .unwrap_or_else(|e| panic!("read the image's mestro-upgrade script ({SCRIPT}): {e}"));
        let replacements: [(&str, String); 4] = [
            ("BOOT=/boot", format!("BOOT={}", boot.display())),
            ("/proc/mounts", mounts.display().to_string()),
            (
                "/proc/sys/vm/drop_caches",
                drop_caches.display().to_string(),
            ),
            (
                "/proc/device-tree/chosen/bootloader/tryboot",
                flag.display().to_string(),
            ),
        ];
        let mut patched = original;
        for (needle, value) in &replacements {
            assert!(
                patched.contains(needle),
                "the script moved its {needle} path — update this test"
            );
            patched = patched.replace(needle, value);
        }
        std::fs::write(&script, patched).unwrap();

        Self {
            _dir: dir,
            boot,
            script,
            stub,
            flag,
        }
    }

    /// the same boot dir, seen by the Rust port through the same
    /// detection the image runs
    fn firmware(&self) -> RpiFirmware {
        RpiFirmware::detect_boot(
            &self.boot,
            self._dir.path().join("drop-caches"),
            self.flag.clone(),
            self._dir.path().join("bootloader"),
        )
        .expect("the fixture device tree marks the Pi firmware")
    }

    /// the script's boot dir as a sorted file tree
    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        dir_tree(&self.boot)
    }

    /// the script's `--status` output (after whatever ran before)
    fn script_status(&self) -> String {
        let out = self.run(&["--status"]);
        assert_eq!(out.0, 0, "the script's status failed: {}", out.2);
        out.1
    }

    /// (exit code, stdout, stderr) of the patched script
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut path_env = std::env::var_os("PATH").unwrap_or_default();
        let mut path = self.stub.as_os_str().to_os_string();
        path.push(":");
        path.push(&path_env);
        path_env = path;
        let output = Command::new("sh")
            .arg(&self.script)
            .args(args)
            .env("PATH", &path_env)
            .output()
            .expect("run the patched script");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn dir_tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut tree = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path.strip_prefix(dir.parent().unwrap()).unwrap();
                tree.insert(name.display().to_string(), std::fs::read(&path).unwrap());
            }
        }
    }
    tree
}

/// the running-slot fixture of a card already on the A/B scheme
fn stage_image(rig: &Rig, image: &'static [u8]) -> (String, String) {
    let path = rig._dir.path().join("new-image");
    std::fs::write(&path, image).unwrap();
    (path.display().to_string(), sha_of(image))
}

/// run script install and the Rust port on two identical fixtures
/// and compare trees and status output. `correct_sha`: the expected
/// sha256 matches the image (the verified path); false pins the
/// mismatch path (the target is removed, no trial armed)
fn golden_install(fixture: &[(&str, &[u8])], trial_boot: bool, correct_sha: bool) {
    assert!(
        sh(),
        "the golden test needs a POSIX sh with sed, sha256sum, tee and od"
    );

    let image: &'static [u8] = b"the new system image, as one kernel file";
    let sha = sha_of(image);
    let want = if correct_sha {
        sha.clone()
    } else {
        "0".repeat(64)
    };

    let script = Rig::new(fixture, trial_boot);
    let (image_file, _) = stage_image(&script, image);
    let (code, out, err) = script.run(&[&image_file, &want]);
    if correct_sha {
        assert_eq!(code, 0, "the script's install failed: {out} {err}");
        assert!(out.contains("writing "), "{out}");
        assert!(out.contains("ready: "), "{out}");
    } else {
        assert_eq!(code, 1, "a sha mismatch must fail: {out} {err}");
        assert!(err.contains("sha256 mismatch"), "{err}");
    }
    let script_tree = script.tree();
    let script_status = script.script_status();

    let rust = Rig::new(fixture, trial_boot);
    let mut firmware = rust.firmware();
    firmware.mount().unwrap();
    let mut target = firmware.begin_update().unwrap();
    let result = install(&mut firmware, &mut target, &mut &image[..], Some(&want));
    if correct_sha {
        let installed = result.unwrap();
        assert_eq!(installed.slot, target.slot);
        assert_eq!(installed.sha256, sha);
    } else {
        assert!(result.is_err(), "the port must refuse the wrong sha too");
    }
    firmware.release();
    assert_eq!(
        rust.tree(),
        script_tree,
        "the port's tree differs from the script's"
    );
    assert_eq!(status(&firmware).unwrap(), script_status);
}

#[test]
fn install_writes_the_inactive_slot_a_to_b() {
    golden_install(
        &[
            ("config.txt", b"arm_64bit=1\nkernel=Image.a\n"),
            ("Image.a", b"the running slot"),
        ],
        false,
        true,
    );
}

#[test]
fn install_writes_the_inactive_slot_b_to_a() {
    golden_install(
        &[
            ("config.txt", b"kernel=Image.b\n"),
            ("Image.b", b"the running slot"),
        ],
        false,
        true,
    );
}

#[test]
fn install_on_a_pre_ab_card_takes_image_b_and_drops_image_old() {
    golden_install(
        &[
            ("config.txt", b"kernel=Image\n"),
            ("Image", b"the original"),
            ("Image.old", b"the manual fallback"),
        ],
        false,
        true,
    );
}

#[test]
fn install_without_a_kernel_line_takes_image_a_like_the_scripts_case() {
    golden_install(
        &[
            ("config.txt", b"arm_64bit=1\n"),
            ("Image", b"booted by the firmware default"),
        ],
        false,
        true,
    );
}

#[test]
fn install_with_the_wrong_sha_removes_the_target_and_arms_nothing() {
    golden_install(
        &[
            ("config.txt", b"kernel=Image.a\n"),
            ("Image.a", b"the running slot"),
        ],
        false,
        false,
    );
}

#[test]
fn install_without_a_sha_still_arms_the_trial() {
    // no sha means the script's reader is the same; arm with the
    // written hash — the golden covers the sha-verified path above,
    // this one pins the no-sha tree (a stale trial is dropped too)
    assert!(sh());
    let fixture: &[(&str, &[u8])] = &[
        ("config.txt", b"kernel=Image.a\n"),
        ("Image.a", b"the running slot"),
        (
            "tryboot.txt",
            b"kernel=Image.b\n\n[all]\nkernel_watchdog_timeout=60\n",
        ),
    ];
    let script = Rig::new(fixture, false);
    let image = b"an image without a sha";
    let image_file = script._dir.path().join("new-image");
    std::fs::write(&image_file, image).unwrap();
    let (code, out, err) = script.run(&[&image_file.display().to_string()]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("no sha256 given"), "{out}");
    let script_tree = script.tree();
    let script_status = script.script_status();

    let rust = Rig::new(fixture, false);
    let mut firmware = rust.firmware();
    firmware.mount().unwrap();
    let mut target = firmware.begin_update().unwrap();
    let installed = install(&mut firmware, &mut target, &mut &image[..], None).unwrap();
    firmware.release();
    assert_eq!(installed.sha256, sha_of(image));
    assert_eq!(rust.tree(), script_tree);
    assert_eq!(status(&firmware).unwrap(), script_status);
}

/// run the script's mode and the Rust port on identical fixtures,
/// then compare trees and status output
fn golden_mode(
    fixture: &[(&str, &[u8])],
    trial_boot: bool,
    script_args: &[&str],
    expect_commit: Option<&str>,
    rust_op: impl FnOnce(&mut RpiFirmware) -> Result<Commit, mestro_update::Error>,
) {
    assert!(sh());
    let script = Rig::new(fixture, trial_boot);
    let (code, out, err) = script.run(script_args);
    assert_eq!(code, 0, "the script failed: {out} {err}");
    if let Some(kernel) = expect_commit {
        assert!(out.contains(&format!("committed: {kernel}")), "{out}");
    }
    let script_tree = script.tree();
    let script_status = script.script_status();

    let rust = Rig::new(fixture, trial_boot);
    let mut firmware = rust.firmware();
    firmware.mount().unwrap();
    let outcome = rust_op(&mut firmware).unwrap();
    firmware.release();
    if let Some(kernel) = expect_commit {
        assert_eq!(
            outcome,
            Commit::Committed {
                kernel: kernel.to_string()
            }
        );
    } else {
        assert_eq!(outcome, Commit::NoTrial, "the script changed nothing");
    }
    assert_eq!(
        rust.tree(),
        script_tree,
        "the port's tree differs from the script's"
    );
    assert_eq!(status(&firmware).unwrap(), script_status);
}

#[test]
fn commit_makes_the_trial_slot_permanent() {
    golden_mode(
        &[
            ("config.txt", b"arm_64bit=1\nkernel=Image.a\n"),
            ("Image.a", b"the old slot"),
            ("Image.b", b"the trial slot"),
            (
                "tryboot.txt",
                b"arm_64bit=1\nkernel=Image.b\n\n[all]\nkernel_watchdog_timeout=60\n",
            ),
        ],
        true,
        &["--commit"],
        Some("Image.b"),
        |firmware| firmware.commit(),
    );
}

#[test]
fn commit_on_a_pre_ab_card_makes_image_slot_a() {
    golden_mode(
        &[
            ("config.txt", b"kernel=Image\n"),
            ("Image", b"the original"),
            ("Image.b", b"the trial slot"),
            (
                "tryboot.txt",
                b"kernel=Image.b\n\n[all]\nkernel_watchdog_timeout=60\n",
            ),
        ],
        true,
        &["--commit"],
        Some("Image.b"),
        |firmware| firmware.commit(),
    );
}

#[test]
fn commit_without_a_trial_boot_is_a_no_op() {
    golden_mode(
        &[
            ("config.txt", b"kernel=Image.a\n"),
            ("Image.a", b"the old slot"),
            ("Image.b", b"the trial slot"),
            (
                "tryboot.txt",
                b"kernel=Image.b\n\n[all]\nkernel_watchdog_timeout=60\n",
            ),
        ],
        false,
        &["--commit"],
        None,
        |firmware| firmware.commit(),
    );
}

#[test]
fn commit_without_a_pending_trial_is_a_no_op() {
    golden_mode(
        &[
            ("config.txt", b"kernel=Image.a\n"),
            ("Image.a", b"the running slot"),
        ],
        true,
        &["--commit"],
        None,
        |firmware| firmware.commit(),
    );
}

#[test]
fn abandon_drops_a_pending_trial() {
    assert!(sh());
    let fixture: &[(&str, &[u8])] = &[
        ("config.txt", b"kernel=Image.a\n"),
        ("Image.a", b"the running slot"),
        (
            "tryboot.txt",
            b"kernel=Image.b\n\n[all]\nkernel_watchdog_timeout=60\n",
        ),
    ];
    let script = Rig::new(fixture, false);
    let (code, out, err) = script.run(&["--abandon"]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(
        out.contains("trial of Image.b never committed, dropped"),
        "{out}"
    );
    let script_tree = script.tree();
    let script_status = script.script_status();

    let rust = Rig::new(fixture, false);
    let mut firmware = rust.firmware();
    firmware.mount().unwrap();
    assert_eq!(firmware.abandon().unwrap(), Some("Image.b".into()));
    firmware.release();
    assert_eq!(rust.tree(), script_tree);
    assert_eq!(status(&firmware).unwrap(), script_status);

    // nothing pending: nothing happens, nothing is said
    let (code, out, _) = script.run(&["--abandon"]);
    assert_eq!((code, out.as_str()), (0, ""));
    assert_eq!(firmware.abandon().unwrap(), None);
}

/// one status case: the boot dir's files, and whether this boot
/// is a trial
type Case = (&'static [(&'static str, &'static [u8])], bool);

#[test]
fn status_matches_the_script_byte_for_byte() {
    assert!(sh());
    let cases: [Case; 4] = [
        (
            &[("config.txt", b"kernel=Image.a\n"), ("Image.a", b"running")],
            false,
        ),
        (
            &[("config.txt", b"kernel=Image.a\n"), ("Image.a", b"running")],
            true,
        ),
        (
            &[
                ("config.txt", b"arm_64bit=1\nkernel=Image.b\n"),
                ("Image.b", b"running"),
                (
                    "tryboot.txt",
                    b"kernel=Image.a\n\n[all]\nkernel_watchdog_timeout=60\n",
                ),
            ],
            false,
        ),
        (
            &[
                ("config.txt", b"arm_64bit=1\nkernel=Image.b\n"),
                ("Image.b", b"running"),
                (
                    "tryboot.txt",
                    b"kernel=Image.a\n\n[all]\nkernel_watchdog_timeout=60\n",
                ),
            ],
            true,
        ),
    ];
    for (fixture, trial_boot) in cases {
        let script = Rig::new(fixture, trial_boot);
        let script_status = script.script_status();
        let rust = Rig::new(fixture, trial_boot);
        assert_eq!(
            status(&rust.firmware()).unwrap(),
            script_status,
            "fixture: {fixture:?}"
        );
    }
}
