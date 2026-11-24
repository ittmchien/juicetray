//! Read-only listening: input reports or serial bytes for a fixed time. This file never calls
//! `Transport::write`, so listening cannot send anything to the device.

use std::time::{Duration, Instant};

use crate::transport::Target;
use crate::write::{HID_READ_LEN, time_left};

impl Target {
    /// Reads for `duration` and hands each read to `on_report` with its time since the start.
    /// A read that times out with nothing is skipped; a read error ends the listen.
    pub fn listen(
        &mut self,
        duration: Duration,
        mut on_report: impl FnMut(Duration, &[u8]),
    ) -> anyhow::Result<()> {
        let start = Instant::now();
        let deadline = start + duration;
        let mut buf = [0u8; HID_READ_LEN];
        while let Some(timeout) = time_left(deadline) {
            let len = self.transport.read(&mut buf, timeout)?;
            if len > 0 {
                on_report(start.elapsed(), &buf[..len]);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use protocol::Link;

    use super::*;
    use crate::transport::Transport;

    /// Plays back scripted reads; any write is a test failure.
    struct ReadOnlyFake {
        reads: VecDeque<Vec<u8>>,
    }

    impl Transport for ReadOnlyFake {
        fn write(&mut self, _bytes: &[u8]) -> anyhow::Result<()> {
            panic!("listen must never write");
        }

        fn read(&mut self, buf: &mut [u8], timeout: Duration) -> anyhow::Result<usize> {
            let Some(bytes) = self.reads.pop_front() else {
                std::thread::sleep(timeout.min(Duration::from_millis(5)));
                return Ok(0);
            };
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    fn target(link: Link, reads: Vec<Vec<u8>>) -> Target {
        Target {
            vid: 0x3837,
            pid: 0x6011,
            link,
            transport: Box::new(ReadOnlyFake {
                reads: reads.into(),
            }),
        }
    }

    #[test]
    fn listen_collects_reads_in_order_without_writing() {
        for link in [Link::Hid, Link::Serial] {
            let mut target = target(link, vec![vec![1, 2, 3], vec![4]]);
            let mut seen: Vec<(Duration, Vec<u8>)> = Vec::new();
            target
                .listen(Duration::from_millis(40), |at, bytes| {
                    seen.push((at, bytes.to_vec()));
                })
                .expect("listen");
            let payloads: Vec<&[u8]> = seen.iter().map(|(_, bytes)| bytes.as_slice()).collect();
            assert_eq!(payloads, [&[1u8, 2, 3][..], &[4][..]]);
            assert!(seen[0].0 <= seen[1].0);
        }
    }

    #[test]
    fn listen_stops_at_the_deadline() {
        let mut target = target(Link::Hid, Vec::new());
        let start = Instant::now();
        target
            .listen(Duration::from_millis(30), |_, _| panic!("nothing to read"))
            .expect("listen");
        assert!(start.elapsed() >= Duration::from_millis(30));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
