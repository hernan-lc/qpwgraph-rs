//! Crash-safe file writes for the app's persisted state.
//!
//! `std::fs::write` truncates the destination before it writes a byte. A
//! crash, a full disk, or a power cut in that window destroys the user's only
//! copy of their configuration or patchbay — files that can represent a lot of
//! patient routing work. Writing a sibling temporary file and renaming it over
//! the target makes the replacement atomic on every platform this app targets:
//! a reader either sees the whole old file or the whole new one.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

/// Write `contents` to `path` atomically.
///
/// With `private`, the file is created readable and writable only by its
/// owner. Pass it for anything holding a secret: on Unix a plain create leaves
/// the mode to the caller's umask, which commonly means group- and
/// world-readable.
pub fn atomic_write(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_sibling(path);
    // Scope the handle so it is closed before the rename: Windows refuses to
    // rename a file that is still open.
    {
        let mut file = create(&temporary, private)?;
        file.write_all(contents)?;
        // Without the flush-to-disk the rename can land before the data does,
        // which on a crash leaves a correctly named but empty file.
        file.flush()?;
        let _ = file.sync_all();
    }
    match std::fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn temporary_sibling(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    // The process id keeps two instances of the app from colliding on the
    // same temporary while both save.
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(unix)]
fn create(path: &Path, private: bool) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.mode(if private { 0o600 } else { 0o644 });
    options.open(path)
}

#[cfg(not(unix))]
fn create(path: &Path, _private: bool) -> io::Result<File> {
    // Windows has no umask; a file in the user's roaming profile is already
    // protected by the directory's ACL.
    File::create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own. Tests run in parallel and this module
    /// asserts on directory *contents*, so a shared scratch directory would
    /// make them observe each other's in-flight temporaries.
    fn scratch(test: &str, name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pw-graph-utils-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn writes_and_replaces_contents() {
        let path = scratch("replace", "atomic.txt");
        atomic_write(&path, b"first", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        atomic_write(&path, b"second", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn creates_missing_parent_directories() {
        let path = scratch("nested", "deeper/atomic.txt");
        atomic_write(&path, b"value", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "value");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn leaves_no_temporary_behind() {
        let path = scratch("clean", "clean.txt");
        atomic_write(&path, b"value", false).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files were left behind");
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("secret", "secret.txt");
        atomic_write(&path, b"pin", true).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "a secret must not be world-readable");
        let _ = std::fs::remove_file(&path);
    }
}
