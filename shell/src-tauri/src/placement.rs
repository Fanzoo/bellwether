//! A strip per monitor: which monitor each sheep lives on, which
//! monitors need a strip, each sheep's slot there, and what each strip window sees.
//! Pure logic, tested with made-up monitor lists; lib.rs runs it on its own thread.
//!
//! - A sheep is (re)located when it first appears, when it raises an alert, every
//!   `strip.relocateSec`, and when the displays change. Until then, and whenever the
//!   locator finds nothing, it lives on the primary monitor.
//! - The primary monitor always has a strip; another monitor has one while at least
//!   one sheep lives there.
//! - Slots: on the primary monitor the brain's `slot`; elsewhere the slot the brain
//!   remembers for that monitor (`slots[<monitor>]`, from earlier `placed` reports) if
//!   it is free, else the lowest free one, which is then reported back (`placed`).
//! - Drops (D10, milestone 12): a sheep put down by hand lives on the monitor it was
//!   dropped on, around the drop, until the app restarts, its session ends, that monitor
//!   goes, or it is dragged again. Placement leaves it alone meanwhile, and nothing about
//!   it is reported to the brain or saved: the table is in memory only.

use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::locator::{monitor_for, Located, Monitor};

#[derive(Clone, Debug)]
pub struct Placed {
    pub monitor: String,
    pub at: Instant,
    /// The alert this placement was made for ("" when calm): a new one re-locates.
    pub alert: String,
}

/// One sheep as placement sees it.
#[derive(Clone, Debug)]
pub struct Sheep {
    pub identity: String,
    pub label: String,
    pub alert: String,
    pub terminal: Value,
    pub slots: HashMap<String, i64>,
    /// Its session is ending (`leaving`).
    pub leaving: bool,
}

/// A sheep put down by hand: on `monitor`, its centre at `centre_x` in that strip's
/// logical px. `seq` goes up by one per drop; `sheep` is its sprite id, for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub monitor: String,
    pub centre_x: f64,
    pub seq: u64,
    pub sheep: String,
}

pub fn sheep_of(snap: &Value) -> Vec<Sheep> {
    snap.get("flock")
        .and_then(Value::as_array)
        .map(|f| {
            f.iter()
                .filter_map(|s| {
                    let alert = match (s.pointer("/alert/kind").and_then(Value::as_str), s.get("lambAlert").and_then(Value::as_bool)) {
                        (Some(k), _) => format!("{k}@{}", s.pointer("/alert/since").and_then(Value::as_f64).unwrap_or(0.0)),
                        (None, Some(true)) => "lamb".into(),
                        _ => String::new(),
                    };
                    Some(Sheep {
                        identity: s.get("identityKey")?.as_str()?.to_string(),
                        label: s.get("label").and_then(Value::as_str).unwrap_or("").to_string(),
                        alert,
                        terminal: s.get("terminal").cloned().unwrap_or(Value::Null),
                        slots: s.get("slots").and_then(Value::as_object).map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_i64()?))).collect()).unwrap_or_default(),
                        leaving: s.get("state").and_then(Value::as_str) == Some("leaving"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a sheep should be located (again) now. Never while it is dropped: no
/// relocation on the timer, on a new alert or back to its terminal's monitor.
pub fn needs_locate(prev: Option<&Placed>, sheep: &Sheep, now: Instant, relocate: Duration, displays_changed: bool, dropped: bool) -> bool {
    if dropped {
        return false;
    }
    match prev {
        None => true,
        Some(p) => displays_changed || now.duration_since(p.at) >= relocate || (!sheep.alert.is_empty() && sheep.alert != p.alert),
    }
}

/// The monitor a locate result points at, or the primary one.
pub fn monitor_of(located: &Located, monitors: &[Monitor], primary: &str) -> String {
    located.bounds.as_ref().and_then(|b| monitor_for(b, monitors)).map(|m| m.name.clone()).unwrap_or_else(|| primary.to_string())
}

/// The monitor a sheep lives on: where it was dropped, else where it was placed.
pub fn home_of<'a>(id: &str, placed: &'a HashMap<String, Placed>, dropped: &'a HashMap<String, Dropped>) -> Option<&'a str> {
    dropped.get(id).map(|d| d.monitor.as_str()).or_else(|| placed.get(id).map(|p| p.monitor.as_str()))
}

/// The monitors that need a strip: the primary, and every connected monitor that
/// hosts a sheep (placed or dropped there). While a sheep is being carried, every
/// connected monitor, so the hand-off is ready when the cursor gets there.
pub fn strips_needed(primary: &str, monitors: &[Monitor], placed: &HashMap<String, Placed>, present: &HashSet<String>, dropped: &HashMap<String, Dropped>, dragging: bool) -> BTreeSet<String> {
    let connected: HashSet<&str> = monitors.iter().map(|m| m.name.as_str()).collect();
    let mut out = BTreeSet::from([primary.to_string()]);
    if dragging {
        out.extend(connected.iter().map(|m| m.to_string()));
    }
    for id in present {
        if let Some(m) = home_of(id, placed, dropped).filter(|m| connected.contains(m)) {
            out.insert(m.to_string());
        }
    }
    out
}

/// The drops that end now, and why: the session ended (the sheep is leaving or gone from
/// the flock), or the monitor it was dropped on is no longer connected.
pub fn ended_drops(dropped: &HashMap<String, Dropped>, sheep: &[Sheep], monitors: &[Monitor]) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = dropped
        .iter()
        .filter_map(|(id, d)| {
            let reason = match sheep.iter().find(|s| &s.identity == id) {
                None => "its session ended",
                Some(s) if s.leaving => "its session ended",
                Some(_) if !monitors.iter().any(|m| m.name == d.monitor) => "its monitor was disconnected",
                Some(_) => return None,
            };
            Some((id.clone(), reason))
        })
        .collect();
    out.sort();
    out
}

/// Slots on the monitors other than the primary. Returns the new slot book and the
/// `(identity, monitor, slot)` placements to report to the brain (new or changed). A
/// dropped sheep keeps its place in the book (its slot's band stays) but is never
/// reported: a drop is not persisted.
pub fn assign_slots(
    sheep: &[Sheep],
    placed: &HashMap<String, Placed>,
    primary: &str,
    book: &HashMap<String, (String, i64)>,
    dropped: &HashMap<String, Dropped>,
) -> (HashMap<String, (String, i64)>, Vec<(String, String, i64)>) {
    let mut out: HashMap<String, (String, i64)> = HashMap::new();
    let mut report = Vec::new();
    let mut taken: HashMap<String, HashSet<i64>> = HashMap::new();
    let on = |s: &Sheep| placed.get(&s.identity).map(|p| p.monitor.clone()).filter(|m| m != primary);
    // Keep what's already assigned on the same monitor first, so nobody jumps.
    let mut order: Vec<&Sheep> = sheep.iter().filter(|s| on(s).is_some()).collect();
    order.sort_by_key(|s| (book.get(&s.identity).map_or(1, |_| 0), s.identity.clone()));
    for s in order {
        let m = on(s).unwrap();
        let t = taken.entry(m.clone()).or_default();
        let kept = book.get(&s.identity).filter(|(bm, slot)| *bm == m && !t.contains(slot)).map(|(_, slot)| *slot);
        let remembered = s.slots.get(&m).copied().filter(|slot| *slot >= 0 && !t.contains(slot));
        let slot = kept.or(remembered).unwrap_or_else(|| (0..).find(|n| !t.contains(n)).unwrap());
        t.insert(slot);
        if s.slots.get(&m) != Some(&slot) && !dropped.contains_key(&s.identity) {
            report.push((s.identity.clone(), m.clone(), slot));
        }
        out.insert(s.identity.clone(), (m, slot));
    }
    (out, report)
}

/// What one strip window sees: the sheep living on its monitor (a sheep whose monitor
/// has no strip yet stays on the primary), with that monitor's slot. A sheep dropped
/// there carries `dropped: {centreX, seq, ghost}`; `ghost` when it was dropped on the
/// monitor placement would put it on anyway (its slot's band stays, empty).
pub fn view_for(snap: &Value, window_monitor: &str, primary: &str, placed: &HashMap<String, Placed>, open: &HashSet<String>, book: &HashMap<String, (String, i64)>, dropped: &HashMap<String, Dropped>) -> Value {
    let mut s = snap.clone();
    let Some(flock) = s.get_mut("flock").and_then(Value::as_array_mut) else { return s };
    flock.retain(|sheep| {
        let id = sheep.get("identityKey").and_then(Value::as_str).unwrap_or("");
        let home = home_of(id, placed, dropped).filter(|m| open.contains(*m)).unwrap_or(primary);
        home == window_monitor
    });
    for sheep in flock.iter_mut() {
        let id = sheep.get("identityKey").and_then(Value::as_str).unwrap_or("").to_string();
        if window_monitor != primary {
            if let Some((_, slot)) = book.get(&id) {
                sheep["slot"] = Value::from(*slot);
            }
        }
        if let Some(d) = dropped.get(&id).filter(|d| d.monitor == window_monitor) {
            let located = placed.get(&id).map(|p| p.monitor.as_str()).unwrap_or(primary);
            sheep["dropped"] = serde_json::json!({"centreX": d.centre_x, "seq": d.seq, "ghost": located == d.monitor});
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locator::{Confidence, Rect};
    use serde_json::json;

    fn monitors() -> Vec<Monitor> {
        vec![
            Monitor { name: "Built-in".into(), rect: Rect { x: 0.0, y: 0.0, w: 1728.0, h: 1117.0 }, primary: true },
            Monitor { name: "DELL".into(), rect: Rect { x: 1728.0, y: 0.0, w: 2560.0, h: 1440.0 }, primary: false },
        ]
    }
    fn placed(pairs: &[(&str, &str)]) -> HashMap<String, Placed> {
        pairs.iter().map(|(id, m)| (id.to_string(), Placed { monitor: m.to_string(), at: Instant::now(), alert: String::new() })).collect()
    }
    fn sheep(id: &str, slots: &[(&str, i64)]) -> Sheep {
        Sheep { identity: id.into(), label: id.into(), alert: String::new(), terminal: Value::Null, slots: slots.iter().map(|(m, s)| (m.to_string(), *s)).collect(), leaving: false }
    }
    fn none() -> HashMap<String, Dropped> {
        HashMap::new()
    }
    fn dropped(pairs: &[(&str, &str, f64, u64)]) -> HashMap<String, Dropped> {
        pairs.iter().map(|(id, m, x, seq)| (id.to_string(), Dropped { monitor: m.to_string(), centre_x: *x, seq: *seq, sheep: format!("s-{id}") })).collect()
    }

    #[test]
    fn a_located_window_puts_the_sheep_on_its_monitor_and_nothing_found_means_primary() {
        let at = |x: f64| Located { strategy: "t".into(), bounds: Some(Rect { x, y: 100.0, w: 800.0, h: 600.0 }), confidence: Confidence::App };
        assert_eq!(monitor_of(&at(2000.0), &monitors(), "Built-in"), "DELL");
        assert_eq!(monitor_of(&at(100.0), &monitors(), "Built-in"), "Built-in");
        let none = Located { strategy: "none".into(), bounds: None, confidence: Confidence::None };
        assert_eq!(monitor_of(&none, &monitors(), "Built-in"), "Built-in");
        assert_eq!(monitor_of(&at(2000.0), &monitors()[..1], "Built-in"), "Built-in", "the second monitor was unplugged");
    }

    #[test]
    fn relocation_happens_on_first_sight_on_a_new_alert_on_the_timer_and_on_display_changes() {
        let now = Instant::now();
        let s = sheep("a", &[]);
        let p = Placed { monitor: "DELL".into(), at: now, alert: String::new() };
        let r = Duration::from_secs(60);
        assert!(needs_locate(None, &s, now, r, false, false));
        assert!(!needs_locate(Some(&p), &s, now + Duration::from_secs(10), r, false, false));
        assert!(needs_locate(Some(&p), &s, now + Duration::from_secs(60), r, false, false));
        assert!(needs_locate(Some(&p), &s, now, r, true, false));
        let alerting = Sheep { alert: "permission@5".into(), ..s.clone() };
        assert!(needs_locate(Some(&p), &alerting, now, r, false, false), "a new alert");
        let p2 = Placed { alert: "permission@5".into(), ..p };
        assert!(!needs_locate(Some(&p2), &alerting, now, r, false, false), "the same alert again");
    }

    #[test]
    fn strips_exist_for_the_primary_and_for_monitors_hosting_present_sheep() {
        let present: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let p = placed(&[("a", "Built-in"), ("b", "DELL"), ("gone", "Projector")]);
        assert_eq!(strips_needed("Built-in", &monitors(), &p, &present, &none(), false), BTreeSet::from(["Built-in".to_string(), "DELL".to_string()]));
        let p = placed(&[("a", "Built-in"), ("b", "Built-in")]);
        assert_eq!(strips_needed("Built-in", &monitors(), &p, &present, &none(), false), BTreeSet::from(["Built-in".to_string()]), "the last sheep left the DELL");
        let p = placed(&[("b", "DELL")]);
        assert_eq!(strips_needed("Built-in", &monitors()[..1], &p, &present, &none(), false).len(), 1, "a disconnected monitor gets no strip");
    }

    #[test]
    fn slots_on_another_monitor_reuse_what_the_brain_remembers_and_report_new_ones() {
        let p = placed(&[("a", "DELL"), ("b", "DELL"), ("c", "Built-in")]);
        let flock = vec![sheep("a", &[("DELL", 3)]), sheep("b", &[]), sheep("c", &[])];
        let (book, report) = assign_slots(&flock, &p, "Built-in", &HashMap::new(), &none());
        assert_eq!(book["a"], ("DELL".to_string(), 3));
        assert_eq!(book["b"], ("DELL".to_string(), 0));
        assert!(!book.contains_key("c"), "the primary uses the brain's slot");
        assert_eq!(report, vec![("b".to_string(), "DELL".to_string(), 0)], "only the new one is reported");
        // Stable: run again with the book, nothing moves or is reported.
        let flock = vec![sheep("a", &[("DELL", 3)]), sheep("b", &[("DELL", 0)])];
        let (again, report) = assign_slots(&flock, &p, "Built-in", &book, &none());
        assert_eq!(again, book.into_iter().filter(|(k, _)| k != "c").collect());
        assert!(report.is_empty());
        // Two remembering the same slot: the one already placed keeps it.
        let flock = vec![sheep("a", &[("DELL", 1)]), sheep("b", &[("DELL", 1)])];
        let held = HashMap::from([("b".to_string(), ("DELL".to_string(), 1))]);
        let (book, _) = assign_slots(&flock, &p, "Built-in", &held, &none());
        assert_eq!(book["b"].1, 1);
        assert_ne!(book["a"].1, 1);
    }

    #[test]
    fn each_window_sees_only_its_monitor_and_a_monitor_without_a_strip_falls_back_to_primary() {
        let snap = json!({"rev": 3, "flock": [
            {"identityKey": "a", "slot": 0}, {"identityKey": "b", "slot": 1}, {"identityKey": "c", "slot": 2}, {"identityKey": "new", "slot": 4}
        ]});
        let p = placed(&[("a", "Built-in"), ("b", "DELL"), ("c", "Projector")]);
        let open: HashSet<String> = ["Built-in", "DELL"].iter().map(|s| s.to_string()).collect();
        let book = HashMap::from([("b".to_string(), ("DELL".to_string(), 0))]);
        let ids = |v: &Value| v["flock"].as_array().unwrap().iter().map(|s| s["identityKey"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let primary = view_for(&snap, "Built-in", "Built-in", &p, &open, &book, &none());
        assert_eq!(ids(&primary), vec!["a", "c", "new"], "c's monitor has no strip; new isn't located yet");
        let dell = view_for(&snap, "DELL", "Built-in", &p, &open, &book, &none());
        assert_eq!(ids(&dell), vec!["b"]);
        assert_eq!(dell["flock"][0]["slot"], 0, "the DELL slot, not the brain's primary slot");
        assert_eq!(primary["rev"], 3);
    }

    #[test]
    fn sheep_are_read_from_the_snapshot_with_alerts_and_remembered_slots() {
        let snap = json!({"flock": [
            {"identityKey": "a", "label": "meadow", "slot": 2, "slots": {"primary": 2, "DELL": 5}, "terminal": {"herdrPane": "w1:p1"}, "alert": {"kind": "permission", "since": 7.0}},
            {"identityKey": "b", "lambAlert": true}, {"label": "no identity"}
        ]});
        let s = sheep_of(&snap);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].alert.as_str(), s[0].slots["DELL"]), ("permission@7", 5));
        assert_eq!(s[1].alert, "lamb");
    }

    // D10, milestone 12: drops.

    #[test]
    fn a_dropped_sheep_is_never_located_again_while_dropped() {
        let now = Instant::now();
        let s = sheep("a", &[]);
        let p = Placed { monitor: "DELL".into(), at: now, alert: String::new() };
        let r = Duration::from_secs(60);
        assert!(!needs_locate(Some(&p), &s, now + Duration::from_secs(600), r, false, true), "not on the strip.relocateSec timer");
        let alerting = Sheep { alert: "permission@5".into(), ..s.clone() };
        assert!(!needs_locate(Some(&p), &alerting, now, r, false, true), "not on a new alert");
        assert!(!needs_locate(Some(&p), &s, now, r, true, true), "not when the displays change (its monitor is still there)");
        assert!(!needs_locate(None, &s, now, r, false, true));
    }

    #[test]
    fn no_placed_report_is_ever_sent_for_a_dropped_sheep() {
        let p = placed(&[("a", "DELL"), ("b", "DELL")]);
        let flock = vec![sheep("a", &[]), sheep("b", &[])];
        let d = dropped(&[("a", "DELL", 400.0, 1)]);
        let (book, report) = assign_slots(&flock, &p, "Built-in", &HashMap::new(), &d);
        assert_eq!(report, vec![("b".to_string(), "DELL".to_string(), 1)], "only b; a's slot stays in the book, unreported");
        assert_eq!(book["a"], ("DELL".to_string(), 0), "a keeps its band on its own monitor (the ghost band)");
        // Dropped onto the primary while placed on the DELL: still never reported.
        let d = dropped(&[("a", "Built-in", 400.0, 2), ("b", "Built-in", 900.0, 3)]);
        let (_, report) = assign_slots(&flock, &p, "Built-in", &HashMap::new(), &d);
        assert!(report.is_empty());
    }

    #[test]
    fn strips_open_on_every_monitor_while_a_sheep_is_carried_and_where_one_was_dropped() {
        let present: HashSet<String> = ["a"].iter().map(|s| s.to_string()).collect();
        let p = placed(&[("a", "Built-in")]);
        let three = {
            let mut m = monitors();
            m.push(Monitor { name: "Projector".into(), rect: Rect { x: 0.0, y: -1080.0, w: 1920.0, h: 1080.0 }, primary: false });
            m
        };
        assert_eq!(strips_needed("Built-in", &three, &p, &present, &none(), false).len(), 1, "no drag: only the primary");
        assert_eq!(strips_needed("Built-in", &three, &p, &present, &none(), true), BTreeSet::from(["Built-in".to_string(), "DELL".to_string(), "Projector".to_string()]), "a drag: every connected monitor");
        let d = dropped(&[("a", "DELL", 300.0, 1)]);
        assert_eq!(strips_needed("Built-in", &three, &p, &present, &d, false), BTreeSet::from(["Built-in".to_string(), "DELL".to_string()]), "after the drop: the drop's monitor");
        let gone: HashSet<String> = HashSet::new();
        assert_eq!(strips_needed("Built-in", &three, &p, &gone, &d, false).len(), 1, "the dropped sheep has gone: its strip closes");
        assert_eq!(strips_needed("Built-in", &three[..1], &p, &present, &d, false).len(), 1, "a disconnected monitor gets no strip");
    }

    #[test]
    fn view_for_routes_a_dropped_sheep_to_its_monitor_and_says_where_it_was_dropped() {
        let snap = json!({"rev": 3, "flock": [{"identityKey": "a", "slot": 0}, {"identityKey": "b", "slot": 1}]});
        let p = placed(&[("a", "Built-in"), ("b", "Built-in")]);
        let open: HashSet<String> = ["Built-in", "DELL"].iter().map(|s| s.to_string()).collect();
        let book = HashMap::new();
        let ids = |v: &Value| v["flock"].as_array().unwrap().iter().map(|s| s["identityKey"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        // a dropped on the DELL: a guest there.
        let d = dropped(&[("a", "DELL", 812.5, 4)]);
        let dell = view_for(&snap, "DELL", "Built-in", &p, &open, &book, &d);
        assert_eq!(ids(&dell), vec!["a"]);
        assert_eq!(dell["flock"][0]["dropped"], json!({"centreX": 812.5, "seq": 4, "ghost": false}));
        assert_eq!(ids(&view_for(&snap, "Built-in", "Built-in", &p, &open, &book, &d)), vec!["b"], "the strip it left no longer has it");
        // b dropped on its own monitor: a ghost (its band stays).
        let d = dropped(&[("b", "Built-in", 100.0, 5)]);
        let primary = view_for(&snap, "Built-in", "Built-in", &p, &open, &book, &d);
        assert_eq!(primary["flock"][1]["dropped"], json!({"centreX": 100.0, "seq": 5, "ghost": true}));
        assert!(primary["flock"][0].get("dropped").is_none());
        assert_eq!(primary["flock"][1]["slot"], 1, "the brain's slot is unchanged");
        // Dropped on a monitor whose strip isn't open: it stays where it would be, with no drop.
        let d = dropped(&[("a", "Projector", 10.0, 6)]);
        let primary = view_for(&snap, "Built-in", "Built-in", &p, &open, &book, &d);
        assert_eq!(ids(&primary), vec!["a", "b"]);
        assert!(primary["flock"][0].get("dropped").is_none());
        // Not located yet (no placement): its normal monitor is the primary, so a drop there is a ghost.
        let d = dropped(&[("a", "Built-in", 10.0, 7)]);
        let primary = view_for(&snap, "Built-in", "Built-in", &HashMap::new(), &open, &book, &d);
        assert_eq!(primary["flock"][0]["dropped"]["ghost"], true);
    }

    #[test]
    fn a_drop_ends_when_its_sheep_leaves_or_is_gone_or_its_monitor_is_disconnected() {
        let d = dropped(&[("a", "DELL", 1.0, 1), ("b", "DELL", 2.0, 2), ("c", "DELL", 3.0, 3), ("e", "Built-in", 4.0, 4)]);
        let flock = vec![Sheep { leaving: true, ..sheep("a", &[]) }, sheep("c", &[]), sheep("e", &[])];
        assert_eq!(
            ended_drops(&d, &flock, &monitors()),
            vec![("a".to_string(), "its session ended"), ("b".to_string(), "its session ended")],
            "a is leaving, b is gone; c and e stay"
        );
        let ended = ended_drops(&d, &flock, &monitors()[..1]);
        assert!(ended.contains(&("c".to_string(), "its monitor was disconnected")));
        assert!(!ended.iter().any(|(id, _)| id == "e"), "e's monitor is still there");
        assert!(ended_drops(&none(), &flock, &monitors()).is_empty());
    }

    #[test]
    fn a_leaving_sheep_is_read_as_leaving() {
        let s = sheep_of(&json!({"flock": [{"identityKey": "a", "state": "leaving"}, {"identityKey": "b", "state": "busy"}]}));
        assert!(s[0].leaving && !s[1].leaving);
    }
}
