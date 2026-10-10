//! Following the Dock (Milestone 10): with the Dock shown, the strip
//! sits just above it, on the work area's bottom edge (today's behaviour). With
//! auto-hide on, it sits on the screen's bottom edge, because macOS keeps a few points
//! of the work area reserved for the hidden Dock. A Dock on the left or right doesn't
//! touch the bottom, so the work area's bottom is the screen's bottom already, and
//! the work area's width keeps the strip clear of the Dock.
//!
//! The settings are read with CFPreferences (no subprocess), and the strip checks them
//! every 2 s (lib.rs), moving without a restart.

/// A rectangle in logical points, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockPrefs {
    pub autohide: bool,
    pub side: Side,
}

impl Default for DockPrefs {
    fn default() -> Self {
        DockPrefs { autohide: false, side: Side::Bottom }
    }
}

/// The strip's frame on one monitor: full work-area width, `height` tall, on the work
/// area's bottom edge, or on the screen's bottom edge when a bottom Dock auto-hides.
pub fn strip_rect_in(screen: Frame, work: Frame, dock: DockPrefs, height: f64) -> Frame {
    let height = height.min(work.h);
    let bottom = if dock.autohide && dock.side == Side::Bottom { screen.y + screen.h } else { work.y + work.h };
    Frame { x: work.x, y: bottom - height, w: work.w, h: height }
}

/// The Dock's settings now (`com.apple.dock` autohide and orientation). Read again from
/// disk each time, so a change made in System Settings is seen.
#[cfg(target_os = "macos")]
pub fn read_prefs() -> DockPrefs {
    use std::ffi::{c_void, CString};
    type Ref = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: Ref, s: *const std::os::raw::c_char, enc: u32) -> Ref;
        fn CFPreferencesAppSynchronize(app: Ref) -> bool;
        fn CFPreferencesCopyAppValue(key: Ref, app: Ref) -> Ref;
        fn CFPreferencesGetAppBooleanValue(key: Ref, app: Ref, valid: *mut bool) -> bool;
        fn CFStringGetCString(s: Ref, buf: *mut u8, size: isize, enc: u32) -> bool;
        fn CFGetTypeID(x: Ref) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFRelease(x: Ref);
    }
    const UTF8: u32 = 0x0800_0100;
    unsafe {
        let cf = |s: &str| {
            let c = CString::new(s).unwrap();
            CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8)
        };
        let app = cf("com.apple.dock");
        let (k_hide, k_side) = (cf("autohide"), cf("orientation"));
        CFPreferencesAppSynchronize(app);
        let mut valid = false;
        let autohide = CFPreferencesGetAppBooleanValue(k_hide, app, &mut valid) && valid;
        let v = CFPreferencesCopyAppValue(k_side, app);
        let mut side = Side::Bottom;
        if !v.is_null() {
            if CFGetTypeID(v) == CFStringGetTypeID() {
                let mut buf = [0u8; 32];
                if CFStringGetCString(v, buf.as_mut_ptr(), buf.len() as isize, UTF8) {
                    let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
                    side = match &buf[..len] {
                        b"left" => Side::Left,
                        b"right" => Side::Right,
                        _ => Side::Bottom,
                    };
                }
            }
            CFRelease(v);
        }
        for r in [app, k_hide, k_side] {
            CFRelease(r);
        }
        DockPrefs { autohide, side }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn read_prefs() -> DockPrefs {
    DockPrefs::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 14-inch MacBook (measured): screen 1728x1117, menu bar 25 pt, Dock 66 pt shown.
    const SCREEN: Frame = Frame { x: 0.0, y: 0.0, w: 1728.0, h: 1117.0 };
    const WITH_DOCK: Frame = Frame { x: 0.0, y: 25.0, w: 1728.0, h: 1026.0 };
    // Auto-hide: macOS keeps a few points reserved at the bottom for the hidden Dock.
    const HIDDEN_DOCK: Frame = Frame { x: 0.0, y: 25.0, w: 1728.0, h: 1088.0 };

    #[test]
    fn a_shown_dock_keeps_the_strip_just_above_it() {
        let r = strip_rect_in(SCREEN, WITH_DOCK, DockPrefs { autohide: false, side: Side::Bottom }, 140.0);
        assert_eq!(r, Frame { x: 0.0, y: 911.0, w: 1728.0, h: 140.0 }, "today's strip, measured: y 911");
    }

    #[test]
    fn an_auto_hidden_dock_puts_the_strip_on_the_screen_bottom_not_the_reserved_edge() {
        let r = strip_rect_in(SCREEN, HIDDEN_DOCK, DockPrefs { autohide: true, side: Side::Bottom }, 140.0);
        assert_eq!(r.y + r.h, 1117.0);
        assert_eq!(r.w, 1728.0);
    }

    #[test]
    fn a_side_dock_leaves_the_bottom_alone_and_the_strip_clear_of_the_dock() {
        let left_work = Frame { x: 70.0, y: 25.0, w: 1658.0, h: 1092.0 };
        for autohide in [false, true] {
            let r = strip_rect_in(SCREEN, left_work, DockPrefs { autohide, side: Side::Left }, 140.0);
            assert_eq!((r.x, r.w, r.y + r.h), (70.0, 1658.0, 1117.0));
        }
    }

    #[test]
    fn a_short_work_area_caps_the_height() {
        let tiny = Frame { x: 0.0, y: 0.0, w: 800.0, h: 100.0 };
        assert_eq!(strip_rect_in(tiny, tiny, DockPrefs::default(), 140.0).h, 100.0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_prefs_read_without_a_subprocess() {
        let _ = read_prefs(); // whatever this Mac says; it must not crash
    }
}
