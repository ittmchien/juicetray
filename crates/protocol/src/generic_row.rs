//! The `Generic` fields of one catalogue row: checked and turned into a `GenericSpec` literal by
//! build.rs, and checked by the protocol tests. Rows of other families may carry none of them.

/// First usage page of the vendor-defined range; keep in step with `hid.rs`.
const VENDOR_PAGE_MIN: u16 = 0xFF00;
/// Firmware-update (DFU) channel; keep in step with `hid.rs`.
const FIRMWARE_UPDATE_PAGE: u16 = 0x008C;
/// Longest request, report id included.
const MAX_REQUEST_LEN: usize = 64;
/// Longest report size a row may name, as `juicetray scan` prints it.
const MAX_REPORT_BYTES: u8 = 64;
/// Longest input report a reply index may point into (64 data bytes plus the id).
const MAX_REPLY_LEN: usize = 65;
const DEFAULT_SCALE: u8 = 100;

/// The row's Generic values as written in the TOML file; `None` = absent.
#[derive(Debug, Default, Clone)]
pub struct Fields<'a> {
    pub usage_page: Option<&'a str>,
    pub usage: Option<&'a str>,
    pub report: Option<&'a str>,
    pub report_bytes: Option<u8>,
    pub request: Option<&'a str>,
    pub reply_prefix: Option<&'a str>,
    pub level_byte: Option<u8>,
    pub charging_byte: Option<u8>,
    pub charging_mask: Option<u8>,
    pub scale: Option<u8>,
}

impl Fields<'_> {
    fn any(&self) -> bool {
        self.usage_page.is_some()
            || self.usage.is_some()
            || self.report.is_some()
            || self.report_bytes.is_some()
            || self.request.is_some()
            || self.reply_prefix.is_some()
            || self.level_byte.is_some()
            || self.charging_byte.is_some()
            || self.charging_mask.is_some()
            || self.scale.is_some()
    }
}

/// A checked Generic row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub usage_page: u16,
    pub usage: u16,
    pub feature: bool,
    pub report_bytes: u8,
    /// The request zero-padded to the report's length on the wire (report id first).
    pub frame: Vec<u8>,
    pub reply_prefix: Vec<u8>,
    pub level_byte: u8,
    pub charging: Option<(u8, u8)>,
    pub scale: u8,
}

/// The checked Generic row of a `Generic` family line, `None` for any other family; a field
/// missing, malformed or unsafe (page 008C, a non-vendor page, a request longer than the report)
/// is an error, and so is any Generic field on another family.
pub fn check(is_generic: bool, fields: &Fields) -> Result<Option<Row>, String> {
    if !is_generic && fields.any() {
        return Err("Generic fields are allowed on Generic rows only".to_owned());
    }
    if !is_generic {
        return Ok(None);
    }
    let usage_page = hex16(required(fields.usage_page, "usage_page")?, "usage_page")?;
    if usage_page == FIRMWARE_UPDATE_PAGE {
        return Err("usage_page 008C is a firmware-update channel; it is never queried".to_owned());
    }
    if usage_page < VENDOR_PAGE_MIN {
        return Err(format!(
            "usage_page {usage_page:04X} is not vendor-defined (FF00-FFFF)"
        ));
    }
    let usage = hex16(required(fields.usage, "usage")?, "usage")?;
    let feature = match required(fields.report, "report")? {
        "output" => false,
        "feature" => true,
        other => return Err(format!("unknown report {other:?}; output or feature")),
    };
    let report_bytes = required(fields.report_bytes, "report_bytes")?;
    if !(1..=MAX_REPORT_BYTES).contains(&report_bytes) {
        return Err(format!(
            "report_bytes {report_bytes} is not 1-{MAX_REPORT_BYTES}"
        ));
    }
    let request = hex_bytes(required(fields.request, "request")?, "request")?;
    if request.len() > MAX_REQUEST_LEN {
        return Err(format!(
            "request is {} bytes; at most {MAX_REQUEST_LEN}",
            request.len()
        ));
    }
    // The report size counts the id byte only when the id is not 0; the wire always carries it.
    let frame_len = usize::from(report_bytes) + usize::from(request[0] == 0);
    if request.len() > frame_len {
        return Err(format!(
            "request is {} bytes; report {:02x} is {frame_len} bytes with its id",
            request.len(),
            request[0]
        ));
    }
    let reply_prefix = hex_bytes(
        required(fields.reply_prefix, "reply_prefix")?,
        "reply_prefix",
    )?;
    // A GetFeature reply is the same report; an input reply needs a byte past its id to be told
    // apart from other input reports.
    if feature && reply_prefix[0] != request[0] {
        return Err("reply_prefix of a feature report starts with the request's id".to_owned());
    }
    if !feature && reply_prefix.len() < 2 {
        return Err("reply_prefix of an output report needs a byte after the report id".to_owned());
    }
    let reply_len = if feature { frame_len } else { MAX_REPLY_LEN };
    let index = |value: u8, name: &str| {
        let at = usize::from(value);
        if at < reply_prefix.len() || at >= reply_len {
            return Err(format!(
                "{name} {at} is not after reply_prefix and inside the {reply_len}-byte reply"
            ));
        }
        Ok(value)
    };
    let level_byte = index(required(fields.level_byte, "level_byte")?, "level_byte")?;
    let charging = match (fields.charging_byte, fields.charging_mask) {
        (None, None) => None,
        (Some(_), Some(0)) => return Err("charging_mask 0 never reads charging".to_owned()),
        (Some(byte), Some(mask)) => Some((index(byte, "charging_byte")?, mask)),
        _ => return Err("charging_byte and charging_mask go together".to_owned()),
    };
    let scale = fields.scale.unwrap_or(DEFAULT_SCALE);
    if scale == 0 {
        return Err("scale 0; a raw level is divided by it".to_owned());
    }
    let mut frame = request;
    frame.resize(frame_len, 0);
    Ok(Some(Row {
        usage_page,
        usage,
        feature,
        report_bytes,
        frame,
        reply_prefix,
        level_byte,
        charging,
        scale,
    }))
}

/// The `Family::Generic` expression of `row`, for the generated catalogue.
pub fn literal(row: &Row) -> String {
    let report = if row.feature { "Feature" } else { "Output" };
    // `{:?}` of a `Vec<u8>` / `Option<(u8, u8)>` is a valid Rust literal.
    format!(
        "Family::Generic(&crate::GenericSpec {{ usage_page: {:#06X}, usage: {:#06X}, \
         report: crate::GenericReport::{report}, report_bytes: {}, request: &{:?}, \
         reply_prefix: &{:?}, level_byte: {}, charging: {:?}, scale: {} }})",
        row.usage_page,
        row.usage,
        row.report_bytes,
        row.frame,
        row.reply_prefix,
        row.level_byte,
        row.charging,
        row.scale
    )
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("a Generic row needs {name}"))
}

/// Exactly 4 hex digits, any case, no prefix (as `vid` / `pid`).
fn hex16(value: &str, name: &str) -> Result<u16, String> {
    let is_hex = value.len() == 4 && value.chars().all(|c| c.is_ascii_hexdigit());
    is_hex
        .then(|| u16::from_str_radix(value, 16).ok())
        .flatten()
        .ok_or_else(|| format!("{name} {value:?} is not 4 hex digits"))
}

/// Bytes as hex pairs; spaces between bytes are allowed. At least one byte.
fn hex_bytes(value: &str, name: &str) -> Result<Vec<u8>, String> {
    let digits: String = value.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bad = || format!("{name} {value:?} is not hex bytes");
    // Hex digits only: `from_str_radix` alone would also take a sign.
    let all_hex = digits.chars().all(|c| c.is_ascii_hexdigit());
    if digits.is_empty() || !digits.len().is_multiple_of(2) || !all_hex {
        return Err(bad());
    }
    (0..digits.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&digits[at..at + 2], 16).map_err(|_| bad()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Driveall-like output row: report 0 of 64 data bytes, reply `55 10 …` read without id.
    fn output() -> Fields<'static> {
        Fields {
            usage_page: Some("FF60"),
            usage: Some("0061"),
            report: Some("output"),
            report_bytes: Some(64),
            request: Some("00 AA 10 30 00 00 00 01 00"),
            reply_prefix: Some("00 55 10"),
            level_byte: Some(26),
            ..Fields::default()
        }
    }

    fn error(fields: &Fields) -> String {
        check(true, fields).expect_err("rejected")
    }

    #[test]
    fn generic_row_pads_the_request_to_the_report() {
        let row = check(true, &output()).expect("valid").expect("generic");
        assert_eq!(row.frame.len(), 65);
        assert_eq!(&row.frame[..9], &[0x00, 0xAA, 0x10, 0x30, 0, 0, 0, 0x01, 0]);
        assert!(row.frame[9..].iter().all(|&byte| byte == 0));
        assert_eq!(
            (row.usage_page, row.usage, row.feature),
            (0xFF60, 0x0061, false)
        );
        assert_eq!(row.reply_prefix, [0x00, 0x55, 0x10]);
        assert_eq!((row.level_byte, row.charging, row.scale), (26, None, 100));
        // A numbered report counts its id in report_bytes.
        let numbered = Fields {
            report: Some("feature"),
            report_bytes: Some(8),
            request: Some("05f7"),
            reply_prefix: Some("05"),
            level_byte: Some(2),
            charging_byte: Some(3),
            charging_mask: Some(0x80),
            scale: Some(10),
            ..output()
        };
        let row = check(true, &numbered).expect("valid").expect("generic");
        assert_eq!(row.frame, [0x05, 0xF7, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            (row.feature, row.charging, row.scale),
            (true, Some((3, 0x80)), 10)
        );
    }

    #[test]
    fn generic_row_literal_names_the_spec() {
        let row = check(true, &output()).unwrap().unwrap();
        let text = literal(&row);
        assert!(text.starts_with("Family::Generic(&crate::GenericSpec { usage_page: 0xFF60"));
        assert!(text.contains("report: crate::GenericReport::Output, report_bytes: 64"));
        assert!(text.contains("request: &[0, 170, 16, 48, 0, 0, 0, 1, 0, 0,"));
        assert!(text.contains("reply_prefix: &[0, 85, 16], level_byte: 26, charging: None"));
    }

    #[test]
    fn generic_row_rejects_unsafe_pages_and_long_requests() {
        let page = |page| Fields {
            usage_page: Some(page),
            ..output()
        };
        assert!(error(&page("008C")).contains("firmware-update"));
        assert!(error(&page("008c")).contains("firmware-update"));
        assert!(error(&page("0001")).contains("not vendor-defined"));
        assert!(error(&page("000C")).contains("not vendor-defined"));
        // 9 request bytes do not fit report 0 of 4 data bytes (5 with the id).
        let long = Fields {
            report_bytes: Some(4),
            ..output()
        };
        assert!(
            error(&long).contains("5 bytes with its id"),
            "{}",
            error(&long)
        );
        let numbered = Fields {
            request: Some("05 01 02 03"),
            reply_prefix: Some("05 01"),
            level_byte: Some(2),
            report_bytes: Some(3),
            ..output()
        };
        assert!(error(&numbered).contains("3 bytes with its id"));
        let zeros = "00".repeat(65);
        let huge = Fields {
            request: Some(&zeros),
            ..output()
        };
        assert!(error(&huge).contains("at most 64"));
        assert!(
            error(&Fields {
                report_bytes: Some(65),
                ..output()
            })
            .contains("1-64")
        );
    }

    #[test]
    fn generic_row_rejects_missing_and_malformed_fields() {
        assert!(
            error(&Fields {
                request: None,
                ..output()
            })
            .contains("needs request")
        );
        assert!(
            error(&Fields {
                level_byte: None,
                ..output()
            })
            .contains("needs level_byte")
        );
        assert!(
            error(&Fields {
                usage: Some("61"),
                ..output()
            })
            .contains("4 hex digits")
        );
        assert!(
            error(&Fields {
                report: Some("input"),
                ..output()
            })
            .contains("unknown report")
        );
        for bad in ["", "0", "0g", "00 5", "+1"] {
            let fields = Fields {
                request: Some(bad),
                ..output()
            };
            assert!(error(&fields).contains("not hex bytes"), "{bad:?}");
        }
        // The reply prefix must tell the reply apart, and indexes must fall after it.
        let bare = Fields {
            reply_prefix: Some("00"),
            ..output()
        };
        assert!(error(&bare).contains("byte after the report id"));
        assert!(
            error(&Fields {
                level_byte: Some(1),
                ..output()
            })
            .contains("not after")
        );
        assert!(
            error(&Fields {
                level_byte: Some(65),
                ..output()
            })
            .contains("65-byte")
        );
        let feature = Fields {
            report: Some("feature"),
            reply_prefix: Some("01"),
            ..output()
        };
        assert!(error(&feature).contains("request's id"));
        let half = Fields {
            charging_byte: Some(27),
            ..output()
        };
        assert!(error(&half).contains("go together"));
        let zero_mask = Fields {
            charging_mask: Some(0),
            ..half.clone()
        };
        assert!(error(&zero_mask).contains("never reads charging"));
        assert!(
            error(&Fields {
                scale: Some(0),
                ..output()
            })
            .contains("scale 0")
        );
    }

    #[test]
    fn generic_fields_belong_to_generic_rows_only() {
        assert_eq!(check(false, &Fields::default()), Ok(None));
        let stray = Fields {
            scale: Some(100),
            ..Fields::default()
        };
        assert!(
            check(false, &stray)
                .unwrap_err()
                .contains("Generic rows only")
        );
        assert!(check(false, &output()).is_err());
    }
}
