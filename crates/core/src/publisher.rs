//! Numbered snapshots for `/state`, coalesced (a port of brain/publisher.mjs): at most one
//! snapshot per `interval_ms` (250 ms), and a new `rev` only when the content changed. The
//! first change after a quiet spell publishes at once. Urgent changes (a new or newly
//! confirmed alert) skip the interval, and the interval restarts from them. A second,
//! hard rule that urgent publishes obey too: never more than `per_window` (4) snapshots in
//! any `window_ms` (1 s).
//!
//! No clock and no timers here: the caller passes `now` (wall-clock ms) and a closure that
//! builds the snapshot, and is told when to call [`Publisher::on_timer`] (crates/brain does
//! it with a tokio sleep; the conformance session with a thread).

use std::collections::VecDeque;
use std::sync::Arc;

use crate::json::{self, Js, Obj};

/// One published snapshot: `{rev, at, ...snap}` and its JSON text, ready to send.
#[derive(Clone, Debug)]
pub struct Published {
    pub rev: u64,
    pub at: f64,
    pub value: Js,
    pub body: Arc<String>,
}

impl Published {
    fn new(rev: u64, at: f64, snap: &Js) -> Published {
        let mut o = Obj::new().with("rev", rev as f64).with("at", at);
        if let Js::Obj(s) = snap {
            for (k, v) in s.iter() {
                o.set(k.clone(), v.clone());
            }
        }
        let value = Js::Obj(o);
        let body = Arc::new(json::stringify(&value));
        Published { rev, at, value, body }
    }
}

pub struct Publisher {
    interval_ms: f64,
    per_window: usize,
    window_ms: f64,
    last_body: String,
    current: Published,
    last_publish_at: f64,
    /// When a publish is due, if one is scheduled.
    timer_at: Option<f64>,
    urgent: bool,
    published: u64,
    recent: VecDeque<f64>,
}

impl Publisher {
    pub fn new(first: &Js, now: f64) -> Publisher {
        Publisher::with_limits(first, now, 250.0, 4, 1000.0)
    }

    pub fn with_limits(first: &Js, now: f64, interval_ms: f64, per_window: usize, window_ms: f64) -> Publisher {
        Publisher {
            interval_ms,
            per_window,
            window_ms,
            last_body: json::stringify(first),
            current: Published::new(0, now, first),
            last_publish_at: f64::NEG_INFINITY,
            timer_at: None,
            urgent: false,
            published: 0,
            recent: VecDeque::new(),
        }
    }

    pub fn current(&self) -> &Published {
        &self.current
    }

    /// Snapshots with new content so far.
    pub fn published(&self) -> u64 {
        self.published
    }

    /// When the caller must call `on_timer`, if a publish is scheduled.
    pub fn timer_at(&self) -> Option<f64> {
        self.timer_at
    }

    /// The earliest time the next snapshot may go out.
    fn earliest(&self) -> f64 {
        let by_window = if self.recent.len() >= self.per_window { self.recent[self.recent.len() - self.per_window] + self.window_ms } else { f64::NEG_INFINITY };
        let by_interval = if self.urgent { f64::NEG_INFINITY } else { self.last_publish_at + self.interval_ms };
        by_window.max(by_interval)
    }

    fn schedule(&mut self) {
        let at = self.earliest();
        if self.timer_at.is_some_and(|t| t <= at) {
            return;
        }
        self.timer_at = Some(at);
    }

    /// Something may have changed; publish when the throttle allows. Returns true when a
    /// new rev went out now.
    pub fn mark_dirty(&mut self, urgent: bool, now: f64, build: impl FnOnce() -> Js) -> bool {
        if urgent {
            self.urgent = true;
            if self.earliest() <= now {
                return self.publish(now, build);
            }
        }
        self.schedule();
        false
    }

    /// The scheduled time has come (a timer may fire a little early; that is re-checked).
    pub fn on_timer(&mut self, now: f64, build: impl FnOnce() -> Js) -> bool {
        self.publish(now, build)
    }

    fn publish(&mut self, now: f64, build: impl FnOnce() -> Js) -> bool {
        self.timer_at = None;
        // Timers can fire a millisecond early; re-check, or five snapshots could fit in a second.
        if self.earliest() > now {
            self.schedule();
            return false;
        }
        self.urgent = false;
        let snap = build();
        let body = json::stringify(&snap);
        self.last_publish_at = now;
        if body == self.last_body {
            return false;
        }
        self.last_body = body;
        self.published += 1;
        self.recent.push_back(now);
        if self.recent.len() > self.per_window {
            self.recent.pop_front();
        }
        self.current = Published::new(self.current.rev + 1, now, &snap);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(n: u32) -> Js {
        Js::Obj(Obj::new().with("n", n as f64))
    }

    /// publisher.test cases from brain-server.test.mjs, on a fake clock.
    #[test]
    fn urgent_goes_at_once_and_the_window_caps_everything() {
        let mut p = Publisher::new(&snap(0), 0.0);
        assert!(!p.mark_dirty(false, 1000.0, || snap(1)));
        assert_eq!(p.timer_at(), Some(f64::NEG_INFINITY.max(f64::NEG_INFINITY)));
        assert!(p.on_timer(1000.0, || snap(1)));
        assert_eq!(p.current().rev, 1);
        assert!(!p.mark_dirty(false, 1010.0, || snap(2)), "inside the interval: waits");
        assert_eq!(p.timer_at(), Some(1250.0));
        assert!(p.mark_dirty(true, 1020.0, || snap(3)), "urgent: now");
        assert_eq!(p.current().rev, 2);
        assert_eq!(p.current().value.get("n"), &Js::Num(3.0));
        assert!(!p.on_timer(1100.0, || snap(4)), "the interval restarts from the urgent one");
        assert!(p.on_timer(1270.0, || snap(4)));
        for (i, t) in [1300.0, 1310.0, 1320.0, 1330.0, 1340.0].iter().enumerate() {
            p.mark_dirty(true, *t, || snap(10 + i as u32));
        }
        assert_eq!(p.current().rev, 4, "capped at 4 snapshots in any second");
        assert_eq!(p.timer_at(), Some(2000.0));
    }
}
