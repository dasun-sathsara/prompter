//! AppKit details that make the window behave like a heads-up display.
//!
//! GPUI gives us a non-activating panel at pop-up level. Two things it leaves
//! out matter here: a panel hides itself when its app deactivates unless told
//! otherwise, and a panel keeps a standard titlebar and traffic lights.

#[cfg(target_os = "macos")]
pub fn configure(window: &mut gpui::Window) {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };

    // SAFETY: this runs on the main thread while the window is alive. Every
    // selector below is stable AppKit API taking scalar arguments.
    unsafe {
        let view = appkit.ns_view.as_ptr() as *mut AnyObject;
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }

        // The one that matters: a visible panel must survive another app taking
        // focus. Clicking Chrome would otherwise hide the prompter.
        let _: () = msg_send![ns_window, setHidesOnDeactivate: Bool::from(false)];

        // No title text over the script, and no traffic lights on the strip.
        let hidden_title: isize = 1; // NSWindowTitleVisibility.hidden
        let _: () = msg_send![ns_window, setTitleVisibility: hidden_title];
        for index in 0..3_usize {
            let button: *mut AnyObject = msg_send![ns_window, standardWindowButton: index];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: Bool::from(true)];
            }
        }

        // The panel paints its own hairline edge; AppKit's shadow on a
        // transparent window is a hard rectangle around the corners.
        let _: () = msg_send![ns_window, setHasShadow: Bool::from(false)];

        // CanJoinAllSpaces | FullScreenAuxiliary, as GPUI sets, plus Stationary
        // so Mission Control leaves the prompter where it is.
        let behavior: usize = (1 << 0) | (1 << 4) | (1 << 8);
        let _: () = msg_send![ns_window, setCollectionBehavior: behavior];

        // `PROMPTER_DEBUG_WINDOW=1 prompter` prints what AppKit actually holds,
        // which is the only way to check this from outside a debugger.
        if std::env::var_os("PROMPTER_DEBUG_WINDOW").is_some() {
            let mask: usize = msg_send![ns_window, styleMask];
            let level: isize = msg_send![ns_window, level];
            let hides: Bool = msg_send![ns_window, hidesOnDeactivate];
            let opaque: Bool = msg_send![ns_window, isOpaque];
            let behavior: usize = msg_send![ns_window, collectionBehavior];
            eprintln!(
                "window styleMask={mask:#x} (titled={} closable={} miniaturizable={} resizable={} nonactivating_panel={} full_size_content={})",
                mask & 1 != 0,
                mask & 2 != 0,
                mask & 4 != 0,
                mask & 8 != 0,
                mask & 0x80 != 0,
                mask & 0x8000 != 0,
            );
            eprintln!(
                "window level={level} hidesOnDeactivate={} opaque={} collectionBehavior={behavior:#x}",
                hides.as_bool(),
                opaque.as_bool()
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn configure(_window: &mut gpui::Window) {}
