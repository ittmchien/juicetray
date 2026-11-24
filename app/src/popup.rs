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
    use tao::event_loop::EventLoopProxy;

    use crate::app::UserEvent;
    use crate::dialog;
    use crate::log;
    use crate::plug::{Id, Offer};

    const TITLE: &str = "New device";

    /// The events for an answer: the id is recorded as asked whatever the answer (so it is never
    /// offered again), and OK also opens the Add device window.
    fn answer_events(ok: bool, id: Id) -> Vec<UserEvent> {
        let mut events = vec![UserEvent::Asked(id)];
        if ok {
            events.push(UserEvent::OpenAddDevice(id));
        }
        events
    }

    /// Asks about `offer` on a `popup` thread, which answers with `Asked` (and `OpenAddDevice`
    /// on OK) and then `OfferClosed`; false when the thread cannot start.
    pub fn show(proxy: EventLoopProxy<UserEvent>, offer: Offer) -> bool {
        let spawned = std::thread::Builder::new()
            .name("popup".into())
            .spawn(move || {
                // Main instruction, then content: the task dialog shows them in two sizes.
                let message = format!("New device found: {}\n\nAdd it to Juicetray?", offer.name);
                // Esc and the close box count as Not now.
                let ok = {
                    let _span = log::span("dialog new device");
                    dialog::confirm(TITLE, &message, "OK", "Not now")
                };
                log::trace(format_args!(
                    "new device {:04X}:{:04X} answered ok={ok}",
                    offer.id.0, offer.id.1
                ));
                // A closed event loop means the app is exiting.
                for event in answer_events(ok, offer.id) {
                    let _ = proxy.send_event(event);
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

        #[test]
        fn every_answer_records_the_id_and_only_ok_opens_the_add_window() {
            let id = (0x3554, 0xFA09);
            // OK: recorded first, then the Add device window.
            let ok = answer_events(true, id);
            assert!(matches!(
                ok.as_slice(),
                [UserEvent::Asked(a), UserEvent::OpenAddDevice(b)] if *a == id && *b == id
            ));
            // Not now, Esc and the close box: recorded only.
            let not_now = answer_events(false, id);
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
