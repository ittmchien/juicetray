//! The self-update itself, run on the offer's own thread: download the release's exe and
//! checksum list, verify the SHA-256, swap the running exe for the new one and start it.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};

use super::check::UpdateAssets;
use super::install::{backup_path, digest_matches, expected_digest, running_exe, staged_path};
use super::windows::get;

/// A release exe is a few MB; anything near this is not Juicetray.
const MAX_EXE_BYTES: usize = 50 * 1024 * 1024;
const MAX_SUMS_BYTES: usize = 64 * 1024;

/// A scanner may hold a file it just saw written or renamed; each rename gets this many tries.
const RENAME_TRIES: u32 = 5;
const RENAME_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Why an install stopped. Reasons name files by file name only: they reach the log and dialogs,
/// and a full path would expose the user's profile folder.
#[derive(Debug)]
pub enum Failure {
    /// The exe's folder refuses new files (Program Files, a protected share): updating by hand
    /// is the way. The current exe is untouched.
    ReadOnlyFolder(String),
    /// The current exe is still in place and runnable.
    Other(String),
    /// The swap failed and putting the old exe back failed too; it is kept as `<exe>.old`.
    RestoreFailed(String),
}

fn other(reason: impl Into<String>) -> Failure {
    Failure::Other(reason.into())
}

/// Replaces the running exe with the release's and starts the new one with
/// `--updated-from <from_version>`; the caller then exits this process. On any error the staged
/// file is deleted and the exe path holds the old exe again (unless `RestoreFailed`).
pub fn install(assets: &UpdateAssets, from_version: &str) -> Result<(), Failure> {
    let exe =
        running_exe().map_err(|error| other(format!("finding the running exe failed: {error}")))?;
    let staged = staged_path(&exe);
    let result = stage_and_swap(&exe, &staged, assets, from_version);
    if result.is_err() {
        // Best effort: a leftover `.new` is replaced by the next attempt anyway.
        let _ = fs::remove_file(&staged);
    }
    result
}

fn stage_and_swap(
    exe: &Path,
    staged: &Path,
    assets: &UpdateAssets,
    from_version: &str,
) -> Result<(), Failure> {
    // Creating the staged file first is the write probe, before anything is downloaded.
    let mut file = create_staged(staged)?;

    let sums = download(&assets.sums_url, MAX_SUMS_BYTES)?;
    let expected = expected_digest(&String::from_utf8_lossy(&sums))
        .ok_or_else(|| other("the release's SHA256SUMS.txt has no valid line for the exe"))?;
    let bytes = download(&assets.exe_url, MAX_EXE_BYTES)?;
    let actual = sha256(&bytes).map_err(other)?;
    if !digest_matches(&expected, &actual) {
        return Err(other(
            "the download does not match the SHA-256 in SHA256SUMS.txt",
        ));
    }

    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| other(format!("writing {} failed: {error}", file_name(staged))))?;
    // The exe must be closed before it can be renamed or run.
    drop(file);

    swap(exe, staged, &backup_path(exe), |new_exe| {
        Command::new(new_exe)
            .arg(super::UPDATED_FLAG)
            .arg(from_version)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    })
}

/// Creates the staged file. A permission error may come from a stale read-only or locked `.new`
/// rather than the folder, so a leftover is removed and the create retried before the folder is
/// blamed.
fn create_staged(staged: &Path) -> Result<File, Failure> {
    let error = match File::create(staged) {
        Ok(file) => return Ok(file),
        Err(error) => error,
    };
    let reason = format!("creating {} failed: {error}", file_name(staged));
    if error.kind() != ErrorKind::PermissionDenied {
        return Err(other(reason));
    }
    if staged.exists() {
        fs::remove_file(staged).map_err(|remove_error| {
            other(format!(
                "removing the leftover {} failed: {remove_error}",
                file_name(staged)
            ))
        })?;
        match File::create(staged) {
            Ok(file) => return Ok(file),
            Err(retry_error) if retry_error.kind() != ErrorKind::PermissionDenied => {
                return Err(other(format!(
                    "creating {} failed: {retry_error}",
                    file_name(staged)
                )));
            }
            Err(_) => {}
        }
    }
    Err(Failure::ReadOnlyFolder(reason))
}

/// Moves the running exe to `backup`, puts `staged` in its place and runs `launch` on it. Any
/// failure after the first move puts the old exe back with one replacing rename, which also
/// overwrites a new exe that is still there and succeeds when a scanner already deleted it.
fn swap(
    exe: &Path,
    staged: &Path,
    backup: &Path,
    launch: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), Failure> {
    // Renaming a running image is allowed; `rename` replaces a stale `.old`.
    rename_retry(exe, backup)
        .map_err(|error| other(format!("moving the running exe aside failed: {error}")))?;
    if let Err(error) = rename_retry(staged, exe) {
        return Err(restore(
            format!("moving the new exe into place failed: {error}"),
            exe,
            backup,
        ));
    }
    if let Err(error) = launch(exe) {
        return Err(restore(
            format!("starting the new version failed: {error}"),
            exe,
            backup,
        ));
    }
    Ok(())
}

/// Puts the old exe back at `exe`; `reason` is why the swap failed.
fn restore(reason: String, exe: &Path, backup: &Path) -> Failure {
    match rename_retry(backup, exe) {
        Ok(()) => Failure::Other(reason),
        Err(error) => Failure::RestoreFailed(format!(
            "{reason}; restoring the old exe failed ({error}), it is kept as {}",
            file_name(backup)
        )),
    }
}

/// `fs::rename` that tries again after a short pause, except when the source is missing.
fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match fs::rename(from, to) {
            Err(error) if error.kind() != ErrorKind::NotFound && attempt < RENAME_TRIES => {
                attempt += 1;
                thread::sleep(RENAME_RETRY_DELAY);
            }
            result => return result,
        }
    }
}

/// The last path component; full paths never go into reasons, logs or dialogs.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The body of a 200 reply for `url`.
fn download(url: &str, max_bytes: usize) -> Result<Vec<u8>, Failure> {
    let name = url.rsplit('/').next().unwrap_or(url);
    let reply = get(url, None, max_bytes)
        .map_err(|error| other(format!("downloading {name} failed: {error}")))?;
    if reply.status != 200 {
        return Err(other(format!(
            "downloading {name} failed: HTTP {}",
            reply.status
        )));
    }
    if reply.truncated {
        return Err(other(format!(
            "{name} is larger than {} MB",
            max_bytes.div_ceil(1024 * 1024)
        )));
    }
    Ok(reply.body)
}

/// SHA-256 of `data` through CNG.
fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    let mut digest = [0_u8; 32];
    // SAFETY: the pseudo handle needs no open or close; the output buffer is exactly the
    // 32-byte SHA-256 digest size and there is no HMAC secret.
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut digest) };
    if status.is_ok() {
        Ok(digest)
    } else {
        Err(format!("hashing the download failed: {status:?}"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A scratch folder holding `app.exe` ("old") and `app.exe.new` ("new"); removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("juicetray-swap-{}-{test}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("app.exe"), "old").unwrap();
            fs::write(dir.join("app.exe.new"), "new").unwrap();
            Self(dir)
        }

        fn exe(&self) -> PathBuf {
            self.0.join("app.exe")
        }

        fn staged(&self) -> PathBuf {
            self.0.join("app.exe.new")
        }

        fn backup(&self) -> PathBuf {
            self.0.join("app.exe.old")
        }

        fn exe_text(&self) -> String {
            fs::read_to_string(self.exe()).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_good_swap_installs_the_new_exe_and_keeps_the_old_as_backup() {
        let dir = Scratch::new("ok");
        swap(&dir.exe(), &dir.staged(), &dir.backup(), |new_exe| {
            assert_eq!(fs::read_to_string(new_exe).unwrap(), "new");
            Ok(())
        })
        .unwrap();
        assert_eq!(dir.exe_text(), "new");
        assert_eq!(fs::read_to_string(dir.backup()).unwrap(), "old");
    }

    #[test]
    fn a_failed_first_rename_leaves_the_old_exe_alone() {
        let dir = Scratch::new("first");
        let unreachable_backup = dir.0.join("missing-folder").join("app.exe.old");
        let result = swap(&dir.exe(), &dir.staged(), &unreachable_backup, |_| Ok(()));
        assert!(matches!(result, Err(Failure::Other(_))));
        assert_eq!(dir.exe_text(), "old");
        assert!(dir.staged().exists());
    }

    #[test]
    fn a_failed_second_rename_restores_the_old_exe() {
        let dir = Scratch::new("second");
        fs::remove_file(dir.staged()).unwrap();
        let result = swap(&dir.exe(), &dir.staged(), &dir.backup(), |_| Ok(()));
        assert!(matches!(result, Err(Failure::Other(_))));
        assert_eq!(dir.exe_text(), "old");
        assert!(!dir.backup().exists());
    }

    #[test]
    fn a_failed_launch_restores_the_old_exe_over_the_new_one() {
        let dir = Scratch::new("launch");
        let result = swap(&dir.exe(), &dir.staged(), &dir.backup(), |_| {
            Err(io::Error::other("no start"))
        });
        assert!(matches!(result, Err(Failure::Other(_))));
        assert_eq!(dir.exe_text(), "old");
        assert!(!dir.staged().exists());
    }

    #[test]
    fn a_new_exe_deleted_before_launch_still_restores_the_old_exe() {
        let dir = Scratch::new("deleted");
        let result = swap(&dir.exe(), &dir.staged(), &dir.backup(), |new_exe| {
            fs::remove_file(new_exe).unwrap();
            Err(io::Error::other("file not found"))
        });
        assert!(matches!(result, Err(Failure::Other(_))));
        assert_eq!(dir.exe_text(), "old");
    }

    #[test]
    fn a_failed_restore_is_reported_by_file_name_only() {
        let dir = Scratch::new("restore");
        let backup = dir.backup();
        let result = swap(&dir.exe(), &dir.staged(), &backup, |_| {
            fs::remove_file(&backup).unwrap();
            Err(io::Error::other("no start"))
        });
        let Err(Failure::RestoreFailed(reason)) = result else {
            panic!("expected a failed restore");
        };
        assert!(reason.contains("app.exe.old"), "{reason}");
        assert!(!reason.contains(&*dir.0.to_string_lossy()), "{reason}");
    }

    #[test]
    fn a_stale_staged_file_does_not_block_creating_a_new_one() {
        let dir = Scratch::new("stale");
        let mut file = create_staged(&dir.staged()).unwrap();
        file.write_all(b"fresh").unwrap();
        drop(file);
        assert_eq!(fs::read_to_string(dir.staged()).unwrap(), "fresh");
    }

    #[test]
    fn sha256_matches_the_known_digest_of_abc() {
        let digest = sha256(b"abc").unwrap();
        assert!(digest_matches(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            &digest
        ));
        // The empty input hashes too.
        assert!(digest_matches(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            &sha256(b"").unwrap()
        ));
    }
}
