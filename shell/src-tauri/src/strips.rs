//! Strip windows, keyed by monitor from the start (Milestone 8 adds one per monitor
//! that hosts a sheep). In Milestone 3 every sheep lives on the primary monitor, so
//! there is exactly one strip.
//!
//! A strip is borderless, transparent, always on top, not focusable, absent from the
//! Dock and the window switcher, on every Space but not on full-screen Spaces (no
//! fullScreenAuxiliary), and sits directly above the Dock: it fills the bottom
//! `strip.heightPx` of the monitor's work area, which already excludes the Dock and
//! the menu bar.
//!
//! On Windows (Windows plan W1) the same window is placed in physical pixels on the work
//! area's bottom edge, which is the taskbar's top edge (WD1, WD3), is made a tool window
//! so it is in neither the taskbar nor Alt-Tab (WD2), and is put back to its frame when
//! its scale changes.

use std::collections::HashMap;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Monitor, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::desktop::{PhysRect, Space, TaskbarEdge};
use crate::log::log;

#[derive(Default)]
pub struct Strips {
    /// monitor name -> window label
    pub by_monitor: std::sync::Mutex<HashMap<String, String>>,
    /// The next window index (labels are never reused).
    pub next: std::sync::atomic::AtomicUsize,
    /// Strips whose page has asked for its first snapshot, so it can draw a carried sheep
    /// (D10): until then the pressed strip keeps drawing it, at its edge.
    pub ready: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Windows: the taskbar edge last logged per monitor (WD1).
    pub edges: std::sync::Mutex<HashMap<String, TaskbarEdge>>,
    /// Windows: the on-top lines' rate limit (deviation W1-15).
    pub on_top: std::sync::Mutex<crate::desktop::OnTopLog>,
}

impl Strips {
    pub fn monitor_of(&self, label: &str) -> Option<String> {
        self.by_monitor.lock().unwrap().iter().find(|(_, l)| l.as_str() == label).map(|(m, _)| m.clone())
    }

    pub fn is_ready(&self, label: &str) -> bool {
        self.ready.lock().unwrap().contains(label)
    }
}

/// Closes the strip on a monitor that no longer hosts a sheep.
pub fn close<R: Runtime>(app: &AppHandle<R>, monitor: &str) {
    let label = app.state::<Strips>().by_monitor.lock().unwrap().remove(monitor);
    if let Some(label) = label {
        app.state::<crate::clickthrough::Hitboxes>().by_window.lock().unwrap().remove(&label);
        app.state::<Strips>().ready.lock().unwrap().remove(&label);
        // A gesture held by this window ends with it.
        let brain = app.state::<std::sync::Arc<crate::brain::Brain>>();
        app.state::<crate::clickthrough::Drag>().cancel(&brain, "its strip closed", Some(&label));
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

/// A monitor's rectangle and work area in physical pixels, as tao reads them (Windows:
/// `GetMonitorInfo`, fresh on every call).
fn physical_areas(monitor: &Monitor) -> (PhysRect, PhysRect) {
    let (p, s, wa) = (monitor.position(), monitor.size(), monitor.work_area());
    (PhysRect { x: p.x, y: p.y, w: s.width, h: s.height }, PhysRect { x: wa.position.x, y: wa.position.y, w: wa.size.width, h: wa.size.height })
}

/// Windows: the strip's frame on one monitor, in physical pixels (WD1, WD3).
pub fn physical_rect(monitor: &Monitor, height_px: f64) -> PhysRect {
    crate::desktop::physical_strip_rect(physical_areas(monitor).1, monitor.scale_factor(), height_px)
}

/// The edge a monitor's work area is inset from: where its taskbar is (WD1).
pub fn taskbar_edge_of(monitor: &Monitor) -> TaskbarEdge {
    let (screen, work) = physical_areas(monitor);
    crate::desktop::taskbar_edge(screen.frame(), work.frame())
}

/// Logs a monitor's taskbar edge when it is new or has changed (Windows only).
fn note_edge<R: Runtime>(app: &AppHandle<R>, name: &str, monitor: &Monitor) {
    let edge = taskbar_edge_of(monitor);
    let before = app.state::<Strips>().edges.lock().unwrap().insert(name.to_string(), edge);
    if before != Some(edge) {
        let (screen, work) = physical_areas(monitor);
        let was = before.map(|b| format!(", was {}", b.as_str())).unwrap_or_default();
        log(&format!(
            "taskbar on monitor {name:?}: {}{was} (screen x {} y {} {}x{}, work area x {} y {} {}x{}, physical px, scale {})",
            edge.as_str(), screen.x, screen.y, screen.w, screen.h, work.x, work.y, work.w, work.h, monitor.scale_factor()
        ));
    }
}

/// Puts a strip window at a physical frame, size first (`set_size` is the inner size;
/// `set_position` the outer position, which for a strip, undecorated, is the same place).
fn set_physical<R: Runtime>(window: &tauri::WebviewWindow<R>, r: PhysRect) -> tauri::Result<()> {
    window.set_size(PhysicalSize::new(r.w, r.h))?;
    window.set_position(PhysicalPosition::new(r.x, r.y))
}

/// Whether a strip window is at a physical frame now, read the way `set_physical` sets it.
fn at_frame<R: Runtime>(window: &tauri::WebviewWindow<R>, r: PhysRect) -> bool {
    match (window.outer_position(), window.inner_size()) {
        (Ok(pos), Ok(size)) => r.matches((pos.x, pos.y), (size.width, size.height)),
        _ => true,
    }
}

/// Moves every strip to where the Dock's settings (macOS) or its monitor's work area
/// (Windows) say it belongs now (main thread). Only a strip whose frame differs is
/// touched; the size changes only with the work area's width, so the sheep keep their
/// slots. `only`: one strip (a scale change), else all.
pub fn follow_dock<R: Runtime>(app: &AppHandle<R>, height_px: f64, dock: crate::dock::DockPrefs) {
    follow(app, height_px, dock, None);
}

pub fn follow<R: Runtime>(app: &AppHandle<R>, height_px: f64, dock: crate::dock::DockPrefs, only: Option<&str>) {
    let by_monitor = app.state::<Strips>().by_monitor.lock().unwrap().clone();
    let monitors = app.available_monitors().unwrap_or_default();
    for (name, label) in by_monitor {
        if only.is_some_and(|l| l != label) {
            continue;
        }
        let (Some(window), Some(monitor)) = (app.get_webview_window(&label), monitors.iter().find(|m| monitor_name(m) == name)) else { continue };
        if Space::NATIVE == Space::Physical {
            note_edge(app, &name, monitor);
            let r = physical_rect(monitor, height_px);
            if at_frame(&window, r) {
                continue;
            }
            let _ = set_physical(&window, r);
            let why = if only.is_some() { "after a scale change".to_string() } else { format!("for the taskbar ({})", taskbar_edge_of(monitor).as_str()) };
            log(&format!("{label}: moved {why}: x {} y {} {}x{} physical px at scale {}", r.x, r.y, r.w, r.h, monitor.scale_factor()));
            continue;
        }
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

/// Windows: puts every strip back on top (`HWND_TOPMOST`, never activating it) and logs,
/// at most once a minute per strip, when that was needed (deviation W1-15). Main thread:
/// from the 2 s re-read and from the foreground-change hook. The locks are only tried: the
/// hook can run while the main thread pumps messages, and the next pass catches up.
/// Nothing on macOS (its window level isn't dropped this way).
pub fn keep_on_top<R: Runtime>(app: &AppHandle<R>, why: &str) {
    if Space::NATIVE != Space::Physical {
        return;
    }
    let strips = app.state::<Strips>();
    let Ok(by_monitor) = strips.by_monitor.try_lock().map(|m| m.values().cloned().collect::<Vec<_>>()) else { return };
    for label in by_monitor {
        let Some(window) = app.get_webview_window(&label) else { continue };
        let Some(found) = put_on_top(&window) else { continue };
        let line = strips.on_top.try_lock().ok().and_then(|mut l| l.note(&label, found, why, std::time::Instant::now()));
        if let Some(line) = line {
            log(&line);
        }
    }
}

#[cfg(windows)]
fn put_on_top<R: Runtime>(window: &tauri::WebviewWindow<R>) -> Option<crate::desktop::OnTop> {
    window.hwnd().ok().map(|h| crate::win::keep_topmost(h.0 as _))
}

#[cfg(not(windows))]
fn put_on_top<R: Runtime>(_window: &tauri::WebviewWindow<R>) -> Option<crate::desktop::OnTop> {
    None
}

/// Whether a window is a strip (`strip-<n>`), not the onboarding window: only strips are
/// made click-through, hidden while sharing, and reloaded for a new pet.
pub fn is_strip(label: &str) -> bool {
    label.strip_prefix("strip-").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

pub fn monitor_name(m: &Monitor) -> String {
    m.name().cloned().unwrap_or_else(|| "primary".into())
}

/// WebView2 throttling (WD2 step 2, risk WR1): `background_throttling(Disabled)` is
/// unsupported on Windows, so the page may be throttled as hidden. W1 measures it with
/// `--heartbeat`; if it is, this turns on the browser-arguments fallback. Off until the
/// measurement says otherwise.
pub const WEBVIEW2_UNTHROTTLED: bool = false;

/// The WebView2 arguments for every webview of the process (strips and the setup window
/// alike, or they would need separate data directories): wry's defaults, which these
/// replace, merged with the fallback's into one `--disable-features`. None: wry's own.
pub fn webview2_args(unthrottled: bool) -> Option<String> {
    unthrottled.then(|| "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion --disable-background-timer-throttling".to_string())
}

pub fn open<R: Runtime>(app: &AppHandle<R>, monitor: &Monitor, height_px: f64, index: usize, capture_exclusion: bool, start_hidden: bool) -> tauri::Result<()> {
    let name = monitor_name(monitor);
    let physical = (Space::NATIVE == Space::Physical).then(|| physical_rect(monitor, height_px));
    let r = match physical {
        // The builder's logical frame is only a first guess; the physical one is set below,
        // before the window is shown.
        Some(p) => {
            let sc = crate::desktop::sane(monitor.scale_factor());
            StripRect { x: p.x as f64 / sc, y: p.y as f64 / sc, width: p.w as f64 / sc, height: p.h as f64 / sc }
        }
        None => strip_rect(monitor, height_px, crate::dock::read_prefs()),
    };
    let label = format!("strip-{index}");
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()));
    if let Some(args) = webview2_args(cfg!(windows) && WEBVIEW2_UNTHROTTLED) {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder
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
        log(&format!("{label}: capture exclusion on ({})", if cfg!(windows) { "WDA_EXCLUDEFROMCAPTURE: also absent from your own screenshots" } else { "sharingType none" }));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window.with_webview(|wv| {
            let applied = unsafe { mac::keep_page_visible(wv.inner()) };
            log(&format!("webview occlusion detection {}", if applied { "disabled: the page stays visible while the strip is on screen" } else { "could not be disabled (WebKit lacks the switch); the page may be throttled" }));
        });
    }
    #[cfg(windows)]
    match window.hwnd() {
        // Not in the taskbar (skip_taskbar), and with this not in Alt-Tab or Task View either.
        Ok(hwnd) => log(&format!("{label}: {}", crate::win::make_tool_window(hwnd.0 as _))),
        Err(e) => log(&format!("warning: {label}: no window handle ({e}); it may show in Alt-Tab")),
    }
    match physical {
        Some(p) => {
            set_physical(&window, p)?;
            // Moving onto a monitor of another scale changes the window's DPI on the way,
            // and Windows resizes it for that: once more puts it at its frame. (The 2 s
            // re-read would too.)
            if !at_frame(&window, p) {
                set_physical(&window, p)?;
                log(&format!("{label}: set again after the move to its monitor changed its scale"));
            }
            note_edge(app, &name, monitor);
            // A scale change (Settings, or the window crossing monitors) makes Windows
            // resize the strip to a size of its own: tao applies it with SetWindowPos inside
            // its WM_DPICHANGED handling, after this event. So the re-fit is posted from
            // another thread, which queues it for a later turn of the event loop
            // (`run_on_main_thread` from the main thread would run it at once, inside that
            // handling, and be overwritten). The 2 s re-read would put it right too.
            let (h, l) = (app.clone(), label.clone());
            window.on_window_event(move |e| {
                if let tauri::WindowEvent::ScaleFactorChanged { scale_factor, new_inner_size, .. } = e {
                    log(&format!("{l}: scale changed to {scale_factor}; Windows resizes it to {}x{}; it is put back to its frame after that", new_inner_size.width, new_inner_size.height));
                    let (h2, l2) = (h.clone(), l.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        let h3 = h2.clone();
                        let _ = h2.run_on_main_thread(move || follow(&h3, height_px, crate::dock::DockPrefs::default(), Some(&l2)));
                    });
                }
            });
        }
        None => {
            window.set_size(LogicalSize::new(r.width, r.height))?;
            window.set_position(LogicalPosition::new(r.x, r.y))?;
        }
    }
    if !start_hidden {
        window.show()?;
    }
    if let (Ok(p), Ok(sz), Ok(sc)) = (window.outer_position(), window.outer_size(), window.scale_factor()) {
        log(&format!("{label} actual frame: x {} y {} {}x{} physical px at scale {sc}", p.x, p.y, sz.width, sz.height));
    }
    match physical {
        Some(p) => log(&format!("strip {label} on monitor {name:?}: x {} y {} {}x{} (physical px), on the work area's bottom edge (the taskbar's top when it is at the bottom)", p.x, p.y, p.w, p.h)),
        None => log(&format!(
            "strip {label} on monitor {name:?}: x {} y {} {}x{} (logical px), work area excludes the Dock and menu bar",
            r.x, r.y, r.width, r.height
        )),
    }
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
