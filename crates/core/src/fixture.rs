//! The golden-fixture runner (a port of test/fixture-runner.mjs, distribution plan D2
//! point 3): replays a recording through the reducer with a fake clock. Each event is
//! applied at its own receivedAt, and the flock is ticked every `step_ms` from the first
//! event to `tail_ms` after the last. The transitions are one line whenever an actor's
//! shown state, flavour or alert changes, with times relative to the first event. The
//! cargo test (tests/fixtures.rs) and `__fixture` compare them byte for byte with the
//! `*.expected.jsonl` files the Node runner made.

use indexmap::IndexMap;

use crate::flock::{Flock, FlockConfig};
use crate::json::{self, Js, Obj};
use crate::normalise::HookEvent;

pub struct FixtureRun {
    pub transitions: Vec<Js>,
    pub snapshots: Vec<Js>,
}

/// `runFixture(lines, {config, stepMs, tailMs})`. `lines` are recording lines
/// (`{normalised, rawKeys, headersPresent}`); `merged` is the merged config.
pub fn run_fixture(lines: &[Js], merged: &Js, step_ms: f64, tail_ms: f64, keep_snapshots: bool) -> FixtureRun {
    let events: Vec<HookEvent> = lines.iter().map(|l| HookEvent::from_js(l.get("normalised"))).collect();
    let mut out = Vec::new();
    let mut snapshots = Vec::new();
    let Some(first) = events.first() else { return FixtureRun { transitions: out, snapshots } };
    let t0 = first.received_at;
    let mut flock = Flock::new(FlockConfig::from_merged(merged), &Js::Null, t0);
    let mut last: IndexMap<String, String> = IndexMap::new();

    let mut record = |flock: &Flock, t: f64, out: &mut Vec<Js>, snapshots: &mut Vec<Js>| {
        let snap = flock.snapshot();
        let mut seen = std::collections::HashSet::new();
        let mut note = |actor: String, a: &Js, out: &mut Vec<Js>| {
            seen.insert(actor.clone());
            let mut view = Obj::new().with("state", a.get("state").clone());
            if a.get("flavour").truthy() {
                view.set("flavour", a.get("flavour").clone());
            }
            if a.get("alert").truthy() {
                let al = a.get("alert");
                let text = format!("{}: {}{}", al.get("kind").as_str().unwrap_or(""), al.get("text").as_str().unwrap_or(""), if al.get("confirmed").truthy() { " (confirmed)" } else { "" });
                view.set("alert", text);
            }
            let key = json::stringify(&Js::Obj(view.clone()));
            if last.get(&actor) != Some(&key) {
                last.insert(actor.clone(), key);
                let mut line = Obj::new().with("atMs", t - t0).with("actor", actor.as_str());
                for (k, v) in view.iter() {
                    line.set(k.clone(), v.clone());
                }
                out.push(Js::Obj(line));
            }
        };
        for s in snap.get("flock").as_arr().cloned().unwrap_or_default() {
            note(format!("sheep:{}", s.get("identityKey").as_str().unwrap_or("")), &s, out);
            for l in s.get("lambs").as_arr().cloned().unwrap_or_default() {
                note(format!("lamb:{}:{}", l.get("agentType").as_str().unwrap_or(""), l.get("id").as_str().unwrap_or("")), &l, out);
            }
        }
        let gone: Vec<String> = last.iter().filter(|(actor, v)| !seen.contains(*actor) && v.as_str() != "gone").map(|(a, _)| a.clone()).collect();
        for actor in gone {
            last.insert(actor.clone(), "gone".into());
            out.push(Js::Obj(Obj::new().with("atMs", t - t0).with("actor", actor.as_str()).with("state", "gone")));
        }
        if keep_snapshots {
            snapshots.push(snap);
        }
    };

    let mut i = 0;
    let end = events.last().unwrap().received_at + tail_ms;
    let mut t = t0;
    while t <= end {
        while i < events.len() && events[i].received_at <= t {
            flock.apply(&events[i]);
            record(&flock, events[i].received_at, &mut out, &mut snapshots);
            i += 1;
        }
        flock.tick(t);
        record(&flock, t, &mut out, &mut snapshots);
        t += step_ms;
    }
    FixtureRun { transitions: out, snapshots }
}
