// SPDX-License-Identifier: AGPL-3.0-or-later
//! Private files and directories (`threat-model.md` AUTH-7, TERM-9).
//!
//! - Directories are created with mode 0700.
//! - Files are created with mode 0600 and `O_NOFOLLOW`; replacements are written to a fresh
//!   temporary name with `O_EXCL`, synced, then renamed over the target atomically.
//! - [`read_private`] refuses symlinks and non-regular files and, in strict mode, any group or
//!   world permission bit (for secrets such as the session file fallback).
//!
//! On Windows there are no mode bits: files live under the per-user profile, whose ACL excludes
//! other users, and symlinks are refused by a metadata check before opening.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Creates `dir` and any missing parents. New directories get mode 0700 on unix, and `dir`
/// itself is tightened to 0700 if it already exists with group or world bits.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(dir)?;
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Open options for a private file: mode 0600 for new files, `O_NOFOLLOW` and `O_CLOEXEC`.
pub fn private_open_options() -> OpenOptions {
    #[allow(unused_mut)]
    let mut opts = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    opts
}

/// Refuses a symlink at `path` on platforms without `O_NOFOLLOW` (a best-effort check).
fn refuse_symlink_fallback(path: &Path) -> io::Result<()> {
    #[cfg(not(unix))]
    {
        if let Ok(meta) = fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("refusing to follow a symlink at {}", path.display()),
                ));
            }
        }
    }
    let _ = path;
    Ok(())
}

/// Opens (creating if needed, mode 0600) a private file for reading and writing without
/// following a symlink at its final component. Used for SQLite databases and lock files.
pub fn open_or_create_private(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    refuse_symlink_fallback(path)?;
    let file = private_open_options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| symlink_hint(e, path))?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    Ok(file)
}

/// A fresh, unused temporary path next to `target` (same directory, so a rename is atomic).
pub fn temp_path_for(target: &Path) -> PathBuf {
    let mut rnd = [0u8; 8];
    // A failure leaves zeros; `O_EXCL` still prevents clobbering an existing file.
    let _ = getrandom::fill(&mut rnd);
    let suffix: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.{suffix}.tmp"))
}

/// Creates a new private file at `path` with `O_EXCL` (fails if anything exists there).
pub fn create_new_private(path: &Path) -> io::Result<File> {
    private_open_options()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Writes `bytes` to `path` atomically: a 0600 temporary file created with `O_EXCL` and
/// `O_NOFOLLOW` in the same 0700 directory, `fsync`, then `rename` over the target.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure_private_dir(dir)?;
    let tmp = temp_path_for(path);
    let result = (|| {
        let mut file = create_new_private(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        // Persist the rename itself (best effort; not supported everywhere).
        #[cfg(unix)]
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Reads a whole private file of at most `max_bytes`, without following a symlink at its final
/// component. Returns `Ok(None)` if it does not exist. With `strict`, a file whose mode grants
/// any group or world permission is refused (unix).
pub fn read_private(path: &Path, max_bytes: u64, strict: bool) -> io::Result<Option<Vec<u8>>> {
    refuse_symlink_fallback(path)?;
    let file = match private_open_options().read(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(symlink_hint(e, path)),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    #[cfg(unix)]
    if strict {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} is accessible by other users (expected mode 0600)",
                    path.display()
                ),
            ));
        }
    }
    let _ = strict;
    if meta.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is larger than {max_bytes} bytes", path.display()),
        ));
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.take(max_bytes + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is larger than {max_bytes} bytes", path.display()),
        ));
    }
    Ok(Some(buf))
}

/// `O_NOFOLLOW` reports a symlink as `ELOOP`; say what happened.
fn symlink_hint(e: io::Error, path: &Path) -> io::Error {
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ELOOP) {
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing to follow a symlink at {}", path.display()),
        );
    }
    let _ = path;
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/secret.json");
        write_private_atomic(&p, b"one").unwrap();
        write_private_atomic(&p, b"two").unwrap();
        assert_eq!(read_private(&p, 10, true).unwrap().unwrap(), b"two");
        assert!(
            read_private(&dir.path().join("missing"), 10, true)
                .unwrap()
                .is_none()
        );
        assert!(read_private(&p, 2, true).is_err(), "size cap");
        // No temporary files are left behind.
        let names: Vec<_> = fs::read_dir(p.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(p.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_and_lax_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        write_private_atomic(&real, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = read_private(&link, 10, false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(open_or_create_private(&link).is_err());

        fs::set_permissions(&real, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_private(&real, 10, true).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(read_private(&real, 10, false).unwrap().is_some());
    }
}
