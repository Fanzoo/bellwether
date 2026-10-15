//! Click-through by cursor polling. Every strip window ignores
//! cursor events by default, so clicks fall through to whatever is underneath. A
//! thread polls the cursor at 30 Hz and hit-tests it against the sprite rectangles
//! each webview reports (`report_hitboxes`); only while the cursor is over a sprite
//! does that window accept the mouse.
//!
//! Known limit (Risk R6): a click that lands within one poll interval of the cursor
//! arriving on a sprite may fall through.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

use crate::log::log;

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

pub fn run<R: Runtime>(app: AppHandle<R>, fade_radius: f64) {
    let mut accepting: HashMap<String, bool> = HashMap::new();
    let brain = app.state::<std::sync::Arc<crate::brain::Brain>>().inner().clone();
    let mut fast = true;
    loop {
        // 30 Hz while the cursor is near a strip that has sprites; 10 Hz otherwise
        // (Milestone 7: most of the shell's idle CPU was this poll).
        std::thread::sleep(Duration::from_millis(if fast { 33 } else { 100 }));
        let Ok(cursor) = app.cursor_position() else { continue };
        let boxes = app.state::<Hitboxes>().by_window.lock().unwrap().clone();
        let mut close: Vec<String> = Vec::new();
        fast = false;
        for (label, window) in app.webview_windows() {
            // Only strips are click-through; the onboarding window is a normal window.
            if !crate::strips::is_strip(&label) {
                continue;
            }
            let Ok(pos) = window.outer_position() else { continue };
            let Ok(scale) = window.scale_factor() else { continue };
            // Physical screen pixels -> the window's logical pixels.
            let x = (cursor.x - pos.x as f64) / scale;
            let y = (cursor.y - pos.y as f64) / scale;
            if let Some(b) = boxes.get(&label) {
                close.extend(near(b, x, y, fade_radius));
                if !b.is_empty() {
                    if let Ok(size) = window.outer_size() {
                        let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
                        let m = fade_radius + 40.0;
                        fast |= x > -m && x < w + m && y > -m && y < h + m;
                    }
                }
            }
            let over = boxes.get(&label).map_or(false, |b| hit(b, x, y));
            if accepting.get(&label) != Some(&over) {
                if window.set_ignore_cursor_events(!over).is_ok() {
                    if crate::settings::env_os("SHELL_TRACE").is_some() {
                        log(&format!("{label}: cursor {} sprite -> {}", if over { "over" } else { "off" }, if over { "accepting clicks" } else { "click-through" }));
                    }
                    accepting.insert(label.clone(), over);
                }
            }
        }
        // The fade: the webview only hears about it when the set changes.
        if brain.view().near != close {
            brain.update_view(|v| v.near = close);
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
}
