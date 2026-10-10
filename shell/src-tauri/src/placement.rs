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
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a sheep should be located (again) now.
pub fn needs_locate(prev: Option<&Placed>, sheep: &Sheep, now: Instant, relocate: Duration, displays_changed: bool) -> bool {
    match prev {
        None => true,
        Some(p) => displays_changed || now.duration_since(p.at) >= relocate || (!sheep.alert.is_empty() && sheep.alert != p.alert),
    }
}

/// The monitor a locate result points at, or the primary one.
pub fn monitor_of(located: &Located, monitors: &[Monitor], primary: &str) -> String {
    located.bounds.as_ref().and_then(|b| monitor_for(b, monitors)).map(|m| m.name.clone()).unwrap_or_else(|| primary.to_string())
}

/// The monitors that need a strip: the primary, and every connected monitor that
/// hosts a sheep.
pub fn strips_needed(primary: &str, monitors: &[Monitor], placed: &HashMap<String, Placed>, present: &HashSet<String>) -> BTreeSet<String> {
    let connected: HashSet<&str> = monitors.iter().map(|m| m.name.as_str()).collect();
    let mut out = BTreeSet::from([primary.to_string()]);
    for (id, p) in placed {
        if present.contains(id) && connected.contains(p.monitor.as_str()) {
            out.insert(p.monitor.clone());
        }
    }
    out
}

/// Slots on the monitors other than the primary. Returns the new slot book and the
/// `(identity, monitor, slot)` placements to report to the brain (new or changed).
pub fn assign_slots(
    sheep: &[Sheep],
    placed: &HashMap<String, Placed>,
    primary: &str,
    book: &HashMap<String, (String, i64)>,
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
        if s.slots.get(&m) != Some(&slot) {
            report.push((s.identity.clone(), m.clone(), slot));
        }
        out.insert(s.identity.clone(), (m, slot));
    }
    (out, report)
}

/// What one strip window sees: the sheep living on its monitor (a sheep whose monitor
/// has no strip yet stays on the primary), with that monitor's slot.
pub fn view_for(snap: &Value, window_monitor: &str, primary: &str, placed: &HashMap<String, Placed>, open: &HashSet<String>, book: &HashMap<String, (String, i64)>) -> Value {
    let mut s = snap.clone();
    let Some(flock) = s.get_mut("flock").and_then(Value::as_array_mut) else { return s };
    flock.retain(|sheep| {
        let id = sheep.get("identityKey").and_then(Value::as_str).unwrap_or("");
        let home = placed.get(id).map(|p| p.monitor.as_str()).filter(|m| open.contains(*m)).unwrap_or(primary);
        home == window_monitor
    });
    if window_monitor != primary {
        for sheep in flock.iter_mut() {
            let id = sheep.get("identityKey").and_then(Value::as_str).unwrap_or("").to_string();
            if let Some((_, slot)) = book.get(&id) {
                sheep["slot"] = Value::from(*slot);
            }
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
        Sheep { identity: id.into(), label: id.into(), alert: String::new(), terminal: Value::Null, slots: slots.iter().map(|(m, s)| (m.to_string(), *s)).collect() }
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
        assert!(needs_locate(None, &s, now, r, false));
        assert!(!needs_locate(Some(&p), &s, now + Duration::from_secs(10), r, false));
        assert!(needs_locate(Some(&p), &s, now + Duration::from_secs(60), r, false));
        assert!(needs_locate(Some(&p), &s, now, r, true));
        let alerting = Sheep { alert: "permission@5".into(), ..s.clone() };
        assert!(needs_locate(Some(&p), &alerting, now, r, false), "a new alert");
        let p2 = Placed { alert: "permission@5".into(), ..p };
        assert!(!needs_locate(Some(&p2), &alerting, now, r, false), "the same alert again");
    }

    #[test]
    fn strips_exist_for_the_primary_and_for_monitors_hosting_present_sheep() {
        let present: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let p = placed(&[("a", "Built-in"), ("b", "DELL"), ("gone", "Projector")]);
        assert_eq!(strips_needed("Built-in", &monitors(), &p, &present), BTreeSet::from(["Built-in".to_string(), "DELL".to_string()]));
        let p = placed(&[("a", "Built-in"), ("b", "Built-in")]);
        assert_eq!(strips_needed("Built-in", &monitors(), &p, &present), BTreeSet::from(["Built-in".to_string()]), "the last sheep left the DELL");
        let p = placed(&[("b", "DELL")]);
        assert_eq!(strips_needed("Built-in", &monitors()[..1], &p, &present).len(), 1, "a disconnected monitor gets no strip");
    }

    #[test]
    fn slots_on_another_monitor_reuse_what_the_brain_remembers_and_report_new_ones() {
        let p = placed(&[("a", "DELL"), ("b", "DELL"), ("c", "Built-in")]);
        let flock = vec![sheep("a", &[("DELL", 3)]), sheep("b", &[]), sheep("c", &[])];
        let (book, report) = assign_slots(&flock, &p, "Built-in", &HashMap::new());
        assert_eq!(book["a"], ("DELL".to_string(), 3));
        assert_eq!(book["b"], ("DELL".to_string(), 0));
        assert!(!book.contains_key("c"), "the primary uses the brain's slot");
        assert_eq!(report, vec![("b".to_string(), "DELL".to_string(), 0)], "only the new one is reported");
        // Stable: run again with the book, nothing moves or is reported.
        let flock = vec![sheep("a", &[("DELL", 3)]), sheep("b", &[("DELL", 0)])];
        let (again, report) = assign_slots(&flock, &p, "Built-in", &book);
        assert_eq!(again, book.into_iter().filter(|(k, _)| k != "c").collect());
        assert!(report.is_empty());
        // Two remembering the same slot: the one already placed keeps it.
        let flock = vec![sheep("a", &[("DELL", 1)]), sheep("b", &[("DELL", 1)])];
        let held = HashMap::from([("b".to_string(), ("DELL".to_string(), 1))]);
        let (book, _) = assign_slots(&flock, &p, "Built-in", &held);
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
        let primary = view_for(&snap, "Built-in", "Built-in", &p, &open, &book);
        assert_eq!(ids(&primary), vec!["a", "c", "new"], "c's monitor has no strip; new isn't located yet");
        let dell = view_for(&snap, "DELL", "Built-in", &p, &open, &book);
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
}
