//! The pure half of a self-update: reading the checksum list, comparing digests and naming the
//! files the swap uses.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::check::EXE_ASSET;

/// The downloaded exe waits here, next to the running one so the final rename stays on one volume.
pub fn staged_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".new")
}

/// The running exe is moved here during the swap; the next start deletes it.
pub fn backup_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".old")
}

/// `<exe>.new`, not `with_extension`, which would replace `.exe`.
fn with_suffix(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(exe);
    name.push(suffix);
    PathBuf::from(name)
}

/// The expected SHA-256 (lower-case hex) of `EXE_ASSET` from `sha256sum` output: lines of
/// `<hex>  <name>` or `<hex> *<name>` (binary marker). `None` when no line names the file or its
/// digest is not 64 hex digits.
pub fn expected_digest(sums: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (digest, name) = line.trim().split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        let is_digest = digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
        (name == EXE_ASSET && is_digest).then(|| digest.to_ascii_lowercase())
    })
}

/// Whether `actual` (raw digest bytes) is the digest `expected` names, in any letter case.
pub fn digest_matches(expected: &str, actual: &[u8]) -> bool {
    let hex: String = actual.iter().map(|byte| format!("{byte:02x}")).collect();
    hex.eq_ignore_ascii_case(expected)
}

/// Whether `url` is HTTPS on GitHub or one of its content hosts, where release downloads
/// redirect to; checked on the final URL of every request.
pub fn is_github_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let is_plain_name = host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    is_plain_name
        && (host == "github.com"
            || host == "api.github.com"
            || host
                .strip_suffix(".githubusercontent.com")
                .is_some_and(|label| !label.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn staged_and_backup_names_append_to_the_whole_file_name() {
        let exe = Path::new(r"C:\Apps\juicetray-windows-x86_64.exe");
        assert_eq!(
            staged_path(exe),
            Path::new(r"C:\Apps\juicetray-windows-x86_64.exe.new")
        );
        assert_eq!(
            backup_path(exe),
            Path::new(r"C:\Apps\juicetray-windows-x86_64.exe.old")
        );
    }

    #[test]
    fn digest_line_is_found_with_text_or_binary_marker() {
        let other = "f".repeat(64);
        for line in [
            format!("{DIGEST}  {EXE_ASSET}"),
            format!("{DIGEST} *{EXE_ASSET}"),
            format!("{DIGEST} {EXE_ASSET}\r"),
        ] {
            let sums = format!("{other}  other.bin\n{line}\n");
            assert_eq!(expected_digest(&sums).as_deref(), Some(DIGEST), "{line:?}");
        }
    }

    #[test]
    fn digest_hex_is_case_insensitive() {
        let upper = DIGEST.to_ascii_uppercase();
        let sums = format!("{upper}  {EXE_ASSET}\n");
        assert_eq!(expected_digest(&sums).as_deref(), Some(DIGEST));
    }

    #[test]
    fn a_bad_or_missing_line_gives_no_digest() {
        assert_eq!(expected_digest(""), None);
        assert_eq!(expected_digest(&format!("{DIGEST}  other.exe\n")), None);
        // Only an exact file name matches.
        assert_eq!(expected_digest(&format!("{DIGEST}  x{EXE_ASSET}\n")), None);
        assert_eq!(
            expected_digest(&format!("{DIGEST}  {EXE_ASSET}.sig\n")),
            None
        );
        // A short or non-hex digest is rejected.
        assert_eq!(expected_digest(&format!("abc123  {EXE_ASSET}\n")), None);
        let non_hex = "g".repeat(64);
        assert_eq!(expected_digest(&format!("{non_hex}  {EXE_ASSET}\n")), None);
        assert_eq!(expected_digest(EXE_ASSET), None);
    }

    #[test]
    fn only_https_github_hosts_are_trusted() {
        for url in [
            "https://github.com/ittmchien/juicetray/releases/download/v1/x.exe",
            "https://api.github.com/repos/a/b",
            "https://objects.githubusercontent.com/github-production-release-asset/1?sig=2",
            "https://release-assets.githubusercontent.com/x",
        ] {
            assert!(is_github_url(url), "{url}");
        }
        for url in [
            "http://github.com/x",
            "https://evil.example/github.com/x",
            "https://github.com.evil.example/x",
            "https://evilgithub.com/x",
            "https://githubusercontent.com/x",
            "https://.githubusercontent.com/x",
            "https://github.com@evil.example/x",
            "https://evil.example#.githubusercontent.com/x",
            "https://evil.example?.githubusercontent.com/x",
            "https://github.com:444/x",
            "file:///c:/windows/calc.exe",
            "",
        ] {
            assert!(!is_github_url(url), "{url}");
        }
    }

    #[test]
    fn digests_compare_by_value_in_any_case() {
        let bytes: Vec<u8> = (0..32).map(|i| (i * 8 + 1) as u8).collect();
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert!(digest_matches(&hex, &bytes));
        assert!(digest_matches(&hex.to_ascii_uppercase(), &bytes));
        let mut tampered = bytes.clone();
        tampered[31] ^= 1;
        assert!(!digest_matches(&hex, &tampered));
        assert!(!digest_matches(&hex, &bytes[..31]));
        assert!(!digest_matches("", &bytes));
    }
}
