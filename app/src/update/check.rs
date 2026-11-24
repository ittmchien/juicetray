//! The pure half of an update check: reading GitHub's reply and comparing versions.

use super::RELEASES_URL;

/// A release page on this project's own repository is the only link the app will open.
const RELEASE_PAGE_PREFIX: &str = "https://github.com/ittmchien/juicetray/";

/// A newer release: its version without the leading `v`, and the page to download it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate,
    Available(UpdateInfo),
}

/// `major.minor.patch` plus whether it is a final release; field order makes the derived
/// ordering right, and a `-pre` version sorts below the plain one.
/// shortcut: two different pre-releases of one version compare equal; upgrade if the app ever
/// ships pre-release builds that must be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    is_release: bool,
}

impl Version {
    /// `1.2.3`, `v1.2.3`, `1.2.3-rc1` or `1.2.3+build`; anything else is `None`.
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let (core, suffix) = text.split_at(text.find(['-', '+']).unwrap_or(text.len()));
        let mut numbers = core.split('.').map(|part| {
            let is_number = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
            is_number.then(|| part.parse::<u64>().ok()).flatten()
        });
        let version = Self {
            major: numbers.next()??,
            minor: numbers.next()??,
            patch: numbers.next()??,
            is_release: !suffix.starts_with('-'),
        };
        numbers.next().is_none().then_some(version)
    }
}

/// Turns GitHub's answer into a verdict; a missing release (404) means nothing newer exists.
pub fn evaluate(status: u16, body: &str, current: &str) -> Result<UpdateCheck, String> {
    match status {
        404 => return Ok(UpdateCheck::UpToDate),
        200 => {}
        other => return Err(format!("GitHub answered HTTP {other}")),
    }
    let tag = json_string_field(body, "tag_name").ok_or("the reply has no tag_name")?;
    let latest = Version::parse(&tag).ok_or_else(|| format!("unrecognised version \"{tag}\""))?;
    let running =
        Version::parse(current).ok_or_else(|| format!("unrecognised own version \"{current}\""))?;
    if latest <= running {
        return Ok(UpdateCheck::UpToDate);
    }
    let url = json_string_field(body, "html_url")
        .filter(|url| url.starts_with(RELEASE_PAGE_PREFIX))
        .unwrap_or_else(|| RELEASES_URL.to_owned());
    Ok(UpdateCheck::Available(UpdateInfo {
        version: tag.trim().trim_start_matches(['v', 'V']).to_owned(),
        url,
    }))
}

/// True the first time `found` is seen: the owner is told once per version.
pub fn should_notify(last_notified: Option<&str>, found: &str) -> bool {
    last_notified != Some(found)
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// The string value of `key` among the top-level members of a JSON object; nested objects
/// (such as the release's `author`) are skipped.
/// shortcut: `\u` escapes decode one UTF-16 unit, so a surrogate pair becomes two U+FFFD;
/// upgrade if a field that may hold astral characters is ever read.
fn json_string_field(json: &str, key: &str) -> Option<String> {
    let mut chars = json.chars().peekable();
    let mut depth = 0_u32;
    while let Some(c) = chars.next() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => depth = depth.saturating_sub(1),
            '"' => {
                let text = read_string(&mut chars)?;
                if depth != 1 || text != key {
                    continue;
                }
                skip_whitespace(&mut chars);
                if chars.next_if_eq(&':').is_none() {
                    continue;
                }
                skip_whitespace(&mut chars);
                if chars.next_if_eq(&'"').is_some() {
                    return read_string(&mut chars);
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_whitespace(chars: &mut Chars) {
    while chars.next_if(|c| c.is_whitespace()).is_some() {}
}

/// The rest of a string whose opening quote is consumed, escapes decoded; `None` if unterminated.
fn read_string(chars: &mut Chars) -> Option<String> {
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => out.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    u32::from_str_radix(&hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .unwrap_or('\u{FFFD}')
                }
                // `"`, `\`, `/` and anything unknown stand for themselves.
                other => other,
            }),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: &str = r#"{"url":"https://api.github.com/x","html_url":"https://github.com/ittmchien/juicetray/releases/tag/v0.2.0","id":1,"author":{"login":"a","html_url":"https://github.com/a"},"tag_name":"v0.2.0","assets":[{"name":"x","html_url":"nested"}]}"#;

    fn check(status: u16, body: &str) -> Result<UpdateCheck, String> {
        evaluate(status, body, "0.1.0")
    }

    #[test]
    fn versions_compare_numerically() {
        let v = |text| Version::parse(text).unwrap();
        assert!(v("0.1.1") > v("0.1.0"));
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3+build5"), v("1.2.3"));
    }

    #[test]
    fn a_pre_release_is_older_than_its_release() {
        let v = |text| Version::parse(text).unwrap();
        assert!(v("1.0.0-rc1") < v("1.0.0"));
        assert!(v("1.0.0") > v("1.0.0-rc1"));
        assert!(v("1.0.0-rc1") > v("0.9.9"));
    }

    #[test]
    fn malformed_versions_are_rejected() {
        for text in [
            "", "latest", "1.0", "1.0.0.0", "1.x.0", "1..0", "-1.0.0", "v", "1.0.+2",
        ] {
            assert!(Version::parse(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn extracts_top_level_strings_only() {
        assert_eq!(
            json_string_field(NORMAL, "tag_name").as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(
            json_string_field(NORMAL, "html_url").as_deref(),
            Some("https://github.com/ittmchien/juicetray/releases/tag/v0.2.0")
        );
        // A nested object's field is not the release's own.
        let nested_only = r#"{"author":{"html_url":"https://github.com/a"}}"#;
        assert_eq!(json_string_field(nested_only, "html_url"), None);
    }

    #[test]
    fn extraction_handles_missing_and_odd_fields() {
        assert_eq!(json_string_field("{}", "tag_name"), None);
        assert_eq!(json_string_field("", "tag_name"), None);
        assert_eq!(json_string_field(r#"{"tag_name":null}"#, "tag_name"), None);
        assert_eq!(json_string_field(r#"{"tag_name":1}"#, "tag_name"), None);
        assert_eq!(json_string_field(r#"{"tag_name":"v1"#, "tag_name"), None);
        // The key's text inside a value, or braces inside a string, change nothing.
        let tricky = r#"{"body":"tag_name } ] {","tag_name" : "v2"}"#;
        assert_eq!(json_string_field(tricky, "tag_name").as_deref(), Some("v2"));
        let as_value = r#"{"name":"tag_name","tag_name":"v3"}"#;
        assert_eq!(
            json_string_field(as_value, "tag_name").as_deref(),
            Some("v3")
        );
    }

    #[test]
    fn extraction_decodes_escapes() {
        let json = r#"{"tag_name":"v1\u0041\n\"q\"\\\/","html_url":"a\/b"}"#;
        assert_eq!(
            json_string_field(json, "tag_name").as_deref(),
            Some("v1A\n\"q\"\\/")
        );
        assert_eq!(json_string_field(json, "html_url").as_deref(), Some("a/b"));
        // A bad escape never panics.
        assert_eq!(
            json_string_field(r#"{"tag_name":"\uZZZZ"}"#, "tag_name").as_deref(),
            Some("\u{FFFD}")
        );
    }

    #[test]
    fn newer_release_is_available_with_its_page() {
        assert_eq!(
            check(200, NORMAL),
            Ok(UpdateCheck::Available(UpdateInfo {
                version: "0.2.0".into(),
                url: "https://github.com/ittmchien/juicetray/releases/tag/v0.2.0".into(),
            }))
        );
    }

    #[test]
    fn same_older_or_missing_release_is_up_to_date() {
        let tag = |tag: &str| format!(r#"{{"tag_name":"{tag}"}}"#);
        assert_eq!(check(200, &tag("v0.1.0")), Ok(UpdateCheck::UpToDate));
        assert_eq!(check(200, &tag("0.0.9")), Ok(UpdateCheck::UpToDate));
        assert_eq!(check(200, &tag("v0.1.0-rc1")), Ok(UpdateCheck::UpToDate));
        assert_eq!(check(404, "{}"), Ok(UpdateCheck::UpToDate));
        // Running a pre-release, the final release of that version is newer.
        assert!(matches!(
            evaluate(200, &tag("0.2.0"), "0.2.0-dev"),
            Ok(UpdateCheck::Available(_))
        ));
    }

    #[test]
    fn foreign_or_missing_release_link_falls_back_to_the_releases_page() {
        let body = |url: &str| format!(r#"{{"tag_name":"v0.2.0","html_url":"{url}"}}"#);
        for url in ["https://evil.example/x", "file:///c:/windows/calc.exe", ""] {
            let Ok(UpdateCheck::Available(info)) = check(200, &body(url)) else {
                panic!("expected an update for {url:?}");
            };
            assert_eq!(info.url, RELEASES_URL);
        }
        let Ok(UpdateCheck::Available(info)) = check(200, r#"{"tag_name":"v0.2.0"}"#) else {
            panic!("expected an update");
        };
        assert_eq!(info.url, RELEASES_URL);
    }

    #[test]
    fn bad_replies_are_errors() {
        assert_eq!(check(403, ""), Err("GitHub answered HTTP 403".into()));
        assert_eq!(check(500, "{}"), Err("GitHub answered HTTP 500".into()));
        assert!(check(200, "{}").unwrap_err().contains("tag_name"));
        assert!(
            check(200, r#"{"tag_name":"nightly"}"#)
                .unwrap_err()
                .contains("nightly")
        );
    }

    #[test]
    fn a_version_is_announced_once() {
        assert!(should_notify(None, "0.2.0"));
        assert!(!should_notify(Some("0.2.0"), "0.2.0"));
        // A still newer release is news again.
        assert!(should_notify(Some("0.2.0"), "0.3.0"));
    }
}
