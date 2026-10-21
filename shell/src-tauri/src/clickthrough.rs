//! Click-through by cursor polling. Every strip window ignores
//! cursor events by default, so clicks fall through to whatever is underneath. A
//! thread polls the cursor at 30 Hz and hit-tests it against the sprite rectangles
//! each webview reports (`report_hitboxes`); only while the cursor is over a sprite
//! does that window accept the mouse.
//!
//! Known limit (Risk R6): a click that lands within one poll interval of the cursor
//! arriving on a sprite may fall through.
//!
//! Dragging a sheep (D10, milestone 12): from the webview's `press` until the gesture
//! ends, the pressed strip window accepts the mouse whatever its boxes say (the hold),
//! so a carried sheep can't outrun its box and make the window click-through mid-drag.
//! The hold ends on the webview's `release` or `cancel`, on the fail-safe (no mouse button
//! down for 250 ms: a lost release, a reloaded or crashed page), when its window closes,
//! and after 2 minutes. While a sheep is lifted the poll runs fast, maps the cursor to the
//! strip of the monitor under it, and publishes `carry` when that is another strip.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime};

use crate::log::log;

/// No mouse button down for this long ends a hold, as a cancel.
pub const FAILSAFE: Duration = Duration::from_millis(250);
/// No hold lasts longer than this.
pub const CEILING: Duration = Duration::from_secs(120);

/// The gesture under way: which strip window holds the mouse, for which sprite.
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    pub label: String,
    pub id: String,
    pub since: Instant,
    /// Past the threshold: a sheep is being carried (not a press that may yet be a click).
    pub lifted: bool,
    /// Cursor x − the sprite's left edge, and the sprite's width (logical px).
    pub grab_dx: f64,
    pub width: f64,
    /// The carried sheep's colour and alert kind, for another strip to draw it.
    pub colour: i64,
    pub alert: Option<String>,
    /// Since when no mouse button has been down (None: one is down).
    pub up_since: Option<Instant>,
}

impl Hold {
    pub fn new(label: &str, id: &str, now: Instant) -> Hold {
        Hold { label: label.into(), id: id.into(), since: now, lifted: false, grab_dx: 0.0, width: 0.0, colour: 0, alert: None, up_since: None }
    }

    /// One look at the mouse buttons: whether the hold must end now, and why. The
    /// fail-safe is no button down for `FAILSAFE`; the ceiling is `CEILING`.
    pub fn observe(&mut self, now: Instant, button_down: bool) -> Option<&'static str> {
        self.up_since = if button_down { None } else { Some(self.up_since.unwrap_or(now)) };
        if now.duration_since(self.since) >= CEILING {
            return Some("held for 2 minutes");
        }
        match self.up_since {
            Some(t) if now.duration_since(t) >= FAILSAFE => Some("no mouse button down for 250 ms"),
            _ => None,
        }
    }
}

/// Whether a strip window takes the mouse: the cursor is over one of its boxes, or the
/// window holds a press.
pub fn accepting(over_box: bool, held: bool) -> bool {
    over_box || held
}

/// Where the carried sheep is drawn when that isn't the pressed strip (D10).
#[derive(Clone, Debug, PartialEq)]
pub struct Carry {
    /// The strip that draws it, and the strip that holds the gesture.
    pub label: String,
    pub pressed: String,
    pub id: String,
    /// The cursor in the drawing strip (logical px).
    pub x: f64,
    pub y: f64,
    pub grab_dx: f64,
    pub colour: i64,
    pub alert: Option<String>,
}

/// The one gesture at a time, shared by `sprite_drag`, this poll and the placement loop.
#[derive(Default)]
pub struct Drag {
    pub hold: Mutex<Option<Hold>>,
    /// Bumped by each cancel the shell makes itself, so the pressed strip hears of it.
    pub cancels: std::sync::atomic::AtomicU64,
    /// Wakes the placement loop early (strips open at the lift).
    kick: (Mutex<bool>, Condvar),
}

impl Drag {
    /// Whether a sheep is being carried (every monitor gets a strip meanwhile).
    pub fn lifted(&self) -> bool {
        self.hold.lock().unwrap().as_ref().is_some_and(|h| h.lifted)
    }

    pub fn kick(&self) {
        *self.kick.0.lock().unwrap() = true;
        self.kick.1.notify_all();
    }

    /// Sleeps up to `d`, or until kicked.
    pub fn wait(&self, d: Duration) {
        let mut k = self.kick.0.lock().unwrap();
        if !*k {
            k = self.kick.1.wait_timeout(k, d).unwrap().0;
        }
        *k = false;
    }

    /// Ends the hold from the shell's side (the fail-safe, the ceiling, Quiet, Hide, a
    /// display change, the session ending, its window closing), and tells its strip.
    /// `only` limits it to one window's hold. Returns whether there was one.
    pub fn cancel(&self, brain: &crate::brain::Brain, reason: &str, only: Option<&str>) -> bool {
        let taken = {
            let mut h = self.hold.lock().unwrap();
            if h.as_ref().is_some_and(|h| only.is_none_or(|l| h.label == l)) {
                h.take()
            } else {
                None
            }
        };
        let Some(h) = taken else { return false };
        log(&format!("drag cancelled: {reason}{}", if h.lifted { String::new() } else { format!(" (sheep {} pressed, not lifted)", h.id) }));
        let n = self.cancels.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        brain.update_view(|v| {
            v.carry = None;
            v.cancel = Some((h.label.clone(), h.id.clone(), n));
        });
        true
    }
}

/// One strip window's frame, in the desktop space (`desktop::Space`: global logical
/// points on macOS, physical pixels on Windows), top-left origin.
#[derive(Clone, Debug, PartialEq)]
pub struct StripFrame {
    pub label: String,
    pub monitor: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Desktop units per logical pixel of this strip: 1 on macOS, its scale on Windows.
    pub per_logical: f64,
}

/// The cursor on the strip of the monitor under it.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorAt {
    pub label: String,
    pub monitor: String,
    /// In that strip window's logical px.
    pub x: f64,
    pub y: f64,
    /// That strip's width.
    pub w: f64,
}

/// tao's cursor position (physical px at the primary monitor's scale, whatever monitor
/// the cursor is on) → global logical points.
pub fn cursor_points(physical: (f64, f64), primary_scale: f64) -> (f64, f64) {
    let s = if primary_scale.is_finite() && primary_scale > 0.0 { primary_scale } else { 1.0 };
    (physical.0 / s, physical.1 / s)
}

/// A window's outer position (physical px at the window's own scale) → global logical points.
pub fn window_points(physical: (f64, f64), window_scale: f64) -> (f64, f64) {
    cursor_points(physical, window_scale)
}

/// The monitor under a point (the desktop space), and that point in its strip window's
/// logical pixels. The monitor wins however the monitors are arranged; None when the point
/// is on no monitor, or on one that has no strip.
pub fn locate_cursor(at: (f64, f64), monitors: &[crate::locator::Monitor], strips: &[StripFrame]) -> Option<CursorAt> {
    let (x, y) = at;
    let m = monitors.iter().find(|m| x >= m.rect.x && x < m.rect.x + m.rect.w && y >= m.rect.y && y < m.rect.y + m.rect.h)?;
    let s = strips.iter().find(|s| s.monitor == m.name)?;
    let per = crate::desktop::sane(s.per_logical);
    Some(CursorAt { label: s.label.clone(), monitor: m.name.clone(), x: (x - s.x) / per, y: (y - s.y) / per, w: s.w / per })
}

/// Whether any mouse button is down, from the window server's combined session state.
/// Needs no permission: it reads the state, it doesn't watch or post events (M12 step 1 d).
#[cfg(target_os = "macos")]
pub fn mouse_down() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    }
    // kCGEventSourceStateCombinedSessionState; left, right, centre.
    (0..3).any(|b| unsafe { CGEventSourceButtonState(0, b) })
}

#[cfg(not(target_os = "macos"))]
pub fn mouse_down() -> bool {
    true
}

/// The monitors and the primary's scale, as the main thread last read them (the poll
/// can't read them from its own thread: macOS reports none there).
#[derive(Default)]
pub struct Screens {
    pub now: Mutex<(Vec<crate::locator::Monitor>, f64)>,
}

/// The strip windows' frames now, in the desktop space.
pub fn strip_frames<R: Runtime>(app: &AppHandle<R>) -> Vec<StripFrame> {
    let space = crate::desktop::Space::NATIVE;
    let by_monitor = app.state::<crate::strips::Strips>().by_monitor.lock().unwrap().clone();
    by_monitor
        .into_iter()
        .filter_map(|(monitor, label)| {
            let w = app.get_webview_window(&label)?;
            let (p, sz, sc) = (w.outer_position().ok()?, w.outer_size().ok()?, w.scale_factor().ok()?);
            let r = space.rect((p.x as f64, p.y as f64), (sz.width as f64, sz.height as f64), sc);
            Some(StripFrame { label, monitor, x: r.x, y: r.y, w: r.w, h: r.h, per_logical: space.per_logical(sc) })
        })
        .collect()
}

/// Where the cursor is now: the monitor under it and the point in that monitor's strip.
pub fn cursor_now<R: Runtime>(app: &AppHandle<R>) -> Option<CursorAt> {
    let c = app.cursor_position().ok()?;
    let (monitors, scale) = app.state::<Screens>().now.lock().unwrap().clone();
    locate_cursor(crate::desktop::Space::NATIVE.cursor((c.x, c.y), scale), &monitors, &strip_frames(app))
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// The sprite's key in the webview (for the fade).
    #[serde(default)]
    pub id: Option<String>,
}

/// Window label -> its sprite rectangles, in the window's logical pixels.
#[derive(Default)]
pub struct Hitboxes {
    pub by_window: Mutex<HashMap<String, Vec<Rect>>>,
}

/// Is a point (logical px, relative to the window) inside any rectangle?
pub fn hit(boxes: &[Rect], x: f64, y: f64) -> bool {
    boxes.iter().any(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
}

/// Ids of the sprites whose box holds a point (the hover label, deviation 136).
pub fn over(boxes: &[Rect], x: f64, y: f64) -> Vec<String> {
    let mut ids: Vec<String> = boxes.iter().filter(|r| hit(std::slice::from_ref(r), x, y)).filter_map(|r| r.id.clone()).collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Ids of the sprites whose box is within `radius` of a point (the cursor-near fade).
pub fn near(boxes: &[Rect], x: f64, y: f64, radius: f64) -> Vec<String> {
    let mut ids: Vec<String> = boxes
        .iter()
        .filter(|r| {
            let dx = (r.x - x).max(0.0).max(x - (r.x + r.w));
            let dy = (r.y - y).max(0.0).max(y - (r.y + r.h));
            dx * dx + dy * dy <= radius * radius
        })
        .filter_map(|r| r.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// `demo_drag`: `--demo-drag`'s scripted gesture has no real button, so the fail-safe
/// doesn't look at the buttons (the 2-minute ceiling still holds).
pub fn run<R: Runtime>(app: AppHandle<R>, fade_radius: f64, demo_drag: bool) {
    let mut taking: HashMap<String, bool> = HashMap::new();
    let brain = app.state::<std::sync::Arc<crate::brain::Brain>>().inner().clone();
    let mut fast = true;
    loop {
        // 30 Hz while the cursor is near a strip that has sprites, or a gesture is under
        // way; 10 Hz otherwise (Milestone 7: most of the shell's idle CPU was this poll).
        std::thread::sleep(Duration::from_millis(if fast { 33 } else { 100 }));
        let Ok(cursor) = app.cursor_position() else { continue };
        let boxes = app.state::<Hitboxes>().by_window.lock().unwrap().clone();
        let mut close: Vec<String> = Vec::new();
        let mut under: Vec<(String, Vec<String>)> = Vec::new();
        fast = false;
        // The hold: the fail-safe and the ceiling first, then Quiet and Hide, which end it too.
        let drag = app.state::<Drag>();
        let ending = {
            let mut h = drag.hold.lock().unwrap();
            h.as_mut().and_then(|h| h.observe(Instant::now(), demo_drag || mouse_down()))
        };
        if let Some(reason) = ending {
            drag.cancel(&brain, reason, None);
        } else if drag.hold.lock().unwrap().is_some() && (brain.muted() || brain.view().hidden) {
            drag.cancel(&brain, if brain.muted() { "Quiet is on" } else { "the strip is hidden" }, None);
        }
        let hold = drag.hold.lock().unwrap().clone();
        fast |= hold.is_some();
        let (monitors, primary_scale) = app.state::<Screens>().now.lock().unwrap().clone();
        let space = crate::desktop::Space::NATIVE;
        let point = space.cursor((cursor.x, cursor.y), if primary_scale > 0.0 { primary_scale } else { 0.0 });
        for (label, window) in app.webview_windows() {
            // Only strips are click-through; the onboarding window is a normal window.
            if !crate::strips::is_strip(&label) {
                continue;
            }
            let Ok(pos) = window.outer_position() else { continue };
            let Ok(scale) = window.scale_factor() else { continue };
            // The cursor in the window's logical pixels. On macOS tao gives the cursor at the
            // primary monitor's scale and the window at its own, so each is made logical by
            // its own (until the primary's scale is known, the window's: the same on one
            // monitor). On Windows both are physical: the difference is divided by the
            // window's scale (WD3).
            let (cx, cy) = if primary_scale > 0.0 || space == crate::desktop::Space::Physical { point } else { cursor_points((cursor.x, cursor.y), scale) };
            let (wx, wy) = space.point((pos.x as f64, pos.y as f64), scale);
            let per = space.per_logical(scale);
            let x = (cx - wx) / per;
            let y = (cy - wy) / per;
            if let Some(b) = boxes.get(&label) {
                close.extend(near(b, x, y, fade_radius));
                let ids = over(b, x, y);
                if !ids.is_empty() {
                    under.push((label.clone(), ids));
                }
                if !b.is_empty() {
                    if let Ok(size) = window.outer_size() {
                        let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
                        let m = fade_radius + 40.0;
                        fast |= x > -m && x < w + m && y > -m && y < h + m;
                    }
                }
            }
            let over = boxes.get(&label).map_or(false, |b| hit(b, x, y));
            let held = hold.as_ref().is_some_and(|h| h.label == label);
            let accept = accepting(over, held);
            if taking.get(&label) != Some(&accept) {
                if window.set_ignore_cursor_events(!accept).is_ok() {
                    if crate::settings::env_os("SHELL_TRACE").is_some() {
                        log(&format!("{label}: cursor {} sprite{} -> {}", if over { "over" } else { "off" }, if held { ", holding a press" } else { "" }, if accept { "accepting clicks" } else { "click-through" }));
                    }
                    taking.insert(label.clone(), accept);
                }
            }
        }
        // The carried sheep: drawn by the strip of the monitor under the cursor. On the
        // pressed strip's own monitor that strip draws it from its own pointer events.
        let carry = hold.as_ref().filter(|h| h.lifted && !demo_drag).and_then(|h| {
            let at = locate_cursor(point, &monitors, &strip_frames(&app))?;
            let ready = app.state::<crate::strips::Strips>().is_ready(&at.label);
            (at.label != h.label && ready).then(|| Carry { label: at.label, pressed: h.label.clone(), id: h.id.clone(), x: at.x.round(), y: at.y.round(), grab_dx: h.grab_dx, colour: h.colour, alert: h.alert.clone() })
        });
        // The fade and the hover: the webview only hears about them when they change.
        under.sort();
        let now = brain.view();
        if now.near != close || now.over != under {
            brain.update_view(|v| {
                v.near = close;
                v.over = under;
            });
        }
        // Published only when the owner or the position changes: at most once a poll (30 Hz).
        if hold.as_ref().is_some_and(|h| h.lifted) && now.carry != carry {
            brain.update_view(|v| v.carry = carry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing() {
        let boxes = [Rect { x: 10.0, y: 70.0, w: 60.0, h: 60.0, id: None }];
        assert!(hit(&boxes, 10.0, 70.0));
        assert!(hit(&boxes, 69.9, 129.9));
        assert!(!hit(&boxes, 70.0, 100.0));
        assert!(!hit(&boxes, 9.9, 100.0));
        assert!(!hit(&[], 0.0, 0.0));
    }

    #[test]
    fn over_lists_the_sprites_under_the_cursor() {
        let a = Rect { x: 100.0, y: 70.0, w: 60.0, h: 60.0, id: Some("a".into()) };
        let b = Rect { x: 140.0, y: 70.0, w: 60.0, h: 60.0, id: Some("b".into()) };
        let boxes = [a, b];
        assert_eq!(over(&boxes, 120.0, 100.0), vec!["a".to_string()]);
        assert_eq!(over(&boxes, 150.0, 100.0), vec!["a".to_string(), "b".to_string()], "where two boxes overlap");
        assert!(over(&boxes, 99.0, 100.0).is_empty(), "just left of a");
        assert!(over(&boxes, 120.0, 60.0).is_empty(), "above a");
        assert!(over(&[Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0, id: None }], 5.0, 5.0).is_empty(), "a box without an id");
    }

    #[test]
    fn the_view_tells_each_strip_only_its_own_hover() {
        let v = crate::brain::ShellView { over: vec![("strip-0".into(), vec!["a".into()])], ..Default::default() };
        assert_eq!(v.json_for("strip-0")["over"], serde_json::json!(["a"]));
        assert_eq!(v.json_for("strip-1")["over"], serde_json::json!([]), "another strip: the cursor is on none of its sprites");
        assert_eq!(crate::brain::ShellView::default().json_for("strip-0")["over"], serde_json::json!([]));
    }

    #[test]
    fn near_lists_sprites_within_the_radius() {
        let a = Rect { x: 100.0, y: 70.0, w: 60.0, h: 60.0, id: Some("a".into()) };
        let b = Rect { x: 400.0, y: 70.0, w: 60.0, h: 60.0, id: Some("b".into()) };
        let boxes = [a, b];
        assert_eq!(near(&boxes, 130.0, 100.0, 90.0), vec!["a".to_string()], "inside a");
        assert_eq!(near(&boxes, 249.0, 100.0, 90.0), vec!["a".to_string()], "89 px right of a");
        assert!(near(&boxes, 251.0, 100.0, 90.0).is_empty(), "91 px right of a");
        assert_eq!(near(&boxes, 280.0, 100.0, 200.0), vec!["a".to_string(), "b".to_string()]);
        assert!(near(&boxes, 130.0, -100.0, 90.0).is_empty(), "far above the strip");
    }

    // D10, milestone 12: the hold, its fail-safe, and the cursor mapping.

    #[test]
    fn a_window_accepts_while_the_cursor_is_on_a_box_or_it_holds_a_press() {
        assert!(accepting(true, false));
        assert!(accepting(false, true), "held: the carried sheep may outrun its box");
        assert!(accepting(true, true));
        assert!(!accepting(false, false), "neither: click-through");
    }

    #[test]
    fn the_failsafe_cancels_after_250_ms_with_no_button_down() {
        let t0 = Instant::now();
        let mut h = Hold::new("strip-0", "s1", t0);
        assert_eq!(h.observe(t0, true), None);
        assert_eq!(h.observe(t0 + Duration::from_millis(100), false), None, "the button just came up");
        assert_eq!(h.observe(t0 + Duration::from_millis(340), false), None, "240 ms up");
        assert_eq!(h.observe(t0 + Duration::from_millis(350), false), Some("no mouse button down for 250 ms"));
        // A button down again in between starts the count over.
        let mut h = Hold::new("strip-0", "s1", t0);
        h.observe(t0 + Duration::from_millis(100), false);
        h.observe(t0 + Duration::from_millis(300), true);
        assert_eq!(h.observe(t0 + Duration::from_millis(400), false), None);
        assert_eq!(h.observe(t0 + Duration::from_millis(649), false), None);
        assert_eq!(h.observe(t0 + Duration::from_millis(650), false), Some("no mouse button down for 250 ms"));
    }

    #[test]
    fn no_hold_outlasts_two_minutes_even_with_the_button_down() {
        let t0 = Instant::now();
        let mut h = Hold::new("strip-0", "s1", t0);
        assert_eq!(h.observe(t0 + Duration::from_secs(119), true), None);
        assert_eq!(h.observe(t0 + CEILING, true), Some("held for 2 minutes"));
    }

    #[test]
    fn the_view_tells_the_drawing_strip_where_and_the_pressed_strip_that_it_is_drawn_elsewhere() {
        let c = Carry { label: "strip-1".into(), pressed: "strip-0".into(), id: "s1".into(), x: 300.0, y: 40.0, grab_dx: 20.0, colour: 3, alert: Some("permission".into()) };
        let v = crate::brain::ShellView { carry: Some(c), cancel: Some(("strip-0".into(), "s1".into(), 2)), ..Default::default() };
        assert_eq!(v.json_for("strip-1")["carry"], serde_json::json!({"id": "s1", "x": 300.0, "y": 40.0, "grabDx": 20.0, "colourIndex": 3, "alert": "permission"}));
        assert_eq!(v.json_for("strip-0")["carry"], serde_json::json!({"id": "s1", "away": true}));
        assert_eq!(v.json_for("strip-2")["carry"], serde_json::Value::Null);
        assert_eq!(v.json_for("strip-0")["dragCancel"], serde_json::json!({"id": "s1", "n": 2}));
        assert_eq!(v.json_for("strip-1")["dragCancel"], serde_json::Value::Null);
        assert_eq!(crate::brain::ShellView { placement: 7, ..Default::default() }.json_for("strip-0")["placement"], 7, "a strip re-applies its flock when placement changes");
    }

    fn mon(name: &str, x: f64, y: f64, w: f64, h: f64) -> crate::locator::Monitor {
        crate::locator::Monitor { name: name.into(), rect: crate::locator::Rect { x, y, w, h }, primary: name == "Built-in" }
    }
    fn strip(label: &str, monitor: &str, x: f64, y: f64, w: f64) -> StripFrame {
        StripFrame { label: label.into(), monitor: monitor.into(), x, y, w, h: 140.0, per_logical: 1.0 }
    }

    #[test]
    fn the_cursor_maps_to_the_strip_of_the_monitor_under_it_side_by_side() {
        let monitors = [mon("Built-in", 0.0, 0.0, 1728.0, 1117.0), mon("DELL", 1728.0, 0.0, 2560.0, 1440.0)];
        let strips = [strip("strip-0", "Built-in", 0.0, 900.0, 1728.0), strip("strip-1", "DELL", 1728.0, 1230.0, 2560.0)];
        let at = locate_cursor((100.0, 950.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y, at.w), ("strip-0", 100.0, 50.0, 1728.0));
        let at = locate_cursor((2000.0, 1300.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.monitor.as_str(), at.x, at.y), ("strip-1", "DELL", 272.0, 70.0));
        // Above the strip, on the same monitor: still that strip (a negative y: it hangs at the top).
        assert_eq!(locate_cursor((2000.0, 200.0), &monitors, &strips).unwrap().y, -1030.0);
        // Below the shorter monitor, beside the taller one: on no monitor.
        assert_eq!(locate_cursor((100.0, 1200.0), &monitors, &strips), None);
        // A monitor without a strip yet: None (the pressed strip keeps drawing, at its edge).
        assert_eq!(locate_cursor((2000.0, 1300.0), &monitors, &strips[..1]), None);
    }

    #[test]
    fn the_cursor_maps_on_stacked_monitors_by_the_monitor_not_by_x() {
        // The DELL above the built-in, overlapping in x: x alone can't tell them apart.
        let monitors = [mon("Built-in", 0.0, 0.0, 1728.0, 1117.0), mon("DELL", -400.0, -1440.0, 2560.0, 1440.0)];
        let strips = [strip("strip-0", "Built-in", 0.0, 977.0, 1728.0), strip("strip-1", "DELL", -400.0, -140.0, 2560.0)];
        let at = locate_cursor((500.0, -60.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y), ("strip-1", 900.0, 80.0));
        let at = locate_cursor((500.0, 1000.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y), ("strip-0", 500.0, 23.0));
    }

    #[test]
    fn mixed_scales_each_position_is_made_logical_by_its_own_scale() {
        // Primary at 2x; the DELL at 1x to its right. tao gives the cursor at the primary's
        // scale wherever it is, and a window's position at the window's own scale.
        let monitors = [mon("Built-in", 0.0, 0.0, 1728.0, 1117.0), mon("DELL", 1728.0, 0.0, 2560.0, 1440.0)];
        let dell_window = window_points((1728.0, 1300.0), 1.0);
        assert_eq!(dell_window, (1728.0, 1300.0));
        let builtin_window = window_points((0.0, 1954.0), 2.0);
        assert_eq!(builtin_window, (0.0, 977.0));
        let strips = [strip("strip-0", "Built-in", builtin_window.0, builtin_window.1, 1728.0), strip("strip-1", "DELL", dell_window.0, dell_window.1, 2560.0)];
        // The cursor 100 points into the DELL's strip: physical (1828 * 2, 1350 * 2) at the primary's 2x.
        let at = locate_cursor(cursor_points((3656.0, 2700.0), 2.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y), ("strip-1", 100.0, 50.0));
        // Scaled by the window's 1x instead (the old way), it would land 1828 points off.
        assert_ne!(cursor_points((3656.0, 2700.0), 1.0).0 - dell_window.0, 100.0);
        // A primary at 1x and the DELL at 2x.
        let at = locate_cursor(cursor_points((1828.0, 1350.0), 1.0), &monitors, &[strip("strip-1", "DELL", 1728.0, 1300.0, 2560.0)]).unwrap();
        assert_eq!((at.x, at.y), (100.0, 50.0));
        assert_eq!(cursor_points((10.0, 10.0), 0.0), (10.0, 10.0), "an unknown scale counts as 1");
    }

    // Windows (WD3): the physical desktop space. Monitors, strips and the cursor are
    // physical pixels of one virtual screen; only the offset inside a strip is divided, by
    // that strip's own scale.
    fn wstrip(label: &str, monitor: &str, r: crate::desktop::PhysRect, scale: f64) -> StripFrame {
        StripFrame { label: label.into(), monitor: monitor.into(), x: r.x as f64, y: r.y as f64, w: r.w as f64, h: r.h as f64, per_logical: scale }
    }
    fn wmon(name: &str, x: f64, y: f64, w: f64, h: f64, primary: bool) -> crate::locator::Monitor {
        crate::locator::Monitor { name: name.into(), rect: crate::locator::Rect { x, y, w, h }, primary }
    }
    fn on(work: (i32, i32, u32, u32), scale: f64) -> crate::desktop::PhysRect {
        crate::desktop::physical_strip_rect(crate::desktop::PhysRect { x: work.0, y: work.1, w: work.2, h: work.3 }, scale, 140.0)
    }

    #[test]
    fn windows_side_by_side_at_100_and_150_percent() {
        let space = crate::desktop::Space::Physical;
        let monitors = [wmon("\\\\.\\DISPLAY1", 0.0, 0.0, 1920.0, 1080.0, true), wmon("\\\\.\\DISPLAY2", 1920.0, 0.0, 3840.0, 2160.0, false)];
        let (one, two) = (on((0, 0, 1920, 1032), 1.0), on((1920, 0, 3840, 2088), 1.5));
        assert_eq!((two.x, two.y, two.w, two.h), (1920, 1878, 3840, 210));
        let strips = [wstrip("strip-0", &monitors[0].name, one, 1.0), wstrip("strip-1", &monitors[1].name, two, 1.5)];
        // 150 physical px into the second strip, 75 down: 100 x 50 of its CSS pixels.
        let at = locate_cursor(space.cursor((2070.0, 1953.0), 1.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y, at.w), ("strip-1", 100.0, 50.0, 2560.0));
        let at = locate_cursor(space.cursor((100.0, 950.0), 1.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y, at.w), ("strip-0", 100.0, 58.0, 1920.0));
        // The cursor on the 150% monitor's left edge is not on the 100% one (the macOS maths
        // would have folded it to x 1280, inside the primary).
        assert_eq!(locate_cursor((1920.0, 1900.0), &monitors, &strips).unwrap().label, "strip-1");
        assert_eq!(locate_cursor((1919.0, 1000.0), &monitors, &strips).unwrap().label, "strip-0");
        // Below the shorter monitor, beside the taller one: on no monitor.
        assert_eq!(locate_cursor((100.0, 1500.0), &monitors, &strips), None);
    }

    #[test]
    fn windows_stacked_by_the_monitor_not_by_x() {
        // A 150% monitor above the primary, overlapping it in x.
        let monitors = [wmon("A", 0.0, 0.0, 1920.0, 1080.0, true), wmon("B", -400.0, -2160.0, 3840.0, 2160.0, false)];
        let strips = [wstrip("strip-0", "A", on((0, 0, 1920, 1032), 1.0), 1.0), wstrip("strip-1", "B", on((-400, -2160, 3840, 2088), 1.5), 1.5)];
        let at = locate_cursor((500.0, -150.0), &monitors, &strips).unwrap();
        // Strip B: y from -282 (2088 - 210 - 2160); 900 px right of its left edge.
        assert_eq!((at.label.as_str(), at.x, at.y), ("strip-1", 600.0, 88.0));
        let at = locate_cursor((500.0, 1000.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y), ("strip-0", 500.0, 108.0));
    }

    #[test]
    fn windows_a_175_percent_monitor_left_of_the_primary_at_negative_x() {
        let monitors = [wmon("P", 0.0, 0.0, 1920.0, 1080.0, true), wmon("L", -2560.0, 0.0, 2560.0, 1440.0, false), wmon("R", 1920.0, 0.0, 3840.0, 2160.0, false)];
        let l = on((-2560, 0, 2560, 1356), 1.75);
        assert_eq!((l.x, l.y, l.h), (-2560, 1111, 245));
        let strips = [wstrip("strip-0", "P", on((0, 0, 1920, 1032), 1.0), 1.0), wstrip("strip-1", "L", l, 1.75), wstrip("strip-2", "R", on((1920, 0, 3840, 2088), 1.5), 1.5)];
        // 350 physical px right of L's left edge, 70 down: 200 x 40 CSS px at 175%.
        let at = locate_cursor((-2210.0, 1181.0), &monitors, &strips).unwrap();
        assert_eq!((at.label.as_str(), at.x, at.y), ("strip-1", 200.0, 40.0));
        assert!((at.w - 2560.0 / 1.75).abs() < 1e-9);
        assert_eq!(locate_cursor((-1.0, 1200.0), &monitors, &strips).unwrap().label, "strip-1");
        assert_eq!(locate_cursor((0.0, 1000.0), &monitors, &strips).unwrap().label, "strip-0");
        assert_eq!(locate_cursor((5000.0, 2000.0), &monitors, &strips).unwrap().label, "strip-2");
    }

    #[test]
    fn a_dock_on_the_left_moves_the_strip_and_the_mapping_with_it() {
        let monitors = [mon("Built-in", 0.0, 0.0, 1728.0, 1117.0)];
        // The work area starts after an 80-point Dock on the left.
        let strips = [strip("strip-0", "Built-in", 80.0, 977.0, 1648.0)];
        let at = locate_cursor((100.0, 1000.0), &monitors, &strips).unwrap();
        assert_eq!((at.x, at.w), (20.0, 1648.0));
        // Over the Dock itself: still that monitor's strip, left of its edge.
        assert_eq!(locate_cursor((40.0, 1000.0), &monitors, &strips).unwrap().x, -40.0);
    }
}
