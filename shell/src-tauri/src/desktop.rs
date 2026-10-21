//! Where the strip goes on Windows, and the one space every position is compared in
//! (Windows plan WD1 and WD3). Pure functions only, so the Mac runs their tests too.
//!
//! - **The taskbar** is never asked for: a shown taskbar is already left out of its
//!   monitor's work area, so the strip sits on the work area's bottom edge, which is the
//!   taskbar's top edge when it is at the bottom, and the screen's bottom when it
//!   auto-hides or stands on another edge. `taskbar_edge` only says which edge the work
//!   area is inset from, for the log and the desktop dump.
//! - **The desktop space** (`Space`): macOS compares global points (each position divided
//!   by its own scale, today's maths); Windows with per-monitor DPI v2 gives the cursor,
//!   monitors and windows in physical pixels of one virtual screen, and dividing each by a
//!   different scale breaks a mixed-scale layout. There, physical pixels are the shared
//!   space and logical pixels are only used inside one strip.

use crate::dock::Frame;

/// Which edge of a monitor its work area is inset from: where a shown taskbar is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskbarEdge {
    Bottom,
    Top,
    Left,
    Right,
    /// The work area is the whole monitor: the taskbar auto-hides, or is on another one.
    None,
}

impl TaskbarEdge {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskbarEdge::Bottom => "bottom",
            TaskbarEdge::Top => "top",
            TaskbarEdge::Left => "left",
            TaskbarEdge::Right => "right",
            TaskbarEdge::None => "none",
        }
    }
}

/// The edge the work area is inset from (the largest inset; under half a pixel is none).
pub fn taskbar_edge(screen: Frame, work: Frame) -> TaskbarEdge {
    let insets = [
        (TaskbarEdge::Bottom, (screen.y + screen.h) - (work.y + work.h)),
        (TaskbarEdge::Top, work.y - screen.y),
        (TaskbarEdge::Left, work.x - screen.x),
        (TaskbarEdge::Right, (screen.x + screen.w) - (work.x + work.w)),
    ];
    let (edge, inset) = insets.iter().copied().fold((TaskbarEdge::None, 0.5), |best, (e, d)| if d > best.1 { (e, d) } else { best });
    if inset > 0.5 {
        edge
    } else {
        TaskbarEdge::None
    }
}

/// A rectangle in physical pixels of the virtual screen (top-left origin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl PhysRect {
    /// Whether a window's outer position and inner size (tao's physical pixels) are this
    /// frame: what the 2 s re-read and the deferred re-fit after a scale change compare.
    pub fn matches(self, pos: (i32, i32), size: (u32, u32)) -> bool {
        (pos.0, pos.1, size.0, size.1) == (self.x, self.y, self.w, self.h)
    }

    pub fn frame(self) -> Frame {
        Frame { x: self.x as f64, y: self.y as f64, w: self.w as f64, h: self.h as f64 }
    }
}

/// A scale the maths can divide by: anything not finite or not positive counts as 1.
pub fn sane(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The strip's frame on one monitor, in physical pixels (WD1, WD3): the work area's full
/// width, `height_px` logical pixels tall at the monitor's scale, standing on the work
/// area's bottom edge. Set with physical units, never logical ones: a logical size is
/// converted with the window's *current* scale, the wrong one while a new strip is still
/// on the monitor it was created on.
pub fn physical_strip_rect(work: PhysRect, scale: f64, height_px: f64) -> PhysRect {
    let h = ((height_px * sane(scale)).round() as i64).clamp(1, (work.h as i64).max(1)) as u32;
    PhysRect { x: work.x, y: work.y + work.h as i32 - h as i32, w: work.w, h }
}

/// The space positions are compared in (WD3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// macOS: global logical points. tao gives the cursor at the primary monitor's scale
    /// and a window or monitor at its own, so each is divided by that scale.
    Points,
    /// Windows (per-monitor DPI v2): the cursor, monitors and windows are all physical
    /// pixels of one virtual screen, used as they are.
    Physical,
}

impl Space {
    /// This build's space.
    pub const NATIVE: Space = if cfg!(windows) { Space::Physical } else { Space::Points };

    /// tao's cursor position → the desktop space.
    pub fn cursor(self, raw: (f64, f64), primary_scale: f64) -> (f64, f64) {
        match self {
            Space::Points => crate::clickthrough::cursor_points(raw, primary_scale),
            Space::Physical => raw,
        }
    }

    /// A window's outer position, or a monitor's (tao's physical pixels) → the desktop space.
    pub fn point(self, raw: (f64, f64), own_scale: f64) -> (f64, f64) {
        match self {
            Space::Points => crate::clickthrough::window_points(raw, own_scale),
            Space::Physical => raw,
        }
    }

    /// A monitor (or window) rectangle from tao's physical position and size → the desktop space.
    pub fn rect(self, pos: (f64, f64), size: (f64, f64), own_scale: f64) -> Frame {
        let (x, y) = self.point(pos, own_scale);
        let (w, h) = match self {
            Space::Points => (size.0 / sane(own_scale), size.1 / sane(own_scale)),
            Space::Physical => size,
        };
        Frame { x, y, w, h }
    }

    /// Desktop units per logical pixel of a window at `window_scale`: how a distance in
    /// the desktop space becomes one in that window's CSS pixels.
    pub fn per_logical(self, window_scale: f64) -> f64 {
        match self {
            Space::Points => 1.0,
            Space::Physical => sane(window_scale),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: f64, y: f64, w: f64, h: f64) -> Frame {
        Frame { x, y, w, h }
    }
    fn p(x: i32, y: i32, w: u32, h: u32) -> PhysRect {
        PhysRect { x, y, w, h }
    }

    #[test]
    fn the_taskbar_edge_is_the_edge_the_work_area_is_inset_from() {
        let screen = f(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(taskbar_edge(screen, f(0.0, 0.0, 1920.0, 1032.0)), TaskbarEdge::Bottom, "a 48 px taskbar at the bottom (Windows 11 at 100%)");
        assert_eq!(taskbar_edge(screen, f(0.0, 40.0, 1920.0, 1040.0)), TaskbarEdge::Top);
        assert_eq!(taskbar_edge(screen, f(62.0, 0.0, 1858.0, 1080.0)), TaskbarEdge::Left);
        assert_eq!(taskbar_edge(screen, f(0.0, 0.0, 1858.0, 1080.0)), TaskbarEdge::Right);
        assert_eq!(taskbar_edge(screen, screen), TaskbarEdge::None, "auto-hide, or a monitor without a taskbar");
        // A monitor left of the primary, at negative x, with its own (secondary) taskbar.
        assert_eq!(taskbar_edge(f(-2560.0, 0.0, 2560.0, 1440.0), f(-2560.0, 0.0, 2560.0, 1368.0)), TaskbarEdge::Bottom);
        // Auto-hide leaves no inset on Windows (unlike the Dock's reserved points), and a
        // sub-pixel difference is none.
        assert_eq!(taskbar_edge(screen, f(0.0, 0.0, 1920.0, 1079.6)), TaskbarEdge::None);
        assert_eq!(TaskbarEdge::Bottom.as_str(), "bottom");
    }

    #[test]
    fn the_strip_stands_on_the_work_areas_bottom_in_physical_pixels_at_each_scale() {
        // 1920x1080 at 100%, taskbar 48 px: the strip is 140 px on y 892..1032.
        assert_eq!(physical_strip_rect(p(0, 0, 1920, 1032), 1.0, 140.0), p(0, 892, 1920, 140));
        // 3840x2160 at 150% to its right: taskbar 72 px, the strip 210 px tall.
        assert_eq!(physical_strip_rect(p(1920, 0, 3840, 2088), 1.5, 140.0), p(1920, 1878, 3840, 210));
        // 175%: 245 px; a monitor left of the primary keeps its negative x.
        assert_eq!(physical_strip_rect(p(-2560, -200, 2560, 1368), 1.75, 140.0), p(-2560, 923, 2560, 245));
        // Auto-hide: the work area is the whole monitor, so the screen's bottom edge.
        let r = physical_strip_rect(p(0, 0, 1920, 1080), 1.0, 140.0);
        assert_eq!(r.y + r.h as i32, 1080);
        // A work area shorter than the strip caps it; a nonsense scale counts as 1.
        assert_eq!(physical_strip_rect(p(0, 0, 800, 100), 2.0, 140.0).h, 100);
        assert_eq!(physical_strip_rect(p(0, 0, 800, 600), f64::NAN, 140.0).h, 140);
        assert_eq!(physical_strip_rect(p(0, 0, 800, 600), 0.0, 140.0).h, 140);
    }

    /// After a scale change Windows resizes the window to its own suggestion (tao applies
    /// it in WM_DPICHANGED); the strip's frame at the new scale differs from that, so the
    /// deferred re-fit has something to do, and once done the frame matches.
    #[test]
    fn a_scale_change_leaves_a_frame_the_re_fit_puts_right() {
        let work = p(1920, 0, 3840, 2088);
        let at_150 = physical_strip_rect(work, 1.5, 140.0);
        let at_125 = physical_strip_rect(work, 1.25, 140.0);
        assert_eq!((at_125.y, at_125.h), (1913, 175));
        // Windows' suggestion keeps the logical size: 210 px at 150% becomes 175 px at 125%,
        // but anchored where the old frame's top was.
        let suggested = ((at_150.x, at_150.y), (3200u32, 175u32));
        assert!(!at_125.matches(suggested.0, suggested.1));
        assert!(at_125.matches((1920, 1913), (3840, 175)));
    }

    #[test]
    fn points_is_todays_macos_maths() {
        let s = Space::Points;
        assert_eq!(s.cursor((3656.0, 2700.0), 2.0), (1828.0, 1350.0));
        assert_eq!(s.point((0.0, 1954.0), 2.0), (0.0, 977.0));
        assert_eq!(s.rect((3456.0, 0.0), (5120.0, 2880.0), 2.0), f(1728.0, 0.0, 2560.0, 1440.0));
        assert_eq!(s.per_logical(2.0), 1.0);
        assert_eq!(Space::NATIVE, if cfg!(windows) { Space::Physical } else { Space::Points });
    }

    #[test]
    fn physical_keeps_every_position_as_windows_gives_it() {
        let s = Space::Physical;
        assert_eq!(s.cursor((2000.0, 1900.0), 1.0), (2000.0, 1900.0), "the primary's scale is irrelevant");
        assert_eq!(s.point((1920.0, 1878.0), 1.5), (1920.0, 1878.0));
        assert_eq!(s.rect((1920.0, 0.0), (3840.0, 2160.0), 1.5), f(1920.0, 0.0, 3840.0, 2160.0));
        assert_eq!(s.per_logical(1.5), 1.5);
        assert_eq!(s.per_logical(-1.0), 1.0);
    }

    /// WD3's example: dividing a 150% monitor right of a 100% primary by its own scale puts
    /// it inside the primary. In the physical space it stays where it is.
    #[test]
    fn a_150_percent_monitor_right_of_a_100_percent_primary_doesnt_fold_into_it() {
        let points = Space::Points.rect((1920.0, 0.0), (3840.0, 2160.0), 1.5);
        assert_eq!(points.x, 1280.0, "the macOS maths would put it at x 1280, inside the primary");
        let physical = Space::Physical.rect((1920.0, 0.0), (3840.0, 2160.0), 1.5);
        assert_eq!(physical.x, 1920.0);
    }
}
