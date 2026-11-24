//! `juicetray listen <VID:PID> [seconds]`: read-only diagnostic. Opens the same target `probe`
//! would and only reads, so you can see what a vendor app's own polling gets back. Nothing is
//! sent, so there is no floor and no query stamp.

use std::process::ExitCode;
use std::time::Duration;

use protocol::{Link, known};

use crate::probe::unknown_device;
use crate::query::{open_hid_target, open_serial_target, parse_vid_pid, refusal_for, target_why};
use crate::scan::bytes_line;

pub(crate) const LISTEN_USAGE: &str = "usage: juicetray listen <VID:PID> [seconds, 1-60, default 10] \
     (read-only: prints what the device sends, e.g. the replies to a vendor app's polling)";

const DEFAULT_SECONDS: u64 = 10;
const MAX_SECONDS: u64 = 60;

/// Prints every report as it arrives, then the count; exit 0 once the listen ran to its end.
pub fn run_listen(args: Vec<String>) -> ExitCode {
    match listen(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(text) => {
            eprintln!("{text}");
            ExitCode::FAILURE
        }
    }
}

/// `<VID:PID>` and the optional whole seconds (1..=60, default 10).
fn parse_listen_args(args: &[String]) -> Result<((u16, u16), Duration), String> {
    let (id, seconds) = match args {
        [id] => (id, DEFAULT_SECONDS),
        [id, seconds] => {
            let seconds = seconds
                .parse::<u64>()
                .ok()
                .filter(|seconds| (1..=MAX_SECONDS).contains(seconds))
                .ok_or_else(|| format!("seconds must be 1-60\n{LISTEN_USAGE}"))?;
            (id, seconds)
        }
        _ => return Err(LISTEN_USAGE.to_owned()),
    };
    let id = parse_vid_pid(id).map_err(|_| LISTEN_USAGE.to_owned())?;
    Ok((id, Duration::from_secs(seconds)))
}

/// One received report: time since the start in ms, then every byte in hex.
fn report_line(elapsed: Duration, bytes: &[u8]) -> String {
    format!("{:>6} ms  {}", elapsed.as_millis(), bytes_line("rx", bytes))
}

/// The flow in order: parse, known device, list, pick, open (never mouse/keyboard collections),
/// then read only. A device that needs a capture is allowed: nothing is written to it.
fn listen(args: &[String]) -> Result<(), String> {
    let ((vid, pid), duration) = parse_listen_args(args)?;
    let device = known(vid, pid).ok_or_else(|| unknown_device("listen", vid, pid))?;
    let family = device.family;

    let inventory = door::inventory(&[(vid, pid)]);
    let opened = match family.link() {
        Link::Hid => open_hid_target(&inventory, family, vid, pid),
        Link::Serial => open_serial_target(&inventory, vid, pid),
    };
    let (mut target, target_line) =
        opened.map_err(|error| refusal_for("listen", vid, pid, &target_why(family, error)))?;

    println!(
        "listen {vid:04X}:{pid:04X} · {} · {}",
        device.name,
        family.name()
    );
    println!("{target_line}");
    println!(
        "listening {} s, read-only: nothing is sent",
        duration.as_secs()
    );
    let mut count = 0usize;
    let result = target.listen(duration, |elapsed, bytes| {
        count += 1;
        println!("{}", report_line(elapsed, bytes));
    });
    println!("{count} report(s) in {} s", duration.as_secs());
    result.map_err(|error| format!("listen {vid:04X}:{pid:04X}: read failed: {error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn listen_args_default_and_bounds() {
        let id = (0x3837, 0x6011);
        let parse = |list: &[&str]| parse_listen_args(&args(list));
        assert_eq!(
            parse(&["3837:6011"]),
            Ok((id, Duration::from_secs(DEFAULT_SECONDS)))
        );
        assert_eq!(parse(&["3837:6011", "1"]), Ok((id, Duration::from_secs(1))));
        assert_eq!(
            parse(&["3837:6011", "60"]),
            Ok((id, Duration::from_secs(60)))
        );
        for bad in ["0", "61", "-1", "abc", "1.5", ""] {
            let error = parse(&["3837:6011", bad]).expect_err(bad);
            assert!(error.contains("seconds must be 1-60"), "{error}");
        }
        for bad in [&[][..], &["3837:6011", "5", "6"][..], &["nope"][..]] {
            assert_eq!(parse(bad), Err(LISTEN_USAGE.to_owned()), "{bad:?}");
        }
    }

    #[test]
    fn report_line_shows_ms_and_hex() {
        assert_eq!(
            report_line(Duration::from_millis(1250), &[0x08, 0x04, 0xff]),
            "  1250 ms  rx (3 B): 08 04 ff"
        );
    }

    #[test]
    fn listen_refuses_unknown_device() {
        let text = listen(&args(&["046D:C52B"])).expect_err("unknown device");
        assert!(
            text.starts_with("listen 046D:C52B: is not a known device"),
            "{text}"
        );
        assert!(text.ends_with("nothing was sent"), "{text}");
    }
}
