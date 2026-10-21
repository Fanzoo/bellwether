//! The WebView2 throttling measure (Windows plan W1, WD2 step 2, risk WR1). With
//! `--heartbeat`, each strip's page logs a `heartbeat` line every 10 s from a 1 s timer
//! (main.js: how many ticks arrived, and the longest gap between two). A throttled or
//! suspended page logs late or not at all; this side notices the silence and says so in
//! the log, once per silence, and again when the page comes back. Works on macOS too.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// No heartbeat for this long (the page beats every 10 s) is a silence.
pub const SILENCE: Duration = Duration::from_secs(25);

#[derive(Default)]
pub struct Beats {
    last: HashMap<String, Instant>,
    silent: HashSet<String>,
}

impl Beats {
    /// A heartbeat from a strip's page. A line to log when it ends a silence.
    pub fn beat(&mut self, label: &str, now: Instant) -> Option<String> {
        let before = self.last.insert(label.to_string(), now);
        if self.silent.remove(label) {
            let gap = before.map(|b| now.duration_since(b).as_secs()).unwrap_or(0);
            return Some(format!("heartbeat: {label} is back after {gap} s of silence"));
        }
        None
    }

    /// The strips (of `live`) that have gone silent since the last check, as log lines.
    /// A strip that is gone is forgotten; one that never beat isn't judged.
    pub fn check(&mut self, now: Instant, live: &[String]) -> Vec<String> {
        self.last.retain(|l, _| live.contains(l));
        self.silent.retain(|l| live.contains(l));
        let mut out = Vec::new();
        let mut labels: Vec<&String> = self.last.keys().collect();
        labels.sort();
        for label in labels {
            let quiet = now.duration_since(self.last[label]);
            if quiet >= SILENCE && !self.silent.contains(label) {
                out.push(format!("heartbeat: {label} silent for {} s: its page is throttled or suspended (WR1)", quiet.as_secs()));
            }
        }
        for label in self.last.iter().filter(|(_, t)| now.duration_since(**t) >= SILENCE).map(|(l, _)| l.clone()).collect::<Vec<_>>() {
            self.silent.insert(label);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silence_is_said_once_and_its_end_once() {
        let t0 = Instant::now();
        let live = vec!["strip-0".to_string(), "strip-1".to_string()];
        let mut b = Beats::default();
        assert!(b.check(t0, &live).is_empty(), "nothing has beaten yet: nothing judged");
        assert_eq!(b.beat("strip-0", t0), None);
        assert_eq!(b.beat("strip-1", t0), None);
        assert!(b.check(t0 + Duration::from_secs(24), &live).is_empty());
        b.beat("strip-1", t0 + Duration::from_secs(20));
        assert_eq!(b.check(t0 + Duration::from_secs(26), &live), ["heartbeat: strip-0 silent for 26 s: its page is throttled or suspended (WR1)"]);
        assert!(b.check(t0 + Duration::from_secs(40), &live).is_empty(), "said once");
        assert_eq!(b.beat("strip-0", t0 + Duration::from_secs(70)).as_deref(), Some("heartbeat: strip-0 is back after 70 s of silence"));
        assert_eq!(b.beat("strip-0", t0 + Duration::from_secs(80)), None);
    }

    #[test]
    fn a_closed_strip_is_forgotten_not_reported() {
        let t0 = Instant::now();
        let mut b = Beats::default();
        b.beat("strip-3", t0);
        assert!(b.check(t0 + Duration::from_secs(60), &[]).is_empty());
        assert!(b.check(t0 + Duration::from_secs(90), &["strip-3".to_string()]).is_empty(), "forgotten: a new strip-3 has to beat first");
    }
}
