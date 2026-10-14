//! `bellwether status`: the flock as text (a port of brain/status.mjs).
//!
//!   meadow           #7fb3ff  busy·running     ALERT  Bash: git push origin main?
//!     └ Explore      lamb     busy·reading

use crate::json::{number_to_string, Js};
use crate::jsstr;

/// `String(v)` for the values a snapshot holds.
pub fn js_string(v: &Js) -> String {
    match v {
        Js::Str(s) => s.clone(),
        Js::Num(n) => number_to_string(*n),
        Js::Bool(b) => b.to_string(),
        Js::Null => "null".into(),
        Js::Undefined => "undefined".into(),
        Js::Arr(a) => a.iter().map(|x| if matches!(x, Js::Null | Js::Undefined) { String::new() } else { js_string(x) }).collect::<Vec<_>>().join(","),
        Js::Obj(_) => "[object Object]".into(),
    }
}

/// Renders a snapshot (`{rev, muted, flock}`) as unit-string text.
pub fn render(snap: &Js, palette: &[String], color: bool) -> String {
    let flock = snap.get("flock").as_arr().cloned().unwrap_or_default();
    let mut lines = vec![format!("bellwether  rev {}  {} sheep{}", js_string(snap.get("rev")), flock.len(), if snap.get("muted").truthy() { "  MUTED" } else { "" })];
    if flock.is_empty() {
        lines.push("  (no sessions yet — start or prompt a Claude Code session)".into());
    }
    for s in &flock {
        let ci = s.get("colourIndex");
        let colour = ci.as_num().filter(|n| *n >= 0.0 && n.fract() == 0.0).and_then(|n| palette.get(n as usize).cloned()).unwrap_or_else(|| format!("#{}", js_string(ci)));
        lines.push(row(&js_string(s.get("label")), &colour, s, color));
        for l in s.get("lambs").as_arr().cloned().unwrap_or_default() {
            lines.push(row(&format!("  └ {}", js_string(l.get("label"))), "lamb", &l, color));
        }
        let hidden = s.get("hiddenLambCount");
        if hidden.truthy() {
            let n = js_string(hidden);
            lines.push(format!("  └ +{n} more lamb{}", if hidden.as_num() == Some(1.0) { "" } else { "s" }));
        }
    }
    lines.join("\n") + "\n"
}

fn row(label: &str, colour: &str, a: &Js, color: bool) -> String {
    let state = if a.get("flavour").truthy() { format!("{}·{}", js_string(a.get("state")), js_string(a.get("flavour"))) } else { js_string(a.get("state")) };
    let mut alert = String::new();
    if a.get("alert").truthy() {
        alert = format!("ALERT  {}{}", js_string(a.get("alert").get("text")), if a.get("alert").get("confirmed").truthy() { "  (!)" } else { "" });
        if color {
            alert = format!("\x1b[1;33m{alert}\x1b[0m");
        }
    } else if a.get("lambAlert").truthy() {
        alert = "a lamb needs you".into();
    }
    let effect = if a.get("effect").truthy() { format!("  *{}*", js_string(a.get("effect").get("name"))) } else { String::new() };
    let line = format!("{} {} {} {alert}{effect}", fit(label, 16), fit(colour, 8), fit(&state, 16));
    line.trim_end_matches(jsstr::is_js_space).to_string()
}

/// Cut to n code points with an ellipsis, or padded with spaces to n code units (padEnd).
fn fit(s: &str, n: usize) -> String {
    let cps = jsstr::code_points(s);
    if cps.len() > n {
        let mut out = jsstr::from_code_points(&cps[..n - 1]);
        out.push('…');
        return out;
    }
    let pad = n.saturating_sub(jsstr::len(s));
    format!("{s}{}", " ".repeat(pad))
}
