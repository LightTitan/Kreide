//! Let a provider page write to the clipboard for a few seconds after the
//! user clicks or types in it.
//!
//! WebKit accepts a clipboard write only briefly after a click, so copies that
//! finish async work first (ChatGPT's link cards) fail with NotAllowedError
//! where Chrome, which keeps a click's activation for 5 seconds, succeeds.
//! WebKit has no setting for that window, only `javaScriptCanAccessClipboard`,
//! a private WKPreferences key that skips the check entirely. Turn it on for
//! the page the user just interacted with, and off again 5 seconds after the
//! last click or key press. Reads still need WebKit's own Paste prompt.
use std::{
    cell::RefCell,
    ptr::NonNull,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, NSObjectProtocol},
};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSView};
use objc2_foundation::{NSNumber, NSString};
use tauri::{AppHandle, Manager};

/// Chrome's user-activation lifetime.
const WINDOW: Duration = Duration::from_secs(5);

static REVISION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    // Only the event monitor and its timers touch this, all on the main thread.
    static GRANTED: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
}

/// Watch input in the main window. Call once, on the main thread.
pub(super) fn install(app: AppHandle) {
    let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::KeyDown;
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        if let Some(page) = page_for(&app, unsafe { event.as_ref() }) {
            grant(page);
        }
        event.as_ptr()
    });
    // The monitor lives as long as the app.
    std::mem::forget(unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) });
}

/// The web view in Kreide's main window that will receive `event`.
fn page_for(app: &AppHandle, event: &NSEvent) -> Option<Retained<NSView>> {
    let window = event.window(objc2::MainThreadMarker::new()?)?;
    let main = app.get_window("main")?.ns_window().ok()?;
    if !std::ptr::eq(&*window as *const _ as *const std::ffi::c_void, main) {
        return None; // Sign-in popups and Settings keep WebKit's default.
    }
    let mut view = if event.r#type() == NSEventType::KeyDown {
        window.firstResponder()?.downcast::<NSView>().ok()?
    } else {
        // hitTest takes a point in the content view's superview.
        let content = window.contentView()?;
        let point = unsafe { content.superview() }.map_or(event.locationInWindow(), |frame| {
            frame.convertPoint_fromView(event.locationInWindow(), None)
        });
        content.hitTest(point)?
    };
    let web_view = AnyClass::get(c"WKWebView")?;
    // WebKit hit-tests to its own subviews: walk up to the WKWebView.
    loop {
        if view.isKindOfClass(web_view) {
            return Some(view);
        }
        view = unsafe { view.superview() }?;
    }
}

fn grant(page: Retained<NSView>) {
    GRANTED.with_borrow_mut(|granted| {
        if granted.as_ref().is_some_and(|current| *current == page) {
            return; // Already allowed: don't send WebKit a preference update per key.
        }
        if let Some(previous) = granted.take() {
            set_clipboard_writes(&previous, false);
        }
        set_clipboard_writes(&page, true);
        *granted = Some(page);
    });
    let revision = REVISION.fetch_add(1, Ordering::Relaxed) + 1;
    let when = DispatchTime::try_from(WINDOW).unwrap();
    let _ = DispatchQueue::main().after(when, move || {
        if REVISION.load(Ordering::Relaxed) != revision {
            return; // Extended by later input.
        }
        GRANTED.with_borrow_mut(|granted| {
            if let Some(page) = granted.take() {
                set_clipboard_writes(&page, false);
            }
        });
    });
}

fn set_clipboard_writes(web_view: &NSView, allowed: bool) {
    unsafe {
        let configuration: Retained<AnyObject> = msg_send![web_view, configuration];
        let preferences: Retained<AnyObject> = msg_send![&configuration, preferences];
        let _: () = msg_send![
            &preferences,
            setValue: &*NSNumber::new_bool(allowed),
            forKey: &*NSString::from_str("javaScriptCanAccessClipboard")
        ];
    }
}
