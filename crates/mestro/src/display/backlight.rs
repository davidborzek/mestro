//! Where a brightness level goes: the platform backends behind
//! [`crate::display::Display`].
//!
//! - **Pi firmware**: the VideoCore mailbox (`SET_BACKLIGHT` via
//!   `/dev/vcio`). Chosen when the firmware already drives the
//!   panel's touch: then it owns the display i2c bus, and an ARM-
//!   side write through the i2c mux collides with its polling —
//!   that hung the firmware (slider drags, hours of standby
//!   transitions).
//! - **sysfs**: `/sys/class/backlight/<dev>` — laptops, panels with
//!   their own kernel driver.
//! - **none**: nothing to control (VMs, HDMI monitors).
//!
//! [`detect`] picks one; `[display] backlight` overrides it.

use std::fs;
use std::path::{Path, PathBuf};

/// one way to set the backlight
pub enum Backlight {
    PiFirmware(FirmwareMailbox),
    Sysfs { dir: PathBuf, max: u32 },
    None,
}

/// `[display] backlight`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BacklightChoice {
    /// detect (the default)
    #[default]
    Auto,
    Firmware,
    Sysfs,
    None,
}

/// the paths detection reads — real system paths in the app, a
/// fake tree in tests
pub struct Roots {
    /// the Pi firmware mailbox device
    pub rpi_vcio: PathBuf,
    /// the Pi firmware touch driver (raspberrypi-ts): present when
    /// the firmware polls the panel's touch controller itself
    pub rpi_fw_touch: PathBuf,
    pub sysfs_backlight: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            rpi_vcio: "/dev/vcio".into(),
            rpi_fw_touch: "/sys/bus/platform/drivers/raspberrypi-ts".into(),
            sysfs_backlight: "/sys/class/backlight".into(),
        }
    }
}

/// what detection would pick, without opening anything — the
/// decision alone, testable on any machine
#[derive(Debug, PartialEq, Eq)]
pub enum Detected {
    PiFirmware,
    Sysfs(PathBuf),
    None,
}

pub fn decide(choice: BacklightChoice, roots: &Roots) -> Detected {
    let firmware = || roots.rpi_vcio.exists();
    let sysfs = || {
        fs::read_dir(&roots.sysfs_backlight)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join("brightness").exists())
    };
    match choice {
        BacklightChoice::None => Detected::None,
        BacklightChoice::Firmware if firmware() => Detected::PiFirmware,
        BacklightChoice::Firmware => Detected::None,
        BacklightChoice::Sysfs => sysfs().map(Detected::Sysfs).unwrap_or(Detected::None),
        BacklightChoice::Auto => {
            // the firmware drives the panel's touch → it owns the bus:
            // never touch it from the ARM side
            if firmware() && roots.rpi_fw_touch.exists() {
                Detected::PiFirmware
            } else if let Some(dir) = sysfs() {
                Detected::Sysfs(dir)
            } else {
                Detected::None
            }
        }
    }
}

/// open what [`decide`] picked
pub fn detect(choice: BacklightChoice, roots: &Roots) -> Backlight {
    match decide(choice, roots) {
        Detected::PiFirmware => match FirmwareMailbox::open(&roots.rpi_vcio) {
            Ok(mailbox) => Backlight::PiFirmware(mailbox),
            Err(e) => {
                tracing::warn!("display: firmware mailbox unavailable ({e}), no backlight control");
                Backlight::None
            }
        },
        Detected::Sysfs(dir) => {
            let max = read_u32(&dir.join("max_brightness")).unwrap_or(1).max(1);
            Backlight::Sysfs { dir, max }
        }
        Detected::None => Backlight::None,
    }
}

impl Backlight {
    pub fn name(&self) -> &'static str {
        match self {
            Backlight::PiFirmware(_) => "pi-firmware",
            Backlight::Sysfs { .. } => "sysfs",
            Backlight::None => "none",
        }
    }

    pub fn supported(&self) -> bool {
        !matches!(self, Backlight::None)
    }

    /// current level in percent, where the backend can tell (the
    /// firmware has no getter)
    pub fn read(&self) -> Option<u8> {
        match self {
            Backlight::Sysfs { dir, max } => {
                read_u32(&dir.join("brightness")).map(|raw| percent_of(raw, *max))
            }
            _ => None,
        }
    }

    /// set a level in percent (0..=100)
    pub fn write(&self, percent: u8) -> std::io::Result<()> {
        let percent = percent.min(100) as u32;
        match self {
            Backlight::PiFirmware(mailbox) => mailbox.set_backlight(percent * 255 / 100),
            Backlight::Sysfs { dir, max } => {
                fs::write(dir.join("brightness"), (percent * max / 100).to_string())
            }
            Backlight::None => Ok(()),
        }
    }
}

/// The VideoCore property mailbox, the path the kernel's
/// rpi_backlight driver takes: one ioctl on /dev/vcio.
pub struct FirmwareMailbox {
    file: fs::File,
}

/// _IOWR(100, 0, char *)
const IOCTL_MBOX_PROPERTY: libc::c_ulong = 0xc008_6400;
const TAG_SET_BACKLIGHT: u32 = 0x0004_800f;
const RESPONSE_OK: u32 = 0x8000_0000;

impl FirmwareMailbox {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self { file })
    }

    /// brightness 0..=255
    fn set_backlight(&self, value: u32) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        // buffer size, request code, tag, value size, req/resp,
        // value, end tag, padding — 16-byte aligned for the GPU
        #[repr(C, align(16))]
        struct Message([u32; 8]);
        let mut msg = Message([32, 0, TAG_SET_BACKLIGHT, 4, 0, value.min(255), 0, 0]);
        // SAFETY: a valid, aligned buffer of the size its first word
        // declares; the ioctl reads and writes within it
        // (unsafe_code is denied crate-wide — this is the audited
        // Pi firmware mailbox ioctl)
        #[allow(unsafe_code)]
        let rc = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                IOCTL_MBOX_PROPERTY as _,
                msg.0.as_mut_ptr(),
            )
        };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if msg.0[1] != RESPONSE_OK {
            return Err(std::io::Error::other(format!(
                "firmware refused SET_BACKLIGHT (0x{:08x})",
                msg.0[1]
            )));
        }
        Ok(())
    }
}

pub(crate) fn percent_of(raw: u32, max: u32) -> u8 {
    (raw * 100 / max.max(1)).min(100) as u8
}

pub(crate) fn read_u32(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// a fake system: which of vcio / firmware touch / a sysfs
    /// backlight exist
    fn tree(name: &str, vcio: bool, touch: bool, sysfs: bool) -> (Tree, Roots) {
        let root = std::env::temp_dir().join(format!("mestro-bl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("backlight")).unwrap();
        if vcio {
            fs::write(root.join("vcio"), "").unwrap();
        }
        if touch {
            fs::create_dir_all(root.join("raspberrypi-ts")).unwrap();
        }
        if sysfs {
            fs::create_dir_all(root.join("backlight/panel0")).unwrap();
            fs::write(root.join("backlight/panel0/brightness"), "255").unwrap();
        }
        let roots = Roots {
            rpi_vcio: root.join("vcio"),
            rpi_fw_touch: root.join("raspberrypi-ts"),
            sysfs_backlight: root.join("backlight"),
        };
        (Tree(root), roots)
    }

    #[test]
    fn pi_with_firmware_touch_uses_the_firmware() {
        let (_t, roots) = tree("fw", true, true, true);
        assert_eq!(decide(BacklightChoice::Auto, &roots), Detected::PiFirmware);
    }

    #[test]
    fn pi_without_firmware_touch_keeps_sysfs() {
        // the kernel owns the bus (touch via edt-ft5x06): no collision
        let (_t, roots) = tree("kernel", true, false, true);
        assert!(matches!(
            decide(BacklightChoice::Auto, &roots),
            Detected::Sysfs(_)
        ));
    }

    #[test]
    fn laptop_uses_sysfs_and_vm_has_none() {
        let (_t, laptop) = tree("laptop", false, false, true);
        assert!(matches!(
            decide(BacklightChoice::Auto, &laptop),
            Detected::Sysfs(_)
        ));
        let (_t2, vm) = tree("vm", false, false, false);
        assert_eq!(decide(BacklightChoice::Auto, &vm), Detected::None);
    }

    #[test]
    fn config_overrides_detection() {
        let (_t, roots) = tree("override", true, true, true);
        assert!(matches!(
            decide(BacklightChoice::Sysfs, &roots),
            Detected::Sysfs(_)
        ));
        assert_eq!(decide(BacklightChoice::None, &roots), Detected::None);
        let (_t2, no_fw) = tree("nofw", false, false, true);
        assert_eq!(decide(BacklightChoice::Firmware, &no_fw), Detected::None);
    }
}
