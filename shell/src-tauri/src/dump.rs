//! `--dump-desktop <file>` (Windows plan WD12, data model `desktop.json`): what the
//! maintainer's PC looks like to Bellwether, written to a file he attaches to the PR, so
//! the developer can turn it into test fixtures (`test/fixtures/windows/desktop-*.json`).
//!
//! Development and test builds only, never a release (`allowed`). Windows only: the
//! collection is `win::desktop`. This module is the file's shape, which the Mac tests.
//!
//! What is in it: the OS version; the monitors (rectangle, work area, scale, name, the
//! taskbar edge); the foreground window; every process's pid, parent pid and exe name; and
//! every visible top-level window's handle, pid, exe name, class name, bounds, cloaked and
//! tool-window state. Never a window title, a command line or an environment value.
//! Rectangles are `[left, top, right, bottom]` in physical pixels.

// Built on every OS so the Mac tests the file's shape; only Windows fills one in.
#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;

/// Whether this build answers `--dump-desktop`: a debug build, or the test zip's
/// (`--features test-build`). A release build refuses it.
pub fn allowed() -> bool {
    cfg!(debug_assertions) || cfg!(feature = "test-build")
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Desktop {
    pub version: u32,
    /// `CurrentMajorVersionNumber.CurrentMinorVersionNumber.CurrentBuildNumber.UBR`, and
    /// the release name (`24H2`), from the registry.
    pub os: Os,
    pub monitors: Vec<MonitorInfo>,
    pub foreground: u64,
    pub processes: Vec<Process>,
    pub windows: Vec<Window>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Os {
    pub version: String,
    pub display_version: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub name: String,
    pub primary: bool,
    pub scale: f64,
    pub rect: [i32; 4],
    pub work: [i32; 4],
    pub taskbar: &'static str,
    /// Where the strip would stand on it (WD1), `[left, top, right, bottom]`.
    pub strip: [i32; 4],
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub exe: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Window {
    pub hwnd: u64,
    pub pid: u32,
    pub exe: String,
    pub class: String,
    pub bounds: [i32; 4],
    pub cloaked: bool,
    pub tool: bool,
    pub topmost: bool,
}

/// One monitor's entry from what Windows says about it: the taskbar edge and the strip's
/// frame are worked out here (the same functions the strip uses).
pub fn monitor(name: String, primary: bool, scale: f64, rect: [i32; 4], work: [i32; 4], strip_height_px: f64) -> MonitorInfo {
    let phys = |r: [i32; 4]| crate::desktop::PhysRect { x: r[0], y: r[1], w: (r[2] - r[0]).max(0) as u32, h: (r[3] - r[1]).max(0) as u32 };
    let edge = crate::desktop::taskbar_edge(phys(rect).frame(), phys(work).frame());
    let s = crate::desktop::physical_strip_rect(phys(work), scale, strip_height_px);
    MonitorInfo { name, primary, scale, rect, work, taskbar: edge.as_str(), strip: [s.x, s.y, s.x + s.w as i32, s.y + s.h as i32] }
}

/// The file's text: pretty JSON with a closing newline.
pub fn render(d: &Desktop) -> String {
    serde_json::to_string_pretty(d).unwrap_or_default() + "\n"
}

/// `--dump-desktop <file>`: writes the file and says where, or why not. Exit code: 0
/// written, 1 failed, 2 refused (a release build, or not Windows).
pub fn run(file: Option<&str>, strip_height_px: f64) -> i32 {
    // To stderr and shell.log (log.rs writes both).
    let say = |line: &str| crate::log::log(&format!("--dump-desktop: {line}"));
    if !allowed() {
        say("refused: --dump-desktop is in development and test builds only");
        return 2;
    }
    let Some(file) = file.filter(|f| !f.is_empty() && !f.starts_with("--")) else {
        say("usage: --dump-desktop <file.json>");
        return 2;
    };
    #[cfg(windows)]
    {
        let d = crate::win::desktop(strip_height_px);
        match std::fs::write(file, render(&d)) {
            Ok(()) => {
                let line = format!("wrote {file}: {} monitors, {} processes, {} windows (no titles, command lines or environment)", d.monitors.len(), d.processes.len(), d.windows.len());
                println!("{line}");
                crate::log::log(&format!("--dump-desktop: {line}"));
                0
            }
            Err(e) => {
                say(&format!("cannot write {file}: {e}"));
                1
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (file, strip_height_px);
        say("--dump-desktop is for Windows (Windows plan WD12)");
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dump_holds_names_and_rectangles_never_titles_or_command_lines() {
        let d = Desktop {
            version: 1,
            os: Os { version: "10.0.26100.4652".into(), display_version: "24H2".into() },
            monitors: vec![monitor("\\\\.\\DISPLAY1".into(), true, 1.0, [0, 0, 1920, 1080], [0, 0, 1920, 1032], 140.0)],
            foreground: 132456,
            processes: vec![Process { pid: 9100, ppid: 8800, exe: "claude.exe".into() }],
            windows: vec![Window { hwnd: 132456, pid: 8800, exe: "WindowsTerminal.exe".into(), class: "CASCADIA_HOSTING_WINDOW_CLASS".into(), bounds: [1930, 10, 3800, 1400], cloaked: false, tool: false, topmost: false }],
        };
        let text = render(&d);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["monitors"][0]["taskbar"], "bottom");
        assert_eq!(v["monitors"][0]["strip"], serde_json::json!([0, 892, 1920, 1032]));
        assert_eq!(v["os"]["displayVersion"], "24H2");
        let keys = |o: &serde_json::Value| {
            let mut k: Vec<String> = o.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k.join(",")
        };
        assert_eq!(keys(&v), "foreground,monitors,os,processes,version,windows");
        assert_eq!(keys(&v["windows"][0]), "bounds,class,cloaked,exe,hwnd,pid,tool,topmost", "no title");
        assert_eq!(keys(&v["processes"][0]), "exe,pid,ppid", "no command line or environment");
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn only_debug_and_test_builds_dump() {
        assert_eq!(allowed(), cfg!(debug_assertions) || cfg!(feature = "test-build"));
        // tauri build and build-app.mjs never turn the feature on (the same rule as conformance).
        let build_app = include_str!("../../../scripts/build-app.mjs");
        assert!(!build_app.contains("test-build"));
    }

    /// The synthetic two-monitor fixture (100% and 150%, as the maintainer's PC can be set):
    /// its taskbar edges and strips are what the strip code works out from its rectangles.
    #[test]
    fn the_windows_fixture_agrees_with_the_strip_maths() {
        let v: serde_json::Value = serde_json::from_str(include_str!("../../../test/fixtures/windows/desktop-synthetic-100-150.json")).unwrap();
        let mons = v["monitors"].as_array().unwrap();
        assert_eq!(mons.len(), 2);
        let arr = |x: &serde_json::Value| -> [i32; 4] { let a = x.as_array().unwrap(); [0, 1, 2, 3].map(|i| a[i].as_i64().unwrap() as i32) };
        for m in mons {
            let got = monitor(m["name"].as_str().unwrap().into(), m["primary"].as_bool().unwrap(), m["scale"].as_f64().unwrap(), arr(&m["rect"]), arr(&m["work"]), 140.0);
            assert_eq!(got.taskbar, m["taskbar"].as_str().unwrap(), "{}", got.name);
            assert_eq!(got.strip, arr(&m["strip"]), "{}", got.name);
        }
    }
}
