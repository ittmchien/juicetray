//! The new-device popup: one question at a time on its own thread, the rest waiting in a queue
//! the UI thread owns. Answers come back as `UserEvent`s, so config writes stay on that thread.

use std::collections::VecDeque;

use crate::plug::{Id, Offer};

/// Offers waiting for the popup, each id once, and the id on screen.
#[derive(Debug, Default)]
pub struct OfferQueue {
    waiting: VecDeque<Offer>,
    showing: Option<Id>,
}

impl OfferQueue {
    /// Queues `offer` unless its id is already waiting or on screen.
    pub fn push(&mut self, offer: Offer) {
        let queued = self.showing == Some(offer.id)
            || self.waiting.iter().any(|waiting| waiting.id == offer.id);
        if !queued {
            self.waiting.push_back(offer);
        }
    }

    /// The next offer to show, marked as on screen; `None` while one is shown or none waits.
    pub fn next(&mut self) -> Option<Offer> {
        if self.showing.is_some() {
            return None;
        }
        let offer = self.waiting.pop_front()?;
        self.showing = Some(offer.id);
        Some(offer)
    }

    /// The popup on screen closed.
    pub fn closed(&mut self) {
        self.showing = None;
    }

    /// Forgets every waiting offer; the one on screen stays until it closes.
    pub fn drop_waiting(&mut self) {
        self.waiting.clear();
    }

    /// Keeps only the waiting offers whose id `keep` accepts.
    pub fn retain(&mut self, keep: impl Fn(Id) -> bool) {
        self.waiting.retain(|offer| keep(offer.id));
    }
}

#[cfg(windows)]
pub use win::show;

#[cfg(windows)]
mod win {
    use std::sync::mpsc::{self, Sender};

    use tao::event_loop::EventLoopProxy;

    use crate::add_device::AddRequest;
    use crate::app::UserEvent;
    use crate::config::CustomEntry;
    use crate::dialog;
    use crate::log;
    use crate::plug::{Id, Offer};
    use crate::remote_catalogue;

    const TITLE: &str = "New device";

    /// The events for an answer: the id is always recorded as asked; OK saves the catalogue
    /// `row` (answered on `reply`) or else opens the Add device window.
    fn answer_events(
        ok: bool,
        id: Id,
        row: Option<CustomEntry>,
        reply: Sender<Result<(), String>>,
    ) -> Vec<UserEvent> {
        let mut events = vec![UserEvent::Asked(id)];
        if ok {
            events.push(match row {
                Some(entry) => UserEvent::AddDevice(AddRequest::Custom(entry), reply),
                None => UserEvent::OpenAddDevice(id),
            });
        }
        events
    }

    /// Asks about `offer` on a `popup` thread that sends `Asked`, on OK `AddDevice` or
    /// `OpenAddDevice`, then `OfferClosed`; false when the thread cannot start.
    pub fn show(proxy: EventLoopProxy<UserEvent>, offer: Offer) -> bool {
        let spawned = std::thread::Builder::new()
            .name("popup".into())
            .spawn(move || {
                let (vid, pid) = offer.id;
                // Only ids the built-in catalogue lacks are looked up; this thread may block.
                let row = if protocol::known(vid, pid).is_none() {
                    remote_catalogue::lookup(offer.id)
                } else {
                    None
                };
                let name = row
                    .as_ref()
                    .map_or_else(|| offer.name.clone(), |entry| entry.name.clone());
                // Main instruction, then content: the task dialog shows them in two sizes.
                let message = format!("New device found: {name}\n\nAdd it to Juicetray?");
                // Esc and the close box count as Not now.
                let ok = {
                    let _span = log::span("dialog new device");
                    dialog::confirm(TITLE, &message, "OK", "Not now")
                };
                log::trace(format_args!(
                    "new device {vid:04X}:{pid:04X} answered ok={ok} catalogue_row={}",
                    row.is_some()
                ));
                let saves_row = ok && row.is_some();
                let (reply, saved) = mpsc::channel();
                // A closed event loop means the app is exiting.
                for event in answer_events(ok, offer.id, row, reply) {
                    let _ = proxy.send_event(event);
                }
                // A dropped sender (the app exiting) ends the wait with no answer to show.
                if saves_row && let Ok(Err(error)) = saved.recv() {
                    dialog::notice(TITLE, &format!("Could not add {name}.\n\n{error}"));
                }
                let _ = proxy.send_event(UserEvent::OfferClosed);
            });
        match spawned {
            Ok(_) => true,
            Err(error) => {
                log::line(&format!("new-device popup unavailable: {error}"));
                false
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn entry() -> CustomEntry {
            CustomEntry {
                id: "3554:7777".into(),
                name: "Board".into(),
                family: "Report13".into(),
                kind: "keyboard".into(),
                connection: "receiver".into(),
            }
        }

        #[test]
        fn every_answer_records_the_id_and_only_ok_opens_the_add_window() {
            let id = (0x3554, 0xFA09);
            // OK: recorded first, then the Add device window.
            let ok = answer_events(true, id, None, mpsc::channel().0);
            assert!(matches!(
                ok.as_slice(),
                [UserEvent::Asked(a), UserEvent::OpenAddDevice(b)] if *a == id && *b == id
            ));
            // Not now, Esc and the close box: recorded only.
            let not_now = answer_events(false, id, None, mpsc::channel().0);
            assert!(matches!(not_now.as_slice(), [UserEvent::Asked(a)] if *a == id));
        }

        #[test]
        fn ok_saves_a_catalogue_row_instead_of_opening_the_add_window() {
            let id = (0x3554, 0x7777);
            let ok = answer_events(true, id, Some(entry()), mpsc::channel().0);
            assert!(matches!(
                ok.as_slice(),
                [UserEvent::Asked(a), UserEvent::AddDevice(AddRequest::Custom(saved), _)]
                    if *a == id && *saved == entry()
            ));
            // Not now saves nothing, row or not.
            let not_now = answer_events(false, id, Some(entry()), mpsc::channel().0);
            assert!(matches!(not_now.as_slice(), [UserEvent::Asked(a)] if *a == id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: Id) -> Offer {
        Offer {
            id,
            name: format!("{id:?}"),
        }
    }

    #[test]
    fn queue_shows_one_at_a_time_and_skips_duplicates() {
        let mut queue = OfferQueue::default();
        assert_eq!(queue.next(), None);
        queue.push(offer((1, 1)));
        queue.push(offer((2, 2)));
        queue.push(offer((1, 1)));

        assert_eq!(queue.next(), Some(offer((1, 1))));
        // The shown id is not queued again, and nothing else shows until it closes.
        queue.push(offer((1, 1)));
        assert_eq!(queue.next(), None);

        queue.closed();
        assert_eq!(queue.next(), Some(offer((2, 2))));
        queue.closed();
        assert_eq!(queue.next(), None);
        // Once closed, an id can be queued again; the `declined` list, not the queue, stops repeats.
        queue.push(offer((1, 1)));
        assert_eq!(queue.next(), Some(offer((1, 1))));

        // Turning detection off drops the waiting ones; the shown one still closes normally.
        queue.push(offer((3, 3)));
        queue.drop_waiting();
        queue.closed();
        assert_eq!(queue.next(), None);

        // Offers that no longer qualify (added in the Add window meanwhile) drop out.
        queue.push(offer((4, 4)));
        queue.push(offer((5, 5)));
        queue.retain(|id| id != (4, 4));
        assert_eq!(queue.next(), Some(offer((5, 5))));
    }
}
