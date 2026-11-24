//! Custom auto-refresh intervals typed by the owner: `45s`, `2m30s`, `1h30m`, or plain minutes.

use std::ops::RangeInclusive;

/// Allowed auto-refresh intervals in seconds: 5 s to 24 h.
pub const INTERVAL_SECONDS: RangeInclusive<u32> = 5..=86_400;
const SECONDS_PER_MINUTE: u64 = 60;

const FORMAT_HINT: &str = "Type a time like 45s, 2m30s or 1h30m (a plain number is minutes).";
const RANGE_HINT: &str = "Choose between 5s and 24h.";

/// Units in the only order they may appear, with their length in seconds.
const UNITS: [(char, u64); 3] = [('h', 3600), ('m', 60), ('s', 1)];

/// Seconds for `input`, or a message to show the owner; spaces between parts and any case are fine.
pub fn parse_interval(input: &str) -> Result<u32, String> {
    let text = input.trim().to_ascii_lowercase();
    if text.is_empty() {
        return Err(FORMAT_HINT.to_owned());
    }
    let seconds = if text.bytes().all(|byte| byte.is_ascii_digit()) {
        number(&text)?.checked_mul(SECONDS_PER_MINUTE)
    } else {
        sum_of_parts(&text)?
    };
    seconds
        .and_then(|seconds| u32::try_from(seconds).ok())
        .filter(|seconds| INTERVAL_SECONDS.contains(seconds))
        .ok_or_else(|| RANGE_HINT.to_owned())
}

/// `5400` → `1h30m`: each non-zero unit, largest first.
pub fn format_interval(seconds: u32) -> String {
    let mut rest = u64::from(seconds);
    let mut text = String::new();
    for (unit, length) in UNITS {
        let count = rest / length;
        rest %= length;
        if count > 0 {
            text.push_str(&format!("{count}{unit}"));
        }
    }
    if text.is_empty() {
        "0s".to_owned()
    } else {
        text
    }
}

/// `<digits><unit>` parts with units in h, m, s order, each at most once; `None` on overflow.
fn sum_of_parts(text: &str) -> Result<Option<u64>, String> {
    let mut total: u64 = 0;
    let mut next_unit = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| FORMAT_HINT.to_owned())?;
        let (digits, after) = rest.split_at(digits_end);
        let unit = after.chars().next().ok_or_else(|| FORMAT_HINT.to_owned())?;
        let offset = UNITS[next_unit..]
            .iter()
            .position(|&(name, _)| name == unit)
            .ok_or_else(|| FORMAT_HINT.to_owned())?;
        let length = UNITS[next_unit + offset].1;
        next_unit += offset + 1;
        let Some(part) = number(digits)?.checked_mul(length) else {
            return Ok(None);
        };
        let Some(sum) = total.checked_add(part) else {
            return Ok(None);
        };
        total = sum;
        rest = after[unit.len_utf8()..].trim_start();
    }
    Ok(Some(total))
}

/// A run of ASCII digits; empty or too long for `u64` is an error.
fn number(digits: &str) -> Result<u64, String> {
    if digits.is_empty() {
        return Err(FORMAT_HINT.to_owned());
    }
    digits.parse().map_err(|_| RANGE_HINT.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interval_cases() {
        let valid = [
            ("45s", 45),
            ("2m30s", 150),
            ("1h30m", 5400),
            ("90", 5400),
            (" 1H 30M ", 5400),
            ("1m 30s", 90),
            ("1m90s", 150),
            ("5s", 5),
            ("24h", 86_400),
            ("1440", 86_400),
        ];
        for (input, seconds) in valid {
            assert_eq!(parse_interval(input), Ok(seconds), "{input:?}");
        }
        let invalid = [
            "4s",
            "25h",
            "",
            "1h1h",
            "abc",
            "0",
            "0s",
            "3s",
            "24h1s",
            "1441",
            "30m1h",
            "1 30",
            "+5m",
            "-5m",
            "1.5h",
            "1h30",
            "h",
            "99999999999999999999999s",
            "9999999999999999999h",
        ];
        for input in invalid {
            assert!(parse_interval(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn format_interval_cases() {
        assert_eq!(format_interval(5400), "1h30m");
        assert_eq!(format_interval(45), "45s");
        assert_eq!(format_interval(150), "2m30s");
        assert_eq!(format_interval(3601), "1h1s");
        assert_eq!(format_interval(86_400), "24h");
        assert_eq!(format_interval(60), "1m");
    }

    #[test]
    fn format_then_parse_roundtrips() {
        for seconds in 5..=86_400 {
            assert_eq!(parse_interval(&format_interval(seconds)), Ok(seconds));
        }
    }
}
