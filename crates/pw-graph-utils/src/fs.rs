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
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

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
    let result = (|| {
        // Scope the handle so it is closed before the rename: Windows refuses
        // to rename a file that is still open.
        let mut file = create(&temporary, private)?;
        file.write_all(contents)?;
        // Without the flush-to-disk the rename can land before the data does,
        // which on a crash leaves a correctly named but empty file.
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        sync_parent_directory(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn temporary_sibling(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    // The process id avoids collisions between app instances; the atomic
    // sequence makes concurrent writes from one process distinct as well.
    let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".{}.{}.tmp", std::process::id(), sequence));
    path.with_file_name(name)
}

#[cfg(unix)]
fn create(path: &Path, private: bool) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    options.mode(if private { 0o600 } else { 0o644 });
    options.open(path)
}

#[cfg(not(unix))]
fn create(path: &Path, _private: bool) -> io::Result<File> {
    // Windows has no umask; a file in the user's roaming profile is already
    // protected by the directory's ACL.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

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

    #[test]
    fn temporary_sibling_names_are_unique_within_one_process() {
        let path = scratch("unique", "same.txt");
        let first = temporary_sibling(&path);
        let second = temporary_sibling(&path);
        assert_ne!(first, second);
    }

    #[test]
    fn concurrent_writes_to_one_target_are_complete_and_clean() {
        let path = scratch("concurrent", "atomic.txt");
        let barrier = Arc::new(Barrier::new(2));
        let first_path = path.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = std::thread::spawn(move || {
            first_barrier.wait();
            atomic_write(&first_path, b"first-complete-value", false).unwrap();
        });
        let second_path = path.clone();
        let second_barrier = Arc::clone(&barrier);
        let second = std::thread::spawn(move || {
            second_barrier.wait();
            atomic_write(&second_path, b"second-complete-value", false).unwrap();
        });
        first.join().unwrap();
        second.join().unwrap();
        let value = std::fs::read(&path).unwrap();
        assert!(
            value == b"first-complete-value" || value == b"second-complete-value",
            "replacement was not one complete write: {value:?}"
        );
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
