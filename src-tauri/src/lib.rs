use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::{NewWindowResponse, WebviewBuilder},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    WebviewUrl,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[cfg(target_os = "macos")]
mod macos_activation;
#[cfg(target_os = "macos")]
mod macos_clipboard;

/// Height of the local toolbar drawn above the provider page.
const TOOLBAR: f64 = 44.0;
const SHORTCUT: &str = "CommandOrControl+Shift+Space";
static POPUP_ID: AtomicU64 = AtomicU64::new(0);
const FOCUS_SCRIPT: &str = include_str!("provider-focus.js");
// Google's published domain set, retrieved 2026-10-19. Vendor it so sign-in
// doesn't depend on a separate network fetch or a guessed country-code pattern.
// Source/update URL: https://www.google.com/supported_domains
const GOOGLE_DOMAINS: &str = include_str!("google-domains.txt");
const PROVIDERS: [Provider; 2] = [Provider::Chatgpt, Provider::Claude];
/// `fresh_after_minutes` value that ends the chat as soon as the window hides.
const FRESH_IMMEDIATELY: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Provider {
    Chatgpt,
    Claude,
}
impl Provider {
    fn label(self) -> &'static str {
        match self {
            Self::Chatgpt => "chatgpt",
            Self::Claude => "claude",
        }
    }
    fn url(self) -> &'static str {
        match self {
            Self::Chatgpt => "https://chatgpt.com/?temporary-chat=true",
            Self::Claude => "https://claude.ai/new?incognito",
        }
    }
    fn store(self) -> [u8; 16] {
        match self {
            Self::Chatgpt => *b"kreide-chatgpt01",
            Self::Claude => *b"kreide-claude001",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    default_provider: Provider,
    always_on_top: bool,
    shortcut_enabled: bool,
    /// Minutes hidden before the next summon starts a fresh chat. 0 = never,
    /// `FRESH_IMMEDIATELY` = as soon as the window hides.
    #[serde(default = "default_fresh_after")]
    fresh_after_minutes: u32,
}
fn default_fresh_after() -> u32 {
    15
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            default_provider: Provider::Chatgpt,
            always_on_top: false,
            shortcut_enabled: true,
            fresh_after_minutes: default_fresh_after(),
        }
    }
}
#[derive(Clone, Copy, Deserialize)]
struct Viewport {
    width: f64,
    height: f64,
}

struct Runtime {
    viewport: Mutex<Viewport>,
    #[cfg(target_os = "macos")]
    toolbar_inset: Mutex<Option<f64>>,
    preferences: Mutex<Preferences>,
    active: Mutex<Option<Provider>>,
    hidden_at: Mutex<Option<SystemTime>>,
    show_revision: AtomicU64,
    show_pending: AtomicBool,
    /// Where the main window was when it was released, to rebuild it there.
    frame: Mutex<Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>>,
    warning: Mutex<Option<String>>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    preferences: Preferences,
    active_provider: Option<Provider>,
    loaded: Vec<Provider>,
    warning: Option<String>,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn trusted(view: &tauri::Webview) -> Result<(), String> {
    // Custom Rust commands also enforce their caller; a remote child has no commands.
    if matches!(view.label(), "main" | "settings") {
        Ok(())
    } else {
        Err("This action is only available to Kreide’s local controls.".into())
    }
}
fn runtime(app: &AppHandle) -> tauri::State<'_, Runtime> {
    app.state::<Runtime>()
}
fn snapshot(app: &AppHandle) -> Snapshot {
    let state = runtime(app);
    let result = Snapshot {
        preferences: state.preferences.lock().unwrap().clone(),
        active_provider: *state.active.lock().unwrap(),
        loaded: PROVIDERS
            .into_iter()
            .filter(|p| app.get_webview(p.label()).is_some())
            .collect(),
        warning: state.warning.lock().unwrap().clone(),
    };
    result
}
fn changed(app: &AppHandle) {
    let _ = app.emit("session-changed", snapshot(app));
}
fn main_window(app: &AppHandle) -> Result<tauri::Window, String> {
    app.get_window("main")
        .ok_or("Main window is unavailable".into())
}
fn provider_bounds(app: &AppHandle) -> Result<(LogicalPosition<f64>, LogicalSize<f64>), String> {
    // Anchor the child to the local webview's actual bounds rather than
    // assuming window (0, 0); some platforms put native chrome above it.
    let main = app
        .get_webview("main")
        .ok_or("Local controls are unavailable")?;
    let scale = main_window(app)?.scale_factor().map_err(err)?;
    let origin = main.position().map_err(err)?.to_logical::<f64>(scale);
    let size = main.size().map_err(err)?.to_logical::<f64>(scale);
    let viewport = *runtime(app).viewport.lock().unwrap();
    #[cfg(target_os = "macos")]
    if let Some(inset) = *runtime(app).toolbar_inset.lock().unwrap() {
        // The local view is only toolbar-height now. Size the chat from the
        // window, never from that view's reduced CSS viewport.
        let window_size = main_window(app)?
            .inner_size()
            .map_err(err)?
            .to_logical::<f64>(scale);
        return Ok(content_bounds(
            origin,
            window_size,
            Viewport {
                width: window_size.width,
                height: (window_size.height - inset).max(1.0),
            },
        ));
    }
    Ok(content_bounds(origin, size, viewport))
}
fn content_bounds(
    origin: LogicalPosition<f64>,
    native: LogicalSize<f64>,
    viewport: Viewport,
) -> (LogicalPosition<f64>, LogicalSize<f64>) {
    // WKWebView's CSS viewport can exclude an automatic title-bar inset that
    // its reported frame includes (notably on macOS 26).
    let inset = (native.height - viewport.height).max(0.0);
    (
        LogicalPosition::new(origin.x, origin.y + inset + TOOLBAR),
        LogicalSize::new(viewport.width, (viewport.height - TOOLBAR).max(1.0)),
    )
}
fn update_viewport(app: &AppHandle, viewport: Viewport) -> Result<(), String> {
    if !viewport.width.is_finite()
        || !viewport.height.is_finite()
        || !(1.0..=100_000.0).contains(&viewport.width)
        || !(1.0..=100_000.0).contains(&viewport.height)
    {
        return Err("Invalid viewport dimensions".into());
    }
    *runtime(app).viewport.lock().unwrap() = viewport;
    Ok(())
}
fn fit(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    fit_toolbar(app);
    if let Ok((position, size)) = provider_bounds(app) {
        for provider in PROVIDERS {
            if let Some(view) = app.get_webview(provider.label()) {
                let _ = view.set_position(position);
                let _ = view.set_size(size);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn fit_toolbar(app: &AppHandle) {
    let Some(inset) = *runtime(app).toolbar_inset.lock().unwrap() else {
        return; // Wait for the first CSS viewport measurement after boot.
    };
    let (Some(main), Ok(window)) = (app.get_webview("main"), main_window(app)) else {
        return;
    };
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    let loaded = PROVIDERS
        .iter()
        .any(|p| app.get_webview(p.label()).is_some());
    // With Tauri's unstable multiwebview runtime the initial webview is a
    // native child too. Disable proportional resizing and lay it out ourselves.
    let _ = main.set_auto_resize(false);
    let _ = main.set_size(toolbar_size(size, inset, loaded));
}

#[cfg(any(target_os = "macos", test))]
fn toolbar_size(window: LogicalSize<f64>, inset: f64, loaded: bool) -> LogicalSize<f64> {
    LogicalSize::new(
        window.width,
        if loaded {
            (TOOLBAR + inset).min(window.height)
        } else {
            window.height
        },
    )
}

fn focus_composer(app: &AppHandle) {
    let Some(provider) = *runtime(app).active.lock().unwrap() else {
        return;
    };
    if let Some(view) = app.get_webview(provider.label()) {
        let _ = view.set_focus();
        let _ = view.eval("window.__kreide && window.__kreide.focus()");
    }
}
fn stale(app: &AppHandle) -> bool {
    let minutes = runtime(app).preferences.lock().unwrap().fresh_after_minutes;
    let hidden_at = *runtime(app).hidden_at.lock().unwrap();
    hidden_at
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|elapsed| {
            minutes == FRESH_IMMEDIATELY
                || minutes > 0 && elapsed >= Duration::from_secs(u64::from(minutes) * 60)
        })
}
/// While no window is open, Kreide lives only in the menu bar: no Dock icon
/// and no ⌘Tab entry. The global shortcut and tray keep working.
fn set_background(app: &AppHandle, background: bool) {
    #[cfg(target_os = "macos")]
    {
        static BACKGROUND: AtomicBool = AtomicBool::new(false);
        if BACKGROUND.swap(background, Ordering::Relaxed) == background {
            return;
        }
        macos_activation::cancel();
        let _ = app.set_activation_policy(if background {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        });
        // Leaving accessory mode leaves the menu bar half-drawn, with its last
        // menu stuck highlighted. Installing the menu again once the policy has
        // taken effect redraws it cleanly.
        if !background {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Ok(menu) = app_menu(&handle) {
                    let _ = handle.set_menu(menu);
                }
                macos_activation::refresh(&handle);
            });
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, background);
}
fn any_window_visible(app: &AppHandle) -> bool {
    app.windows()
        .values()
        .any(|w| w.is_visible().unwrap_or(false))
}
fn show(app: &AppHandle) {
    let Ok(window) = main_window(app).or_else(|_| rebuild_main(app)) else {
        return;
    };
    let state = runtime(app);
    let revision = state.show_revision.fetch_add(1, Ordering::Relaxed) + 1;
    state.show_pending.store(true, Ordering::Relaxed);
    // macOS queues native size/position changes even on the main thread.
    // Reveal on that same queue after restoration, so the first visible frame
    // already has its final geometry. No timer or extra animation is needed.
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        dispatch2::DispatchQueue::main().exec_async(move || present_main(&app, window, revision));
    }
    #[cfg(not(target_os = "macos"))]
    present_main(app, window, revision);
}
fn present_main(app: &AppHandle, window: tauri::Window, revision: u64) {
    let state = runtime(app);
    if state.show_revision.load(Ordering::Relaxed) != revision {
        return;
    }
    state.show_pending.store(false, Ordering::Relaxed);
    // Coming back after a while means a new question: start from a clean slate.
    if stale(app) {
        close_all(app);
    }
    *runtime(app).hidden_at.lock().unwrap() = None;
    set_background(app, false);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    // Until the toolbar has measured the title-bar inset once in `boot`, let
    // `boot` open the provider. Don't size it from a stale CSS viewport.
    #[cfg(target_os = "macos")]
    if runtime(app).toolbar_inset.lock().unwrap().is_none() {
        return;
    }
    // Reopen the last provider (or the default after a fresh start) if its
    // page was unloaded while hidden.
    let provider = runtime(app)
        .active
        .lock()
        .unwrap()
        .unwrap_or_else(|| runtime(app).preferences.lock().unwrap().default_provider);
    if app.get_webview(provider.label()).is_none() {
        if let Err(e) = select(app, provider) {
            let _ = app.emit_to("main", "error", e);
        }
    }
    focus_composer(app);
}
fn hide(app: &AppHandle) {
    // A second shortcut press must cancel a reveal that is still queued.
    runtime(app).show_revision.fetch_add(1, Ordering::Relaxed);
    runtime(app).show_pending.store(false, Ordering::Relaxed);
    if let Ok(window) = main_window(app) {
        let _ = window.hide();
        runtime(app)
            .hidden_at
            .lock()
            .unwrap()
            .get_or_insert_with(SystemTime::now);
        if !any_window_visible(app) {
            set_background(app, true);
        }
        if runtime(app).preferences.lock().unwrap().fresh_after_minutes == FRESH_IMMEDIATELY {
            close_all(app);
            release_main(app);
        } else {
            unload_unused(app);
        }
    }
}
/// A hidden window with no chat in it is just the toolbar page: destroy it so
/// WebKit can shut down its content and GPU processes. `show` rebuilds it.
fn release_main(app: &AppHandle) {
    let Ok(window) = main_window(app) else {
        return;
    };
    if runtime(app).show_pending.load(Ordering::Relaxed)
        || window.is_visible().unwrap_or(true)
        || PROVIDERS
            .iter()
            .any(|p| app.get_webview(p.label()).is_some())
    {
        return;
    }
    if let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) {
        *runtime(app).frame.lock().unwrap() = Some((position, size));
    }
    let _ = window.destroy();
}
/// Explicitly discard the session, bypassing the configured idle timeout.
/// Release the toolbar too so it doesn't keep WebKit processes alive.
fn end_chat_and_close(app: &AppHandle) {
    close_all(app);
    hide(app);
    release_main(app);
}
fn rebuild_main(app: &AppHandle) -> Result<tauri::Window, String> {
    // Keep the measured title-bar inset: the rebuilt window has the same
    // chrome, and keeping it lets `present_main` open the provider right away
    // instead of waiting for the toolbar page to load and call `boot`.
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .ok_or("Main window is not configured")?
        .clone();
    config.visible = false;
    config.center = false;
    let window = tauri::WebviewWindowBuilder::from_config(app, &config)
        .map_err(err)?
        .build()
        .map_err(err)?;
    if let Some((position, size)) = *runtime(app).frame.lock().unwrap() {
        let _ = window.set_size(size);
        let _ = window.set_position(position);
    } else {
        let _ = window.center();
    }
    let _ = window.set_always_on_top(runtime(app).preferences.lock().unwrap().always_on_top);
    main_window(app)
}
/// A page the user never typed into holds nothing worth keeping: free its
/// memory now. Used chats stay until the fresh-chat timeout.
fn unload_unused(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let unused: Vec<Provider> = PROVIDERS
            .into_iter()
            .filter(|provider| {
                let Some(view) = app.get_webview(provider.label()) else {
                    return false;
                };
                let (tx, rx) = std::sync::mpsc::channel();
                let asked = view.eval_with_callback(
                    "window.__kreide ? window.__kreide.used() : true",
                    move |result| {
                        let _ = tx.send(result);
                    },
                );
                // No answer (page busy or still loading) counts as used.
                asked.is_ok()
                    && rx
                        .recv_timeout(Duration::from_secs(2))
                        .is_ok_and(|result| result == "false")
            })
            .collect();
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            // The user may have brought the window back while we asked.
            if runtime(&handle).show_pending.load(Ordering::Relaxed)
                || main_window(&handle)
                    .and_then(|w| w.is_visible().map_err(err))
                    .unwrap_or(true)
            {
                return;
            }
            if !unused.is_empty() {
                for provider in unused {
                    let _ = close_view(&handle, provider);
                }
                changed(&handle);
            }
            release_main(&handle);
        });
    });
}
fn toggle(app: &AppHandle) {
    if runtime(app).show_pending.load(Ordering::Relaxed) {
        hide(app);
        return;
    }
    match main_window(app) {
        Ok(window) if window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false) => {
            hide(app)
        }
        _ => show(app),
    }
}

fn close_view(app: &AppHandle, provider: Provider) -> Result<(), String> {
    if let Some(view) = app.get_webview(provider.label()) {
        // Queued before the close, so it still sees the live page.
        #[cfg(target_os = "macos")]
        let _ = view.with_webview(|webview| {
            resign_focus(webview.inner());
            close_page(webview.inner());
        });
        view.close().map_err(err)?;
    }
    // Popups belong to this session and must not outlive it.
    let prefix = format!("auth-{}-", provider.label());
    for (label, window) in app.webview_windows() {
        if label.starts_with(&prefix) {
            #[cfg(target_os = "macos")]
            let _ = window.with_webview(|webview| close_page(webview.inner()));
            window.destroy().map_err(err)?;
        }
    }
    Ok(())
}
fn close_all(app: &AppHandle) {
    for provider in PROVIDERS {
        let _ = close_view(app, provider);
    }
    *runtime(app).active.lock().unwrap() = None;
    changed(app);
}

/// A window keeps its focused view alive after the view is closed, and with it
/// the page's WebContent process. Move focus off a page before closing it.
#[cfg(target_os = "macos")]
fn resign_focus(webview: *mut std::ffi::c_void) {
    use objc2_app_kit::NSView;
    // wry's WKWebView is an NSView, and with_webview runs on the main thread.
    let Some(webview) = (unsafe { webview.cast::<NSView>().as_ref() }) else {
        return;
    };
    let Some(window) = webview.window() else {
        return;
    };
    let focused = window.firstResponder().and_then(|r| r.downcast::<NSView>().ok());
    if focused.is_some_and(|view| view.isDescendantOf(webview)) {
        window.makeFirstResponder(None);
    }
}
/// AppKit holds other references to a removed view too: a destroyed window
/// leaks and keeps the last scrolled view in `_latchedViewForScrollEvents`.
/// Close the page itself so WebKit ends its WebContent process even if the
/// WKWebView object is never freed. `_close` is WebKit SPI, as used by Safari.
#[cfg(target_os = "macos")]
fn close_page(webview: *mut std::ffi::c_void) {
    use objc2::{msg_send, runtime::NSObject, sel};
    let Some(webview) = (unsafe { webview.cast::<NSObject>().as_ref() }) else {
        return;
    };
    let responds: bool = unsafe { msg_send![webview, respondsToSelector: sel!(_close)] };
    if responds {
        let _: () = unsafe { msg_send![webview, _close] };
    }
}
fn allowed_navigation(url: &tauri::Url) -> bool {
    // about:srcdoc only ever names an iframe's inline content.
    if matches!(url.as_str(), "about:blank" | "about:srcdoc") {
        return true;
    }
    if url.scheme() != "https" {
        return false;
    }
    let host = url.host_str().unwrap_or_default();
    // Google sign-in uses regional account frames as well as accounts.google.com.
    // Match the exact accounts host on every published Google domain, including
    // multi-label suffixes such as google.co.uk and google.com.au.
    if host
        .strip_prefix("accounts.")
        .is_some_and(|domain| GOOGLE_DOMAINS.lines().any(|known| known == domain))
    {
        return true;
    }
    // Google's password step can depend on reCAPTCHA subframes. The native
    // navigation callback also filters iframes, so allowing accounts.google.com
    // alone is insufficient. Keep these exceptions limited to reCAPTCHA paths.
    // https://developers.google.com/recaptcha/docs/faq
    if matches!(host, "www.google.com" | "www.recaptcha.net")
        && url.path().starts_with("/recaptcha/")
    {
        return true;
    }
    [
        "chatgpt.com",
        "openai.com",
        "claude.ai",
        "claude.com",
        "anthropic.com",
        "appleid.apple.com",
        "account.apple.com",
        "login.microsoftonline.com",
        "login.live.com",
        // Bot-check iframes. wry runs this filter on subframes too, so
        // blocking these can prevent authentication from completing.
        "hcaptcha.com",
        "challenges.cloudflare.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

fn navigation_origin(url: &tauri::Url) -> String {
    // Never log paths, queries, fragments, or userinfo: auth URLs carry tokens.
    let origin = url.origin().ascii_serialization();
    if origin == "null" {
        format!("{}:", url.scheme())
    } else {
        origin
    }
}

fn filter_navigation(app: &AppHandle, provider: Provider, url: &tauri::Url) -> bool {
    if allowed_navigation(url) {
        return true;
    }
    let time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let message = format!(
        "{time} {}: blocked navigation to {}\n",
        provider.label(),
        navigation_origin(url)
    );
    eprint!("{message}");
    // Bundled apps usually have no terminal. Keep a bounded, origin-only log
    // so a failed sign-in can be diagnosed without collecting credentials.
    if let Ok(dir) = app.path().app_log_dir() {
        if fs::create_dir_all(&dir).is_ok() {
            if let Ok(mut file) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("navigation.log"))
            {
                if file.metadata().is_ok_and(|metadata| metadata.len() >= 64 * 1024) {
                    let _ = file.set_len(0);
                }
                let _ = file.write_all(message.as_bytes());
            }
        }
    }
    false
}

/// WKWebView's default user agent has no Safari token, so providers can't
/// tell it apart from an unknown browser. Present as the Safari that ships
/// this WebKit, in Safari's frozen user-agent format.
#[cfg(target_os = "macos")]
fn safari_user_agent() -> String {
    use objc2_foundation::{NSBundle, NSString};
    let version = NSBundle::bundleWithPath(&NSString::from_str("/Applications/Safari.app"))
        .and_then(|bundle| {
            bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))
        })
        .and_then(|version| version.downcast::<NSString>().ok())
        .map(|version| version.to_string())
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .unwrap_or_else(|| "18.6".into());
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
         (KHTML, like Gecko) Version/{version} Safari/605.1.15"
    )
}
/// AppKit still offers drags to a hidden view, so a file dropped on the shown
/// chat can land in the hidden one. Withdraw a hidden page's drag types and
/// give WebKit's types back when it is shown again.
#[cfg(target_os = "macos")]
fn set_drop_target(webview: *mut std::ffi::c_void, enabled: bool) {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSPasteboardType, NSView};
    use objc2_foundation::NSArray;
    use std::cell::RefCell;
    thread_local! {
        // Every WKWebView registers the same types. with_webview runs on the
        // main thread, so this is only touched there.
        static TYPES: RefCell<Option<Retained<NSArray<NSPasteboardType>>>> = const { RefCell::new(None) };
    }
    let Some(webview) = (unsafe { webview.cast::<NSView>().as_ref() }) else {
        return;
    };
    let registered = webview.registeredDraggedTypes();
    if !registered.is_empty() {
        TYPES.with_borrow_mut(|types| *types = Some(registered.clone()));
    }
    if !enabled {
        webview.unregisterDraggedTypes();
    } else if registered.is_empty() {
        TYPES.with_borrow(|types| {
            if let Some(types) = types {
                webview.registerForDraggedTypes(types);
            }
        });
    }
}

fn create_view(app: &AppHandle, provider: Provider, url: &str) -> Result<(), String> {
    let window = main_window(app)?;
    let (position, size) = provider_bounds(app)?;
    let popup_app = app.clone();
    let navigation_app = app.clone();
    let mut builder = WebviewBuilder::new(
        provider.label(),
        WebviewUrl::External(url.parse().map_err(err)?),
    )
    .initialization_script(FOCUS_SCRIPT)
    // Tauri's handler claims every file drop to emit its own event, so the
    // page never receives it. Let WebKit deliver drops to the provider.
    .disable_drag_drop_handler()
    .on_navigation(move |url| filter_navigation(&navigation_app, provider, url))
    .on_new_window(move |url, features| {
        if !filter_navigation(&popup_app, provider, &url) {
            return NewWindowResponse::Deny;
        }
        let label = format!(
            "auth-{}-{}",
            provider.label(),
            POPUP_ID.fetch_add(1, Ordering::SeqCst)
        );
        let navigation_app = popup_app.clone();
        let mut popup = tauri::WebviewWindowBuilder::new(
            &popup_app,
            label,
            WebviewUrl::External("about:blank".parse().unwrap()),
        )
        .title("Kreide · Sign in")
        .window_features(features)
        .disable_drag_drop_handler()
        .on_navigation(move |url| filter_navigation(&navigation_app, provider, url))
        .initialization_script(FOCUS_SCRIPT);
        #[cfg(target_os = "macos")]
        {
            popup = popup
                .data_store_identifier(provider.store())
                .user_agent(&safari_user_agent());
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Ok(path) = profile_dir(&popup_app, provider) {
                popup = popup.data_directory(path);
            }
        }
        match popup.build() {
            Ok(window) => NewWindowResponse::Create { window },
            Err(_) => NewWindowResponse::Deny,
        }
    });
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .data_store_identifier(provider.store())
            .user_agent(&safari_user_agent());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let path = profile_dir(app, provider)?;
        fs::create_dir_all(&path).map_err(err)?;
        builder = builder.data_directory(path);
    }
    window.add_child(builder, position, size).map_err(err)?;
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn profile_dir(app: &AppHandle, provider: Provider) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(err)?
        .join("profiles")
        .join(provider.label()))
}


/// Show a provider, creating its page if needed. The other provider's chat
/// stays loaded in the background so switching back is instant.
fn select(app: &AppHandle, provider: Provider) -> Result<(), String> {
    if app.get_webview(provider.label()).is_none() {
        create_view(app, provider, provider.url())?;
    }
    for other in PROVIDERS {
        if let Some(view) = app.get_webview(other.label()) {
            if other == provider {
                view.show().map_err(err)?;
            } else {
                view.hide().map_err(err)?;
            }
            #[cfg(target_os = "macos")]
            let _ = view.with_webview(move |webview| set_drop_target(webview.inner(), other == provider));
        }
    }
    *runtime(app).active.lock().unwrap() = Some(provider);
    fit(app);
    changed(app);
    focus_composer(app);
    Ok(())
}
/// Replace the active page with a brand-new temporary chat. A new webview
/// (rather than a reload) avoids reusing any in-page session state.
fn new_chat(app: &AppHandle) -> Result<(), String> {
    let provider = runtime(app)
        .active
        .lock()
        .unwrap()
        .unwrap_or_else(|| runtime(app).preferences.lock().unwrap().default_provider);
    close_view(app, provider)?;
    select(app, provider)
}

#[tauri::command]
fn get_state(app: AppHandle, webview: tauri::Webview) -> Result<Snapshot, String> {
    trusted(&webview)?;
    Ok(snapshot(&app))
}
#[tauri::command]
async fn boot(app: AppHandle, webview: tauri::Webview, viewport: Viewport) -> Result<Snapshot, String> {
    trusted(&webview)?;
    update_viewport(&app, viewport)?;
    #[cfg(target_os = "macos")]
    {
        let state = runtime(&app);
        let mut inset = state.toolbar_inset.lock().unwrap();
        if inset.is_none() {
            let scale = main_window(&app)?.scale_factor().map_err(err)?;
            let native = webview.size().map_err(err)?.to_logical::<f64>(scale);
            *inset = Some((native.height - viewport.height).max(0.0));
        }
    }
    // Reopen the last provider (or the default after a fresh start) if its
    // page was unloaded while hidden. On macOS `present_main` defers this to
    // us for a rebuilt toolbar, so a remembered but closed provider must open.
    let provider = runtime(&app)
        .active
        .lock()
        .unwrap()
        .unwrap_or_else(|| runtime(&app).preferences.lock().unwrap().default_provider);
    if app.get_webview(provider.label()).is_none() {
        select(&app, provider)?;
    }
    fit(&app);
    let result = snapshot(&app);
    // Report the startup warning once, not on every window rebuild.
    *runtime(&app).warning.lock().unwrap() = None;
    Ok(result)
}
#[tauri::command]
async fn resize_provider(
    app: AppHandle,
    webview: tauri::Webview,
    viewport: Viewport,
) -> Result<(), String> {
    trusted(&webview)?;
    update_viewport(&app, viewport)?;
    fit(&app);
    Ok(())
}
#[tauri::command]
async fn select_provider(
    app: AppHandle,
    webview: tauri::Webview,
    provider: Provider,
) -> Result<(), String> {
    trusted(&webview)?;
    select(&app, provider)
}
#[tauri::command]
async fn start_new_chat(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    trusted(&webview)?;
    new_chat(&app)
}
#[tauri::command]
async fn open_settings(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    trusted(&webview)?;
    show_settings(&app).map_err(err)
}
#[tauri::command]
async fn reset_provider(
    app: AppHandle,
    webview: tauri::Webview,
    provider: Provider,
) -> Result<Snapshot, String> {
    trusted(&webview)?;
    let was_active = *runtime(&app).active.lock().unwrap() == Some(provider);
    if let Some(view) = app.get_webview(provider.label()) {
        view.clear_all_browsing_data().map_err(err)?;
    }
    close_view(&app, provider)?;
    #[cfg(target_os = "macos")]
    {
        // The store can only be removed once no webview uses it.
        let _ = app.remove_data_store(provider.store()).await;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = fs::remove_dir_all(profile_dir(&app, provider)?);
    }
    // Without a main window, `show` reopens the provider when summoned.
    if was_active && main_window(&app).is_ok() {
        *runtime(&app).active.lock().unwrap() = None;
        select(&app, provider)?;
    }
    changed(&app);
    Ok(snapshot(&app))
}
#[tauri::command]
async fn save_preferences(
    app: AppHandle,
    webview: tauri::Webview,
    preferences: Preferences,
) -> Result<Snapshot, String> {
    trusted(&webview)?;
    if preferences.fresh_after_minutes > 24 * 60 && preferences.fresh_after_minutes != FRESH_IMMEDIATELY {
        return Err("Fresh-chat delay is too long".into());
    }
    let old = runtime(&app).preferences.lock().unwrap().clone();
    let apply = |prefs: &Preferences| -> Result<(), String> {
        // A released main window picks this up when it is rebuilt.
        if let Ok(window) = main_window(&app) {
            window.set_always_on_top(prefs.always_on_top).map_err(err)?;
        }
        if prefs.shortcut_enabled && !app.global_shortcut().is_registered(SHORTCUT) {
            app.global_shortcut().register(SHORTCUT).map_err(err)?;
        }
        if !prefs.shortcut_enabled && app.global_shortcut().is_registered(SHORTCUT) {
            app.global_shortcut().unregister(SHORTCUT).map_err(err)?;
        }
        Ok(())
    };
    let persist = || -> Result<(), String> {
        apply(&preferences)?;
        let dir = app.path().app_config_dir().map_err(err)?;
        fs::create_dir_all(&dir).map_err(err)?;
        fs::write(
            dir.join("preferences.json.tmp"),
            serde_json::to_vec_pretty(&preferences).map_err(err)?,
        )
        .map_err(err)?;
        fs::rename(
            dir.join("preferences.json.tmp"),
            dir.join("preferences.json"),
        )
        .map_err(err)?;
        Ok(())
    };
    if let Err(error) = persist() {
        let _ = apply(&old);
        return Err(error);
    }
    *runtime(&app).preferences.lock().unwrap() = preferences;
    *runtime(&app).warning.lock().unwrap() = None;
    changed(&app);
    Ok(snapshot(&app))
}

fn show_settings(app: &AppHandle) -> tauri::Result<()> {
    set_background(app, false);
    if let Some(window) = app.get_webview_window("settings") {
        window.show()?;
        return window.set_focus();
    }
    tauri::WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html#settings".into()))
        .title("Settings")
        .inner_size(440.0, 640.0)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .center()
        .build()?;
    Ok(())
}

fn handle_menu(app: &AppHandle, id: &str) {
    let result = match id {
        "show" => {
            show(app);
            Ok(())
        }
        "new" => {
            show(app);
            new_chat(app)
        }
        "chatgpt" => {
            show(app);
            select(app, Provider::Chatgpt)
        }
        "claude" => {
            show(app);
            select(app, Provider::Claude)
        }
        "reload" => runtime(app)
            .active
            .lock()
            .unwrap()
            .and_then(|p| app.get_webview(p.label()))
            .map_or(Ok(()), |view| view.reload().map_err(err)),
        "settings" => show_settings(app).map_err(err),
        "hide" => {
            hide(app);
            Ok(())
        }
        "end" => {
            end_chat_and_close(app);
            Ok(())
        }
        "quit" => {
            app.exit(0);
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        let _ = app.emit_to("main", "error", e);
    }
}

fn app_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let sep = || PredefinedMenuItem::separator(app);
    let item = |id: &str, text: &str, accel: &str| MenuItem::with_id(app, id, text, true, Some(accel));
    let app_sub = Submenu::with_items(
        app,
        "Kreide",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Kreide"), None)?,
            &sep()?,
            &item("settings", "Settings…", "CmdOrCtrl+,")?,
            &sep()?,
            &item("hide", "Hide Kreide", "CmdOrCtrl+H")?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &sep()?,
            &item("quit", "Quit Kreide", "CmdOrCtrl+Q")?,
        ],
    )?;
    // Without an Edit menu, macOS webviews lose copy/paste shortcuts.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let chat = Submenu::with_items(
        app,
        "Chat",
        true,
        &[
            &item("new", "New Temporary Chat", "CmdOrCtrl+N")?,
            &item("end", "End Chat and Close", "CmdOrCtrl+Shift+W")?,
            &sep()?,
            &item("chatgpt", "ChatGPT", "CmdOrCtrl+1")?,
            &item("claude", "Claude", "CmdOrCtrl+2")?,
            &sep()?,
            &item("reload", "Reload Page", "CmdOrCtrl+R")?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::close_window(app, None)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&app_sub, &edit, &chat, &window])
}

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Visibility stays under the menu-bar app's control.
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION,
                )
                .with_filter(|label| label == "main")
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_state,
            boot,
            resize_provider,
            select_provider,
            start_new_chat,
            open_settings,
            reset_provider,
            save_preferences
        ])
        .menu(app_menu)
        .on_menu_event(|app, event| handle_menu(app, event.id.as_ref()))
        .setup(|app| {
            let preferences = app
                .path()
                .app_config_dir()
                .ok()
                .and_then(|p| fs::read(p.join("preferences.json")).ok())
                .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
                .unwrap_or_default();
            let mut warning = None;
            if preferences.shortcut_enabled {
                if let Err(e) = app.global_shortcut().register(SHORTCUT) {
                    warning = Some(format!(
                        "Global shortcut unavailable: {e}. You can still use the tray."
                    ));
                }
            }
            app.get_window("main")
                .unwrap()
                .set_always_on_top(preferences.always_on_top)?;
            app.manage(Runtime {
                #[cfg(target_os = "macos")]
                toolbar_inset: Mutex::new(None),
                viewport: Mutex::new(Viewport {
                    width: 1236.0,
                    height: 852.0,
                }),
                preferences: Mutex::new(preferences),
                active: Mutex::new(None),
                hidden_at: Mutex::new(None),
                show_revision: AtomicU64::new(0),
                show_pending: AtomicBool::new(false),
                frame: Mutex::new(None),
                warning: Mutex::new(warning),
            });
            #[cfg(target_os = "macos")]
            macos_clipboard::install(app.handle().clone());
            // Unload idle chats in the background so a hidden app costs little.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                let hidden = !main_window(&handle)
                    .and_then(|w| w.is_visible().map_err(err))
                    .unwrap_or(true);
                if hidden && stale(&handle) && runtime(&handle).active.lock().unwrap().is_some() {
                    let app = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        close_all(&app);
                        release_main(&app);
                    });
                }
            });
            let menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "show", "Open Kreide", true, None::<&str>)?,
                    &MenuItem::with_id(app, "new", "New Temporary Chat", true, None::<&str>)?,
                    &MenuItem::with_id(app, "end", "End Chat and Close", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "chatgpt", "ChatGPT", true, None::<&str>)?,
                    &MenuItem::with_id(app, "claude", "Claude", true, None::<&str>)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", "Quit Kreide", true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(
                    include_bytes!("../icons/tray.rgba").to_vec(),
                    44,
                    44,
                ))
                .icon_as_template(true)
                .tooltip("Kreide")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "settings" {
                if let tauri::WindowEvent::Destroyed = event {
                    if !any_window_visible(window.app_handle()) {
                        set_background(window.app_handle(), true);
                    }
                }
                return;
            }
            if window.label() != "main" {
                return;
            }
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    hide(window.app_handle());
                }
                tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                    fit(window.app_handle())
                }
                _ => (),
            }
        })
        .build(tauri::generate_context!())
        .expect("Could not start Kreide")
        .run(|app, event| {
            // Releasing the hidden main window must not quit the menu-bar app;
            // only an explicit exit (Quit) carries a code.
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = &event {
                api.prevent_exit();
            }
            // Clicking the Dock icon brings the window back.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = app;
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_sits_below_toolbar_and_titlebar_inset() {
        let (position, size) = content_bounds(
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(960.0, 760.0),
            Viewport {
                width: 960.0,
                height: 732.0,
            },
        );
        assert_eq!(position.y, 28.0 + TOOLBAR);
        assert_eq!(position.y + size.height, 760.0);
    }
    #[test]
    fn child_fills_window_below_toolbar_without_inset() {
        let (position, size) = content_bounds(
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(960.0, 760.0),
            Viewport {
                width: 960.0,
                height: 760.0,
            },
        );
        assert_eq!(position.y, TOOLBAR);
        assert_eq!(size.height, 760.0 - TOOLBAR);
    }
    #[test]
    fn toolbar_and_chat_do_not_overlap_after_resize() {
        for (width, height, inset) in [
            (960.0, 760.0, 28.0),
            (1440.0, 900.0, 28.0),
            (800.0, 600.0, 0.0),
        ] {
            let window = LogicalSize::new(width, height);
            let toolbar = toolbar_size(window, inset, true);
            let (position, chat) = content_bounds(
                LogicalPosition::new(0.0, 0.0),
                window,
                Viewport {
                    width,
                    height: height - inset,
                },
            );
            assert_eq!(toolbar.height, position.y);
            assert_eq!(toolbar.width, chat.width);
            assert_eq!(position.y + chat.height, height);
            // A loading failure needs the full local error/retry page.
            assert_eq!(toolbar_size(window, inset, false), window);
        }
    }
    #[test]
    fn navigation_is_https_and_domain_scoped() {
        for url in [
            "https://chatgpt.com/",
            "https://auth.openai.com/",
            "https://claude.ai/new?incognito",
            "https://accounts.google.com/",
            "https://newassets.hcaptcha.com/captcha/v1/abc/static/hcaptcha.html#frame=checkbox-invisible",
            "https://challenges.cloudflare.com/turnstile/v0/iframe",
            "about:srcdoc",
        ] {
            assert!(allowed_navigation(&url.parse().unwrap()));
        }
        for url in [
            "http://chatgpt.com",
            "https://chatgpt.com.evil.test",
            "https://evilclaude.ai",
            "https://hcaptcha.com.evil.test/",
            "https://challenges.cloudflare.com.evil.test/",
            "https://unrelated.cloudflare.com/",
            "https://accounts.google.com.evil.test/",
            "https://evilaccounts.google.com/",
            "https://www.google.com/",
            "http://accounts.google.com/",
            "file:///etc/passwd",
            "tauri://localhost",
        ] {
            assert!(!allowed_navigation(&url.parse().unwrap()));
        }
    }
    #[test]
    fn google_accounts_support_every_published_region_with_exact_hosts() {
        for domain in GOOGLE_DOMAINS.lines() {
            let allowed = format!("https://accounts.{domain}/CheckConnection?test=1");
            assert!(allowed_navigation(&allowed.parse().unwrap()), "{allowed}");
            for blocked in [
                format!("http://accounts.{domain}/"),
                format!("https://accounts.{domain}.evil.test/"),
                format!("https://evilaccounts.{domain}/"),
                format!("https://evil.accounts.{domain}/"),
                format!("https://www.{domain}/"),
                format!("https://{domain}/"),
            ] {
                assert!(!allowed_navigation(&blocked.parse().unwrap()), "{blocked}");
            }
        }
        // Explicit regressions for different regional suffix shapes.
        for region in ["de", "fr", "co.uk", "com.au", "co.jp", "co.in", "com.br"] {
            let url = format!("https://accounts.google.{region}/");
            assert!(allowed_navigation(&url.parse().unwrap()), "{url}");
        }
        for url in [
            "https://accounts.google.evil/",
            "https://accounts.google.invalid/",
            "https://accounts.google.com@evil.test/",
        ] {
            assert!(!allowed_navigation(&url.parse().unwrap()), "{url}");
        }
    }
    #[test]
    fn google_verification_is_limited_to_https_recaptcha_paths() {
        for host in ["www.google.com", "www.recaptcha.net"] {
            for path in [
                "/recaptcha/api2/anchor?k=test",
                "/recaptcha/api2/bframe",
                "/recaptcha/enterprise/anchor?k=test",
                "/recaptcha/enterprise/bframe",
            ] {
                let url = format!("https://{host}{path}").parse().unwrap();
                assert!(allowed_navigation(&url), "{url}");
            }
        }
        for url in [
            "http://www.google.com/recaptcha/api2/anchor",
            "https://www.google.com/",
            "https://www.google.com/search?q=recaptcha",
            "https://www.google.com/recaptcha-evil/anchor",
            "https://www.google.com/recaptcha/../search",
            "https://www.google.com.evil.test/recaptcha/api2/anchor",
            "https://www.recaptcha.net.evil.test/recaptcha/api2/anchor",
            "https://www.recaptcha.net/other",
            "https://evil.test/recaptcha/api2/anchor",
        ] {
            assert!(!allowed_navigation(&url.parse().unwrap()), "{url}");
        }
    }
    #[test]
    fn navigation_diagnostics_exclude_sign_in_secrets() {
        let url = "https://user:password@blocked.test/private-token?code=secret#token"
            .parse()
            .unwrap();
        assert_eq!(navigation_origin(&url), "https://blocked.test");
        for (url, expected) in [
            ("file:///private/token", "file:"),
            ("data:text/plain,secret", "data:"),
            ("about:blank#secret", "about:"),
        ] {
            assert_eq!(navigation_origin(&url.parse().unwrap()), expected);
        }
    }
    #[test]
    fn provider_profiles_are_distinct() {
        assert_ne!(Provider::Chatgpt.store(), Provider::Claude.store());
        assert_ne!(Provider::Chatgpt.label(), Provider::Claude.label());
    }
    #[test]
    fn preferences_have_no_conversation_state() {
        let json = serde_json::to_value(Preferences::default()).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 4);
        assert!(serde_json::from_str::<Preferences>(r#"{"defaultProvider":"chatgpt","alwaysOnTop":false,"shortcutEnabled":true,"url":"https://chatgpt.com/c/123"}"#).is_err());
    }
    #[test]
    fn old_preferences_files_still_load() {
        let prefs: Preferences = serde_json::from_str(
            r#"{"defaultProvider":"claude","alwaysOnTop":false,"shortcutEnabled":true}"#,
        )
        .unwrap();
        assert_eq!(prefs.fresh_after_minutes, 15);
    }
}
