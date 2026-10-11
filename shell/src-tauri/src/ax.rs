//! The Dock's exact frame, through Accessibility (Milestone 10, Dock-width
//! confinement). Only read when this binary is already trusted: `AXIsProcessTrusted`
//! is checked without its prompt; the system prompt appears only when the user runs
//! `bellwether shell dock-permission` (which runs this binary with `--ax-prompt`).
//!
//! Without the permission nothing changes: the full-width strip, as before.
//!
//! The permission belongs to this binary as macOS identifies it. The release build is
//! ad-hoc, linker-signed, and its designated requirement is its cdhash (measured with
//! `codesign -d -r-`), so every rebuild is a new identity and the grant must be given
//! again (System Settings > Privacy & Security > Accessibility).

use crate::dock::Frame;

/// The Dock's extent within a strip, in the strip's own x (logical points): Some only
/// when the Dock is at the bottom, shown (not auto-hidden), and overlaps the strip.
pub fn dock_extent(strip: Frame, dock: Option<Frame>, prefs: crate::dock::DockPrefs) -> Option<(f64, f64)> {
    if prefs.autohide || prefs.side != crate::dock::Side::Bottom {
        return None;
    }
    let d = dock?;
    let lo = d.x.max(strip.x) - strip.x;
    let hi = (d.x + d.w).min(strip.x + strip.w) - strip.x;
    // The Dock must actually sit under this strip (same screen, touching its bottom).
    let below = (d.y - (strip.y + strip.h)).abs() < 40.0;
    (hi - lo >= 60.0 && below).then_some((lo, hi))
}

/// Accepts a new Dock frame only once it has been read the same twice in a row, so the
/// magnification while the pointer is over the Dock doesn't move the sheep.
#[derive(Default)]
pub struct Steady {
    last: Option<Frame>,
    pub accepted: Option<Frame>,
}

impl Steady {
    pub fn feed(&mut self, read: Option<Frame>) -> bool {
        let same = |a: Option<Frame>, b: Option<Frame>| match (a, b) {
            (Some(a), Some(b)) => (a.x - b.x).abs() < 1.0 && (a.y - b.y).abs() < 1.0 && (a.w - b.w).abs() < 1.0 && (a.h - b.h).abs() < 1.0,
            (None, None) => true,
            _ => false,
        };
        let stable = same(read, self.last);
        self.last = read;
        if stable && !same(read, self.accepted) {
            self.accepted = read;
            return true;
        }
        false
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Frame;
    use std::ffi::{c_void, CString};
    type Ref = *const c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: Ref) -> bool;
        fn AXUIElementCreateApplication(pid: i32) -> Ref;
        fn AXUIElementCopyAttributeValue(el: Ref, attr: Ref, out: *mut Ref) -> i32;
        fn AXValueGetValue(v: Ref, kind: u32, out: *mut c_void) -> bool;
        fn AXUIElementSetMessagingTimeout(el: Ref, seconds: f32) -> i32;
        static kAXTrustedCheckOptionPrompt: Ref;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: Ref, s: *const std::os::raw::c_char, enc: u32) -> Ref;
        fn CFArrayGetCount(a: Ref) -> isize;
        fn CFArrayGetValueAtIndex(a: Ref, i: isize) -> Ref;
        fn CFStringGetCString(s: Ref, buf: *mut u8, size: isize, enc: u32) -> bool;
        fn CFDictionaryCreate(alloc: Ref, keys: *const Ref, values: *const Ref, n: isize, kcb: Ref, vcb: Ref) -> Ref;
        fn CFRelease(x: Ref);
        static kCFBooleanTrue: Ref;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
    }
    const UTF8: u32 = 0x0800_0100;
    const POINT: u32 = 1; // kAXValueCGPointType
    const SIZE: u32 = 2; // kAXValueCGSizeType

    pub fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// Asks macOS to show its Accessibility prompt for this binary (only on request).
    pub fn prompt() -> bool {
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let opts = CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 1, &kCFTypeDictionaryKeyCallBacks as *const _ as Ref, &kCFTypeDictionaryValueCallBacks as *const _ as Ref);
            let ok = AXIsProcessTrustedWithOptions(opts);
            CFRelease(opts);
            ok
        }
    }

    fn cf(s: &str) -> Ref {
        let c = CString::new(s).unwrap();
        unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
    }


    /// A CF object this code owns, released when it goes out of scope (every path).
    struct Cf(Ref);
    impl Drop for Cf {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    /// How long one AX request may take before it gives up (the default is seconds).
    const AX_TIMEOUT_S: f32 = 0.25;

    unsafe fn attr(el: Ref, name: &str) -> Option<Cf> {
        let key = Cf(cf(name));
        let mut out: Ref = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(el, key.0, &mut out);
        (err == 0 && !out.is_null()).then(|| Cf(out))
    }

    /// The frame of the Dock's icon list (its AXList), in global points, top-left origin.
    /// Each request times out after AX_TIMEOUT_S, so a hung Dock costs at most a few of
    /// those; it runs on its own thread anyway (lib.rs), never the privacy loop's.
    pub fn dock_frame(dock_pid: i32) -> Option<Frame> {
        unsafe {
            let app = Cf(AXUIElementCreateApplication(dock_pid));
            if app.0.is_null() {
                return None;
            }
            AXUIElementSetMessagingTimeout(app.0, AX_TIMEOUT_S);
            let children = attr(app.0, "AXChildren")?;
            for i in 0..CFArrayGetCount(children.0) {
                let el = CFArrayGetValueAtIndex(children.0, i); // borrowed from the array
                AXUIElementSetMessagingTimeout(el, AX_TIMEOUT_S);
                let Some(role) = attr(el, "AXRole") else { continue };
                let mut buf = [0u8; 32];
                let is_list = CFStringGetCString(role.0, buf.as_mut_ptr(), buf.len() as isize, UTF8) && buf.starts_with(b"AXList\0");
                if !is_list {
                    continue;
                }
                // Each released on its own when it goes out of scope, whichever fails.
                let position = attr(el, "AXPosition");
                let size = attr(el, "AXSize");
                let mut pt = [0f64; 2];
                let mut sz = [0f64; 2];
                let ok = matches!((&position, &size), (Some(p), Some(s)) if AXValueGetValue(p.0, POINT, pt.as_mut_ptr() as *mut c_void) && AXValueGetValue(s.0, SIZE, sz.as_mut_ptr() as *mut c_void));
                return ok.then_some(Frame { x: pt[0], y: pt[1], w: sz[0], h: sz[1] });
            }
            None
        }
    }

    /// The Dock's pid: the process named "Dock" (libproc, no subprocess).
    pub fn dock_pid() -> Option<i32> {
        let mut pids = vec![0i32; 4096];
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as i32) };
        let mut buf = [0u8; 64];
        pids.into_iter().take(n.max(0) as usize).filter(|&p| p > 0).find(|&pid| {
            let len = unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
            len > 0 && &buf[..len as usize] == b"Dock"
        })
    }
}

#[cfg(target_os = "macos")]
pub use mac::{dock_frame, dock_pid, prompt, trusted};

#[cfg(not(target_os = "macos"))]
pub fn trusted() -> bool {
    false
}
#[cfg(not(target_os = "macos"))]
pub fn prompt() -> bool {
    false
}
#[cfg(not(target_os = "macos"))]
pub fn dock_pid() -> Option<i32> {
    None
}
#[cfg(not(target_os = "macos"))]
pub fn dock_frame(_pid: i32) -> Option<Frame> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{DockPrefs, Side};

    const STRIP: Frame = Frame { x: 0.0, y: 911.0, w: 1728.0, h: 140.0 };
    const SHOWN: DockPrefs = DockPrefs { autohide: false, side: Side::Bottom };
    fn dock(x: f64, w: f64) -> Frame {
        Frame { x, y: 1051.0, w, h: 66.0 }
    }

    #[test]
    fn a_shown_bottom_dock_gives_its_extent_in_strip_coordinates() {
        assert_eq!(dock_extent(STRIP, Some(dock(514.0, 700.0)), SHOWN), Some((514.0, 1214.0)), "centred");
        assert_eq!(dock_extent(STRIP, Some(dock(0.0, 300.0)), SHOWN), Some((0.0, 300.0)), "at the left");
        assert_eq!(dock_extent(STRIP, Some(dock(0.0, 1728.0)), SHOWN), Some((0.0, 1728.0)), "full width");
        assert_eq!(dock_extent(Frame { x: 100.0, ..STRIP }, Some(dock(514.0, 700.0)), SHOWN), Some((414.0, 1114.0)), "a strip not at x 0");
    }

    #[test]
    fn no_extent_when_auto_hidden_on_a_side_too_narrow_unknown_or_elsewhere() {
        assert_eq!(dock_extent(STRIP, Some(dock(514.0, 700.0)), DockPrefs { autohide: true, side: Side::Bottom }), None);
        assert_eq!(dock_extent(STRIP, Some(dock(514.0, 700.0)), DockPrefs { autohide: false, side: Side::Left }), None);
        assert_eq!(dock_extent(STRIP, Some(dock(800.0, 40.0)), SHOWN), None, "too narrow to stand on");
        assert_eq!(dock_extent(STRIP, None, SHOWN), None, "no permission: no frame");
        assert_eq!(dock_extent(STRIP, Some(Frame { y: 300.0, ..dock(514.0, 700.0) }), SHOWN), None, "not under this strip");
    }

    #[test]
    fn magnification_is_ignored_until_the_frame_holds_still() {
        let mut s = Steady::default();
        let a = Some(dock(514.0, 700.0));
        assert!(!s.feed(a));
        assert!(s.feed(a), "the same twice: accepted");
        assert!(!s.feed(Some(dock(480.0, 768.0))), "hovering: magnified once");
        assert!(!s.feed(a), "back to normal");
        assert_eq!(s.accepted, a);
        let wider = Some(dock(480.0, 768.0));
        s.feed(wider);
        assert!(s.feed(wider), "an icon was added: held twice, accepted");
    }
}
