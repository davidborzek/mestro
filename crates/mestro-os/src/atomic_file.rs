//! Atomic file writes: the one place that knows the crash
//! discipline (tmp in the same directory, fsync, rename, directory
//! sync). Used by the wifi confs, the persisted device id and the
//! device-local state files (settings overlay, pairing PIN).

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// the scratch file next to `path`: `<file name>.<pid>.<n>.tmp`, unique
/// per call — two writers of one target never share a tmp file
pub(crate) fn tmp_path(path: &Path) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

/// write atomically: tmp in the same directory (rename cannot cross
/// filesystems), fsync before the rename, the directory after —
/// only EINVAL/ENOTSUP are tolerated (vfat refuses the sync);
/// anything else (EIO) fails the write: the rename may never
/// have reached the disk. Creates the directory
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    write_atomic_with(path, text, &sync_dir)
}

/// [`write_atomic`] with the file mode set on the tmp file before
/// the rename: a secret never exists with wider permissions, not
/// even for an instant (the mode is exact — the umask cannot
/// narrow it)
pub fn write_atomic_mode(path: &Path, text: &str, mode: u32) -> Result<(), String> {
    write_atomic_inner(path, text, Some(mode), &sync_dir)
}

/// [`write_atomic`] with the directory-sync seam injected — the
/// wifi conf tests pin the failure discipline
pub fn write_atomic_with(
    path: &Path,
    text: &str,
    dir_sync: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    write_atomic_inner(path, text, None, dir_sync)
}

fn write_atomic_inner(
    path: &Path,
    text: &str,
    mode: Option<u32>,
    dir_sync: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let tmp = tmp_path(path);
    write_tmp(&tmp, text, mode)
        .and_then(|()| {
            std::fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))
        })
        // unique names would pile up if a failed write left its tmp
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
    if let Some(dir) = path.parent() {
        match dir_sync(dir) {
            // vfat has no directory sync — the rename happened
            Ok(()) => {}
            Err(e)
                if e.raw_os_error() == Some(libc::EINVAL)
                    || e.raw_os_error() == Some(libc::ENOTSUP) => {}
            Err(e) => return Err(format!("cannot sync {}: {e}", dir.display())),
        }
    }
    Ok(())
}

/// create the tmp file (created with `mode`, then fchmod'ed to the
/// exact mode — the umask only narrows the create), write and fsync
/// it; the caller renames
fn write_tmp(tmp: &Path, text: &str, mode: Option<u32>) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    if let Some(mode) = mode {
        options.mode(mode);
    }
    let mut file = options
        .open(tmp)
        .map_err(|e| format!("create {}: {e}", tmp.display()))?;
    if let Some(mode) = mode {
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    }
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("write {}: {e}", tmp.display()))
}

/// fsync the directory a rename happened in — the rename must
/// survive a power cut, not just the file contents
pub fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_creates_the_dir_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a").join("b").join("state");
        write_atomic(&file, "one").unwrap();
        write_atomic(&file, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("state")]);
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_tmp_file_has_the_mode_before_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("secret.tmp");
        write_tmp(&tmp, "pin", Some(0o600)).unwrap();
        assert_eq!(mode_of(&tmp), 0o600);
        assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "pin");
    }

    #[test]
    fn write_atomic_mode_sets_the_exact_mode_and_replaces_a_wider_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("web-pin");
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic_mode(&file, "new", 0o600).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(mode_of(&file), 0o600);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_write_removes_the_tmp_and_keeps_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), "x").unwrap();
        assert!(write_atomic_mode(&target, "text", 0o600).is_err());
        assert!(target.join("keep").exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn the_tmp_name_extends_the_file_name_and_is_unique_per_call() {
        let target = Path::new("/x/wpa_supplicant.conf.bak");
        let (first, second) = (tmp_path(target), tmp_path(target));
        assert_ne!(first, second, "two writers must never share a tmp file");
        for tmp in [first, second] {
            assert_eq!(tmp.parent(), Some(Path::new("/x")));
            let name = tmp.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("wpa_supplicant.conf.bak."), "{name}");
            assert!(name.ends_with(".tmp"), "{name}");
        }
    }

    #[test]
    fn concurrent_writers_of_one_target_leave_one_intact_file_and_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("squeezelite.env");
        let texts: Vec<String> = (0..8)
            .map(|n| format!("OUTPUT={n}\n").repeat(500))
            .collect();
        std::thread::scope(|scope| {
            for text in &texts {
                let file = &file;
                scope.spawn(move || {
                    for _ in 0..20 {
                        write_atomic(file, text).unwrap();
                    }
                });
            }
        });
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(texts.contains(&written), "a torn or mixed file");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
