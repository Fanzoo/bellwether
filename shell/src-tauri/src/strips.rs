//! Strip windows, keyed by monitor from the start (Milestone 8 adds one per monitor
//! that hosts a sheep). In Milestone 3 every sheep lives on the primary monitor, so
//! there is exactly one strip.
//!
//! A strip is borderless, transparent, always on top, not focusable, absent from the
//! Dock and the window switcher, on every Space but not on full-screen Spaces (no
//! fullScreenAuxiliary), and sits directly above the Dock: it fills the bottom
//! `strip.heightPx` of the monitor's work area, which already excludes the Dock and
//! the menu bar.

use std::collections::HashMap;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Monitor, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::log::log;

#[derive(Default)]
pub struct Strips {
    /// monitor name -> window label
    pub by_monitor: std::sync::Mutex<HashMap<String, String>>,
    /// The next window index (labels are never reused).
    pub next: std::sync::atomic::AtomicUsize,
}

impl Strips {
    pub fn monitor_of(&self, label: &str) -> Option<String> {
        self.by_monitor.lock().unwrap().iter().find(|(_, l)| l.as_str() == label).map(|(m, _)| m.clone())
    }
}

/// Closes the strip on a monitor that no longer hosts a sheep.
pub fn close<R: Runtime>(app: &AppHandle<R>, monitor: &str) {
    let label = app.state::<Strips>().by_monitor.lock().unwrap().remove(monitor);
    if let Some(label) = label {
        app.state::<crate::clickthrough::Hitboxes>().by_window.lock().unwrap().remove(&label);
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.destroy();
        }
        log(&format!("strip {label} on monitor {monitor:?} closed: no sheep live there now"));
    }
}

pub struct StripRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The strip's frame, in logical pixels, for one monitor and the Dock's settings
/// (dock.rs: on the screen's bottom when a bottom Dock auto-hides).
pub fn strip_rect(monitor: &Monitor, height_px: f64, dock: crate::dock::DockPrefs) -> StripRect {
    let scale = monitor.scale_factor();
    let wa = monitor.work_area();
    let (p, s) = (monitor.position(), monitor.size());
    let screen = crate::dock::Frame { x: p.x as f64 / scale, y: p.y as f64 / scale, w: s.width as f64 / scale, h: s.height as f64 / scale };
    let work = crate::dock::Frame { x: wa.position.x as f64 / scale, y: wa.position.y as f64 / scale, w: wa.size.width as f64 / scale, h: wa.size.height as f64 / scale };
    let r = crate::dock::strip_rect_in(screen, work, dock, height_px);
    StripRect { x: r.x, y: r.y, width: r.w, height: r.h }
}

/// Moves every strip to where the Dock's settings say it belongs now (main thread).
/// Only a strip whose frame differs is touched; the size changes only with the work
/// area's width, so the sheep keep their slots.
pub fn follow_dock<R: Runtime>(app: &AppHandle<R>, height_px: f64, dock: crate::dock::DockPrefs) {
    let by_monitor = app.state::<Strips>().by_monitor.lock().unwrap().clone();
    let monitors = app.available_monitors().unwrap_or_default();
    for (name, label) in by_monitor {
        let (Some(window), Some(monitor)) = (app.get_webview_window(&label), monitors.iter().find(|m| monitor_name(m) == name)) else { continue };
        let r = strip_rect(monitor, height_px, dock);
        let (Ok(pos), Ok(size), Ok(sc)) = (window.outer_position(), window.outer_size(), window.scale_factor()) else { continue };
        let now = (pos.x as f64 / sc, pos.y as f64 / sc, size.width as f64 / sc, size.height as f64 / sc);
        if (now.0 - r.x).abs() < 0.5 && (now.1 - r.y).abs() < 0.5 && (now.2 - r.width).abs() < 0.5 && (now.3 - r.height).abs() < 0.5 {
            continue;
        }
        let _ = window.set_size(LogicalSize::new(r.width, r.height));
        let _ = window.set_position(LogicalPosition::new(r.x, r.y));
        log(&format!("{label}: moved for the Dock (autohide {}, {:?}): x {} y {} {}x{}", dock.autohide, dock.side, r.x, r.y, r.width, r.height));
    }
}

pub fn monitor_name(m: &Monitor) -> String {
    m.name().cloned().unwrap_or_else(|| "primary".into())
}

pub fn open<R: Runtime>(app: &AppHandle<R>, monitor: &Monitor, height_px: f64, index: usize, capture_exclusion: bool, start_hidden: bool) -> tauri::Result<()> {
    let name = monitor_name(monitor);
    let r = strip_rect(monitor, height_px, crate::dock::read_prefs());
    let label = format!("strip-{index}");
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title("bellwether")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true)
        // macOS judges a mostly transparent, click-through window "not visible", and
        // WebKit then throttles and suspends the page: the strip stopped updating a few
        // seconds after an alert appeared (measured, Milestone 5). The strip must keep
        // animating whenever it is on screen.
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .inner_size(r.width, r.height)
        .position(r.x, r.y)
        .visible(false)
        .build()?;
    // Start click-through before the window is ever shown.
    window.set_ignore_cursor_events(true)?;
    // Capture exclusion (privacy.captureExclusion): macOS NSWindow.sharingType = none.
    // One layer only: ScreenCaptureKit reportedly ignores it since macOS 15, so the
    // strip also hides while sharing, and the tray can hide it (Milestone 7).
    if capture_exclusion {
        window.set_content_protected(true)?;
        log(&format!("{label}: capture exclusion on (sharingType none)"));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window.with_webview(|wv| {
            let applied = unsafe { mac::keep_page_visible(wv.inner()) };
            log(&format!("webview occlusion detection {}", if applied { "disabled: the page stays visible while the strip is on screen" } else { "could not be disabled (WebKit lacks the switch); the page may be throttled" }));
        });
    }
    window.set_size(LogicalSize::new(r.width, r.height))?;
    window.set_position(LogicalPosition::new(r.x, r.y))?;
    if !start_hidden {
        window.show()?;
    }
    if let (Ok(p), Ok(sz), Ok(sc)) = (window.outer_position(), window.outer_size(), window.scale_factor()) {
        log(&format!("{label} actual frame: x {} y {} {}x{} physical px at scale {sc}", p.x, p.y, sz.width, sz.height));
    }
    log(&format!(
        "strip {label} on monitor {name:?}: x {} y {} {}x{} (logical px), work area excludes the Dock and menu bar",
        r.x, r.y, r.width, r.height
    ));
    app.state::<Strips>().by_monitor.lock().unwrap().insert(name, label);
    Ok(())
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_void, CString};

    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(name: *const std::os::raw::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    /// WebKit decides a page is hidden from macOS window occlusion, and a mostly
    /// transparent, click-through window like the strip counts as occluded, so the
    /// page was throttled and then suspended (Milestone 5 measurement: "page hidden"
    /// from the start, timers stopping). WKWebView's `_setWindowOcclusionDetectionEnabled:`
    /// (WebKit SPI, present on macOS 11+) turns that off. Called only if the view
    /// responds to it. Returns whether it was applied.
    pub unsafe fn keep_page_visible(webview: *mut c_void) -> bool {
        if webview.is_null() {
            return false;
        }
        let responds = CString::new("respondsToSelector:").unwrap();
        let setter = CString::new("_setWindowOcclusionDetectionEnabled:").unwrap();
        let sel_responds = sel_registerName(responds.as_ptr());
        let sel_setter = sel_registerName(setter.as_ptr());
        let send_sel: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> bool = std::mem::transmute(objc_msgSend as *const ());
        if !send_sel(webview, sel_responds, sel_setter) {
            return false;
        }
        let send_bool: extern "C" fn(*mut c_void, *mut c_void, bool) = std::mem::transmute(objc_msgSend as *const ());
        send_bool(webview, sel_setter, false);
        true
    }
}
