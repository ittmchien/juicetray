//! Wake-from-sleep notice (macOS); Windows gets it from its power broadcast in `plug.rs`.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
use objc2_foundation::{NSNotification, NSOperationQueue};
use tao::event_loop::EventLoopProxy;

use crate::app::UserEvent;

/// Sends `UserEvent::Woke` when the Mac wakes, so devices that dropped during sleep are read again.
pub fn watch(proxy: EventLoopProxy<UserEvent>) {
    let on_wake = RcBlock::new(move |_: NonNull<NSNotification>| {
        // A closed event loop means the app is exiting.
        let _ = proxy.send_event(UserEvent::Woke);
    });
    // SAFETY: the block only sends a proxy event; the main queue runs it on the main thread.
    let observer = unsafe {
        NSWorkspace::sharedWorkspace()
            .notificationCenter()
            .addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &on_wake,
            )
    };
    // The observer stays registered for the whole process lifetime.
    std::mem::forget(observer);
}
