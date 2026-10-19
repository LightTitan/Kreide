//! Repair activation after restoring a menu-bar app's Dock/switcher entry.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use dispatch2::{DispatchQueue, DispatchTime};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_foundation::NSString;
use tauri::{AppHandle, Manager};

static REVISION: AtomicU64 = AtomicU64::new(0);

pub(super) fn cancel() {
    REVISION.fetch_add(1, Ordering::Relaxed);
}

fn next_turn(work: impl FnOnce() + Send + 'static) {
    let when = DispatchTime::try_from(Duration::from_millis(1)).unwrap();
    let _ = DispatchQueue::main().after(when, work);
}

/// A policy change can leave the app active before its switcher entry exists.
/// Focusing an already-active app then produces no fresh activation for the
/// switcher. Hand activation to the Dock and back after the policy change.
/// Electron uses the same Dock handoff for its foreground-promotion workaround:
/// https://github.com/electron/electron/blob/main/shell/browser/browser_mac.mm
pub(super) fn refresh(app: &AppHandle) {
    let revision = REVISION.load(Ordering::Relaxed);
    let app = app.clone();
    next_turn(move || {
        if REVISION.load(Ordering::Relaxed) != revision || !super::any_window_visible(&app) {
            return;
        }
        // If the user has already switched away, don't take focus back.
        if !app
            .windows()
            .values()
            .any(|window| window.is_focused().unwrap_or(false))
        {
            return;
        }
        let current = NSRunningApplication::currentApplication();
        #[allow(deprecated)]
        let options = NSApplicationActivationOptions::ActivateIgnoringOtherApps;
        // A key window doesn't necessarily make its process frontmost. Ask
        // LaunchServices to activate the process explicitly in that case.
        if !current.isActive() {
            current.activateWithOptions(options);
            return;
        }
        let docks = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str("com.apple.dock"),
        );
        let Some(dock) = docks.firstObject() else {
            return;
        };
        if !dock.activateWithOptions(options) {
            return;
        }
        next_turn(move || {
            if REVISION.load(Ordering::Relaxed) != revision
                || !super::any_window_visible(&app)
                || !dock.isActive()
            {
                return;
            }
            if current.activateWithOptions(options) {
                if let Ok(menu) = super::app_menu(&app) {
                    let _ = app.set_menu(menu);
                }
                super::focus_composer(&app);
            }
        });
    });
}
