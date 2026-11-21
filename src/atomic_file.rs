//! Atomic file writes: the one place that knows the crash
//! discipline (tmp in the same directory, fsync, rename, directory
//! sync). Used by the wifi confs and the persisted device id.

use std::io::Write as _;
use std::path::Path;

/// the scratch file next to `path`: `<file name>.tmp`
pub(crate) fn tmp_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
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

/// [`write_atomic`] with the directory-sync seam injected — the
/// tests pin the failure discipline
pub(crate) fn write_atomic_with(
    path: &Path,
    text: &str,
    dir_sync: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let tmp = tmp_path(path);
    let mut file =
        std::fs::File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))?;
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

/// fsync the directory a rename happened in — the rename must
/// survive a power cut, not just the file contents
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
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

    #[test]
    fn the_tmp_name_extends_the_file_name() {
        assert_eq!(
            tmp_path(Path::new("/x/wpa_supplicant.conf.bak")),
            Path::new("/x/wpa_supplicant.conf.bak.tmp")
        );
        assert_eq!(
            tmp_path(Path::new("/x/device-id")),
            Path::new("/x/device-id.tmp")
        );
    }
}
