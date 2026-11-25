//! Light or dark ink for the bar the icon sits on.

use strip_render::Theme;
use tray_icon::TrayIcon;

/// Best Aqua / DarkAqua match of the status-item button, which follows the menu bar tint.
#[cfg(target_os = "macos")]
pub fn current_theme(tray: &TrayIcon) -> Theme {
    use objc2_app_kit::{
        NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    };
    use objc2_foundation::{MainThreadMarker, NSArray};

    let Some(mtm) = MainThreadMarker::new() else {
        return Theme::Light;
    };
    let Some(button) = tray.ns_status_item().and_then(|item| item.button(mtm)) else {
        return Theme::Light;
    };
    // SAFETY: both statics are valid appearance names for the lifetime of the process.
    let (aqua, dark_aqua) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    let best = button
        .effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[aqua, dark_aqua]));
    if best.is_some_and(|name| name.isEqualToString(dark_aqua)) {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// Asks the event loop to re-check the theme when macOS announces an appearance change.
#[cfg(target_os = "macos")]
pub fn watch_changes(proxy: tao::event_loop::EventLoopProxy<crate::app::UserEvent>) {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2_foundation::{
        NSDistributedNotificationCenter, NSNotification, NSOperationQueue, ns_string,
    };

    let on_change = RcBlock::new(move |_: NonNull<NSNotification>| {
        // A closed event loop means the app is exiting.
        let _ = proxy.send_event(crate::app::UserEvent::ThemeChanged);
    });
    // SAFETY: the block only sends a proxy event; the main queue runs it on the main thread.
    let observer = unsafe {
        NSDistributedNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(ns_string!("AppleInterfaceThemeChangedNotification")),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &on_change,
        )
    };
    // The observer stays registered for the whole process lifetime.
    std::mem::forget(observer);
}

#[cfg(target_os = "macos")]
mod appearance_observer {
    use std::ffi::c_void;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol};
    use objc2::{AnyThread, DefinedClass, define_class, msg_send};
    use objc2_foundation::{NSObject, NSString};
    use tao::event_loop::EventLoopProxy;

    use crate::app::UserEvent;

    pub struct Ivars {
        pub proxy: EventLoopProxy<UserEvent>,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and `AppearanceObserver` has no `Drop`.
        #[unsafe(super(NSObject))]
        #[ivars = Ivars]
        pub struct AppearanceObserver;

        impl AppearanceObserver {
            // Fires on every `effectiveAppearance` change of the observed button.
            #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
            fn observe_value(
                &self,
                _key_path: Option<&NSString>,
                _object: Option<&AnyObject>,
                _change: Option<&AnyObject>,
                _context: *mut c_void,
            ) {
                // A closed event loop means the app is exiting.
                let _ = self.ivars().proxy.send_event(UserEvent::ThemeChanged);
            }
        }

        unsafe impl NSObjectProtocol for AppearanceObserver {}
    );

    impl AppearanceObserver {
        pub fn new(proxy: EventLoopProxy<UserEvent>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(Ivars { proxy });
            // SAFETY: `init` on an NSObject subclass with its ivars set.
            unsafe { msg_send![super(this), init] }
        }
    }
}

/// The status-item button's appearance resolves after launch and follows the menu-bar tint
/// (wallpaper too), which no system notification announces, so its changes are observed directly.
#[cfg(target_os = "macos")]
pub fn watch_button_appearance(
    tray: &TrayIcon,
    proxy: tao::event_loop::EventLoopProxy<crate::app::UserEvent>,
) {
    use objc2_foundation::{
        MainThreadMarker, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration,
        ns_string,
    };

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(button) = tray.ns_status_item().and_then(|item| item.button(mtm)) else {
        return;
    };
    let observer = appearance_observer::AppearanceObserver::new(proxy);
    // SAFETY: the observer implements `observeValueForKeyPath:ofObject:change:context:`, is
    // kept alive below, and the context is null.
    unsafe {
        button.addObserver_forKeyPath_options_context(
            &observer,
            ns_string!("effectiveAppearance"),
            // The observer only needs the nudge, not the change dictionary.
            NSKeyValueObservingOptions::empty(),
            std::ptr::null_mut(),
        );
    }
    // Observer and button stay alive for the whole process, so KVO never messages a freed object.
    std::mem::forget(observer);
    std::mem::forget(button);
}

/// `SystemUsesLightTheme = 0` means dark (taskbar); a missing value means light.
#[cfg(windows)]
pub fn system_theme() -> Theme {
    personalize_theme(windows::core::w!("SystemUsesLightTheme"))
}

/// `AppsUseLightTheme = 0` means dark (app windows and their menus); a missing value means light.
#[cfg(windows)]
pub fn apps_theme() -> Theme {
    personalize_theme(windows::core::w!("AppsUseLightTheme"))
}

/// Reads one `Personalize` light-theme flag: 0 is dark, anything else or missing is light.
#[cfg(windows)]
fn personalize_theme(value: windows::core::PCWSTR) -> Theme {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;

    let mut uses_light = 1u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the buffer is a u32 and `size` is its byte length.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut uses_light).cast()),
            Some(&mut size),
        )
    };
    if read.is_ok() && uses_light == 0 {
        Theme::Dark
    } else {
        Theme::Light
    }
}

#[cfg(windows)]
pub fn current_theme(_tray: &TrayIcon) -> Theme {
    system_theme()
}
