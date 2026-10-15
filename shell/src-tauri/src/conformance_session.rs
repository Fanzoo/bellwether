//! `__session --in <fifo> --out <fifo>`: the test bridge (test/impl.mjs, distribution plan
//! D2). The Node tests that check the brain's pure parts in process (the flock, the
//! cleaner, the normaliser, the config, the publisher) call these instead of brain/*.mjs
//! under `BELLWETHER_IMPL=rust`, one call at a time, and get the Rust answers.
//!
//! A call is one JSON line, `{"fn": "...", "args": [...]}`; the answer is one line,
//! `{"ok": value}` or `{"error": {"message", "code"}}`. Objects and handles (a Flock, a
//! publisher) live in this process between calls. JS's `undefined` travels as
//! `{"$undefined": true}`, so a property that is there but undefined stays there.
//! Milestone 4 adds the installer (`hooks.*`), the diff (`diff.*`) and doctor's pure checks
//! (`doctor.*`); settings cross as JS values and are read into the lossless JSON in their
//! iteration order. Only in builds with the `conformance` feature.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use bellwether_brain::exit;
use bellwether_core::config::{self, ConfigError};
use bellwether_core::flock::{self, Flock, FlockConfig};
use bellwether_core::json::{self, Js, Obj};
use bellwether_core::normalise::{self, HookEvent};
use bellwether_core::publisher::Publisher;
use bellwether_core::redact::Cleaner;
use bellwether_core::{envfile, mask, outfile, persist, replay, shellstate, status};

fn now() -> f64 {
    bellwether_brain::now_ms() as f64
}

/// A publisher with a real clock: a thread publishes when the throttle's timer is due.
struct LivePublisher {
    state: Mutex<(Publisher, Js, bool)>,
    wake: Condvar,
}

impl LivePublisher {
    fn start(first: Js, interval_ms: f64) -> Arc<LivePublisher> {
        let p = Arc::new(LivePublisher { state: Mutex::new((Publisher::with_limits(&first, now(), interval_ms, 4, 1000.0), first, false)), wake: Condvar::new() });
        let t = p.clone();
        std::thread::spawn(move || {
            let mut g = t.state.lock().unwrap();
            loop {
                if g.2 {
                    return;
                }
                match g.0.timer_at() {
                    Some(at) if at <= now() => {
                        let content = g.1.clone();
                        g.0.on_timer(now(), || content);
                    }
                    Some(at) => {
                        let wait = Duration::from_millis(((at - now()).max(1.0)) as u64);
                        g = t.wake.wait_timeout(g, wait).unwrap().0;
                    }
                    None => g = t.wake.wait(g).unwrap(),
                }
            }
        });
        p
    }
}

#[derive(Default)]
struct Session {
    cleaners: HashMap<String, Arc<Cleaner>>,
    flocks: HashMap<u64, Flock>,
    publishers: HashMap<u64, Arc<LivePublisher>>,
    next: u64,
    seen: Vec<String>,
}

fn err(message: impl Into<String>, code: i32) -> Js {
    Js::Obj(Obj::new().with("error", Obj::new().with("message", message.into()).with("code", code as f64)))
}

fn config_err(e: ConfigError) -> Js {
    err(e.message, e.code)
}

/// `undefined` as `{"$undefined": true}`, everywhere in a value.
fn mark_undefined(v: Js) -> Js {
    match v {
        Js::Undefined => Js::Obj(Obj::new().with("$undefined", true)),
        Js::Arr(a) => Js::Arr(a.into_iter().map(mark_undefined).collect()),
        Js::Obj(o) => {
            let mut out = Obj::new();
            for (k, x) in o.iter() {
                out.set(k.clone(), mark_undefined(x.clone()));
            }
            Js::Obj(out)
        }
        other => other,
    }
}

impl Session {
    fn cleaner(&mut self, opts: &Js) -> Result<Arc<Cleaner>, Js> {
        let key = json::stringify(opts);
        if let Some(c) = self.cleaners.get(&key) {
            return Ok(c.clone());
        }
        let max = opts.get("maxChars").as_num().unwrap_or(60.0) as usize;
        let mut patterns = Vec::new();
        if let Js::Arr(a) = opts.get("redactPatterns") {
            for p in a {
                match p.as_str() {
                    Some(s) if !s.is_empty() => patterns.push(s.to_string()),
                    _ => return Err(err("bubble.redactPatterns: every entry must be a non-empty string", exit::BAD_CONFIG)),
                }
            }
        }
        let c = Arc::new(Cleaner::new(max, &patterns).map_err(|e| err(e.to_string(), exit::BAD_CONFIG))?);
        self.cleaners.insert(key, c.clone());
        Ok(c)
    }

    fn handle(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn call(&mut self, f: &str, a: &[Js]) -> Result<Js, Js> {
        let arg = |i: usize| a.get(i).cloned().unwrap_or(Js::Undefined);
        let num = |i: usize| a.get(i).and_then(Js::as_num).unwrap_or(f64::NAN);
        let handle = || a.first().and_then(Js::as_num).map(|n| n as u64).unwrap_or(0);
        Ok(match f {
            "clean" => {
                let c = self.cleaner(&arg(0))?;
                match arg(1) {
                    Js::Str(t) => Js::Str(c.clean(&t)),
                    _ => Js::Undefined,
                }
            }
            "cleaner" => {
                self.cleaner(&arg(0))?;
                Js::Bool(true)
            }
            "normalise" => {
                let clean = match arg(3) {
                    Js::Obj(_) => Some(self.cleaner(&arg(3))?),
                    _ => None,
                };
                let headers = arg(1).as_obj().cloned().unwrap_or_default();
                normalise::normalise(&arg(0), &headers, num(2), clean.as_deref()).to_js()
            }
            "fingerprint" => Js::Str(normalise::fingerprint(arg(0).as_str().unwrap_or(""), &arg(1))),
            "rawKeys" => normalise::raw_keys(&arg(0)),
            "headersPresent" => normalise::headers_present(&arg(0).as_obj().cloned().unwrap_or_default()),
            "identityOf" => {
                let ev = arg(0);
                let (key, terminal_based) = flock::identity_of(&ev.get("terminal").as_obj().cloned().unwrap_or_default(), ev.get("sessionId").as_str());
                Js::Obj(Obj::new().with("key", key).with("terminalBased", terminal_based))
            }
            "limits" => flock::limits_js(),
            "palettes" => Js::Obj(
                Obj::new()
                    .with("DEFAULT_PALETTE", Js::Arr(config::DEFAULT_PALETTE.iter().map(|c| Js::str(c)).collect()))
                    .with("OLD_DEFAULT_PALETTE", Js::Arr(config::OLD_DEFAULT_PALETTE.iter().map(|c| Js::str(c)).collect())),
            ),
            "withDefaults" => {
                let m = config::with_defaults(&arg(0)).map_err(config_err)?;
                Js::Obj(Obj::new().with("config", m.config).with("paletteUpgraded", m.palette_upgraded))
            }
            "validateConfig" => {
                config::validate_config(&arg(0), arg(1).as_str().unwrap_or("config")).map_err(config_err)?;
                Js::Bool(true)
            }
            "flock.new" => {
                let merged = arg(0);
                let t = if a.get(2).is_some_and(|x| !x.is_undefined()) { num(2) } else { 0.0 };
                let f = Flock::new(FlockConfig::from_merged(&merged), &arg(1), t);
                let h = self.handle();
                self.flocks.insert(h, f);
                Js::Num(h as f64)
            }
            f if f.starts_with("flock.") => {
                let fl = self.flocks.get_mut(&handle()).ok_or_else(|| err("no such flock", 1))?;
                match f {
                    "flock.apply" => Js::Bool(fl.apply(&HookEvent::from_js(&arg(1)))),
                    "flock.tick" => {
                        fl.tick(num(1));
                        Js::Undefined
                    }
                    "flock.control" => fl.control(&arg(1), num(2)).to_js(),
                    "flock.snapshot" => fl.snapshot(),
                    "flock.persistable" => mark_undefined(fl.persistable()),
                    "flock.alertMarks" => Js::Arr(fl.alert_marks().into_iter().map(Js::Str).collect()),
                    "flock.muted" => Js::Bool(fl.muted),
                    "flock.persistDirty" => Js::Bool(fl.persist_dirty),
                    "flock.identityKeys" => Js::Arr(fl.identity_keys().into_iter().map(Js::Str).collect()),
                    _ => return Err(err(format!("unknown call {f}"), 1)),
                }
            }
            "fixture" => {
                let merged = match arg(1) {
                    Js::Obj(_) => arg(1),
                    _ => config::Config::defaults().merged,
                };
                let run = bellwether_core::fixture::run_fixture(arg(0).as_arr().map(Vec::as_slice).unwrap_or(&[]), &merged, 250.0, 3000.0, true);
                Js::Obj(Obj::new().with("transitions", Js::Arr(run.transitions)).with("snapshots", Js::Arr(run.snapshots)))
            }
            "loadState" => {
                let (v, warnings) = persist::load_state(std::path::Path::new(&arg(0).as_str().map(bellwether_core::jsstr::to_string_lossy).unwrap_or_default()));
                Js::Obj(Obj::new().with("value", v).with("warnings", Js::Arr(warnings.into_iter().map(|w| Js::Str(bellwether_core::jsstr::from_str(&w))).collect())))
            }
            "recorder.write" => {
                let file = bellwether_core::jsstr::to_string_lossy(arg(0).as_str().unwrap_or(""));
                let headers = arg(2).as_obj().cloned().unwrap_or_default();
                let line = Obj::new()
                    .with("normalised", normalise::normalise(&arg(1), &headers, num(3), None).to_js())
                    .with("rawKeys", normalise::raw_keys(&arg(1)))
                    .with("headersPresent", normalise::headers_present(&headers));
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&file).map_err(|e| err(e.to_string(), 1))?;
                f.write_all((json::stringify(&Js::Obj(line)) + "\n").as_bytes()).map_err(|e| err(e.to_string(), 1))?;
                Js::Undefined
            }
            "rebuild" => {
                let (payload, headers) = replay::rebuild(&arg(0), arg(1).as_str());
                Js::Obj(Obj::new().with("payload", payload).with("headers", Js::Obj(headers)))
            }
            "renderStatus" => {
                let palette: Vec<String> = arg(1).as_arr().map(|p| p.iter().map(status::js_string).collect()).unwrap_or_default();
                Js::Str(status::render(&arg(0), &palette, arg(2).truthy()))
            }
            "guardPrint" => {
                outfile::guard_print(std::path::Path::new(&bellwether_core::jsstr::to_string_lossy(arg(0).as_str().unwrap_or(""))), arg(1).truthy()).map_err(config_err)?;
                Js::Undefined
            }
            "guardRecord" => {
                outfile::guard_record(std::path::Path::new(&bellwether_core::jsstr::to_string_lossy(arg(0).as_str().unwrap_or("")))).map_err(config_err)?;
                Js::Undefined
            }
            "isBellwetherRecording" => Js::Bool(outfile::is_bellwether_recording(arg(0).as_str().unwrap_or(""))),
            "maskToken" => {
                // A `seen` set of the caller's, or this session's own.
                let mut seen: Vec<String> = match arg(2) {
                    Js::Arr(s) => s.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                    _ => self.seen.clone(),
                };
                let text = mask::mask_with(arg(0).as_str().unwrap_or(""), arg(1).as_str(), &mut seen);
                if !matches!(arg(2), Js::Arr(_)) {
                    self.seen = seen.clone();
                }
                Js::Obj(Obj::new().with("text", text).with("seen", Js::Arr(seen.into_iter().map(Js::Str).collect())))
            }
            "sharingProcessWarnings" => {
                let config: serde_json::Value = serde_json::from_str(&json::stringify(&arg(0))).unwrap_or(serde_json::Value::Null);
                Js::Arr(crate::privacy::sharing_warnings_for(&config, arg(1).as_str().unwrap_or("darwin")).into_iter().map(|w| Js::Str(bellwether_core::jsstr::from_str(&w))).collect())
            }
            "envValue" => envfile::env_value(arg(0).as_str().unwrap_or(""), arg(1).as_str().unwrap_or("")).map(Js::Str).unwrap_or(Js::Null),
            "shellState.read" => {
                let (hidden, show_labels) = shellstate::read(std::path::Path::new(&bellwether_core::jsstr::to_string_lossy(arg(0).as_str().unwrap_or(""))));
                Js::Obj(Obj::new().with("hidden", hidden).with("showLabels", show_labels))
            }
            "publisher.new" => {
                let interval = a.get(1).and_then(Js::as_num).unwrap_or(250.0);
                let p = LivePublisher::start(arg(0), interval);
                let h = self.handle();
                self.publishers.insert(h, p);
                Js::Num(h as f64)
            }
            f if f.starts_with("publisher.") => {
                let p = self.publishers.get(&handle()).cloned().ok_or_else(|| err("no such publisher", 1))?;
                let mut g = p.state.lock().unwrap();
                let v = match f {
                    "publisher.markDirty" => {
                        g.1 = arg(2);
                        let content = g.1.clone();
                        g.0.mark_dirty(arg(1).truthy(), now(), || content);
                        Js::Undefined
                    }
                    "publisher.current" => g.0.current().value.clone(),
                    "publisher.published" => Js::Num(g.0.published() as f64),
                    "publisher.close" => {
                        g.2 = true;
                        Js::Undefined
                    }
                    _ => return Err(err(format!("unknown call {f}"), 1)),
                };
                drop(g);
                p.wake.notify_all();
                v
            }
            f if f.starts_with("hooks.") || f.starts_with("diff.") || f.starts_with("doctor.") => installer_call(f, a)?,
            _ => return Err(err(format!("unknown call {f}"), 1)),
        })
    }
}

fn path_arg(v: &Js) -> std::path::PathBuf {
    std::path::PathBuf::from(bellwether_core::jsstr::to_string_lossy(v.as_str().unwrap_or("")))
}

fn port_arg(v: &Js) -> u16 {
    v.as_num().filter(|n| *n >= 0.0 && *n <= 65535.0).unwrap_or(0.0) as u16
}

fn check_js(c: &bellwether_core::settings_checks::Check) -> Js {
    use bellwether_core::jsstr::from_str;
    let mut o = Obj::new().with("name", from_str(&c.name)).with("status", c.status).with("detail", from_str(&c.detail)).with("exitCode", c.exit_code as f64);
    if let Some(n) = c.found {
        o.set("found", n as f64);
    }
    Js::Obj(o)
}

fn counts_js(c: &[(String, usize)]) -> Js {
    let mut o = Obj::new();
    for (e, n) in c {
        o.set(e.clone(), *n as f64);
    }
    Js::Obj(o)
}

/// The installer, the diff and doctor's checks of brain/hooks.mjs, diff.mjs and doctor.mjs.
fn installer_call(f: &str, a: &[Js]) -> Result<Js, Js> {
    use bellwether_core::hooks::{self as installer, Action, Created};
    use bellwether_core::lossless::{deep_equal, Lj};
    use bellwether_core::{diff, settings_checks};
    let arg = |i: usize| a.get(i).cloned().unwrap_or(Js::Undefined);
    let lj = |i: usize| Lj::from_js(&arg(i));
    let real = |v: &Js| bellwether_core::jsstr::to_string_lossy(v.as_str().unwrap_or(""));
    Ok(match f {
        "hooks.EVENTS" => Js::Arr(installer::EVENTS.iter().map(|e| Js::str(e)).collect()),
        "hooks.HTTP_UNSUPPORTED_EVENTS" => Js::Arr(installer::HTTP_UNSUPPORTED_EVENTS.iter().map(|e| Js::str(e)).collect()),
        "hooks.hookBlock" => installer::hook_block(port_arg(&arg(0)), &real(&arg(1))).to_js(),
        "hooks.install" => installer::install(&lj(0), port_arg(&arg(1)), &real(&arg(2))).map_err(config_err)?.to_js(),
        "hooks.uninstall" => {
            let created = match arg(2) {
                Js::Obj(o) => Some(Created { events: o.get("events").and_then(Js::as_arr).map(|e| e.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(), hooks: o.get("hooks").map(Js::truthy).unwrap_or(false) }),
                _ => None,
            };
            installer::uninstall(&lj(0), port_arg(&arg(1)), created.as_ref()).map_err(config_err)?.to_js()
        }
        "hooks.countGroups" => counts_js(&installer::count_groups(&lj(0), port_arg(&arg(1)))),
        "hooks.deepEqual" => Js::Bool(deep_equal(&lj(0), &lj(1))),
        "hooks.isBellwetherHandler" => Js::Bool(installer::is_bellwether_handler(&lj(0), port_arg(&arg(1)))),
        "hooks.isLegacyHandler" => Js::Bool(installer::is_legacy_handler(&lj(0), port_arg(&arg(1)))),
        "hooks.shapeProblems" => Js::Arr(installer::shape_problems(&lj(0)).into_iter().map(|p| Js::Str(bellwether_core::jsstr::from_str(&p))).collect()),
        "hooks.planEdit" => {
            let action = if arg(1).as_str() == Some("uninstall") { Action::Uninstall } else { Action::Install };
            let o = arg(2);
            let data = match o.get("dataDir") {
                Js::Str(_) => Some(path_arg(o.get("dataDir"))),
                _ => None,
            };
            let token = o.get("token").as_str().map(|t| bellwether_core::jsstr::to_string_lossy(t)).unwrap_or_default();
            let p = installer::plan_edit(&path_arg(&arg(0)), action, port_arg(o.get("port")), &token, data.as_deref()).map_err(config_err)?;
            let record = p.record.as_ref().map(|r| Js::Obj(Obj::new().with("events", Js::Arr(r.events.iter().map(|e| Js::Str(e.clone())).collect())).with("hooks", r.hooks))).unwrap_or(Js::Null);
            Js::Obj(
                Obj::new()
                    .with("file", bellwether_core::jsstr::from_str(&p.file.display().to_string()))
                    .with("action", action.word())
                    .with("exists", p.exists)
                    .with("originalHash", p.original_hash.clone().map(Js::Str).unwrap_or(Js::Null))
                    .with("before", p.before.to_js())
                    .with("after", p.after.to_js())
                    .with("changed", p.changed)
                    .with("oldText", bellwether_core::jsstr::from_str(&p.old_text))
                    .with("newText", bellwether_core::jsstr::from_str(&p.new_text))
                    .with("layoutKept", p.layout_kept)
                    .with("record", record)
                    .with("warnings", Js::Arr(p.warnings.iter().map(|w| Js::Str(bellwether_core::jsstr::from_str(w))).collect())),
            )
        }
        "diff.unifiedDiff" => {
            let o = arg(2);
            let name = |k: &str, d: &str| o.get(k).as_str().map(bellwether_core::jsstr::to_string_lossy).unwrap_or_else(|| d.to_string());
            let context = o.get("context").as_num().map(|n| n.max(0.0) as usize).unwrap_or(3);
            Js::Str(bellwether_core::jsstr::from_str(&diff::unified_diff(&real(&arg(0)), &real(&arg(1)), &name("oldName", "a"), &name("newName", "b"), context)))
        }
        "diff.diffStats" => {
            let st = diff::diff_stats(&real(&arg(0)), &real(&arg(1)));
            Js::Obj(Obj::new().with("added", st.added as f64).with("removed", st.removed as f64).with("removedLines", Js::Arr(st.removed_lines.into_iter().map(|l| Js::Str(bellwether_core::jsstr::from_str(&l))).collect())))
        }
        "doctor.checkSettings" => {
            let o = arg(1);
            let token = o.get("token").as_str().map(|t| bellwether_core::jsstr::to_string_lossy(t));
            let file = o.get("file").as_str().map(|t| bellwether_core::jsstr::to_string_lossy(t)).unwrap_or_else(|| "settings".into());
            Js::Arr(settings_checks::check_settings(&lj(0), port_arg(o.get("port")), token.as_deref(), &file).iter().map(check_js).collect())
        }
        "doctor.checkOtherLevels" => {
            let o = arg(0);
            let dir = |k: &str| match o.get(k) {
                Js::Str(_) => Some(path_arg(o.get(k))),
                _ => None,
            };
            let user = match o.get("userSettings") {
                Js::Obj(_) => Some(Lj::from_js(o.get("userSettings"))),
                _ => None,
            };
            Js::Arr(settings_checks::check_other_levels(dir("projectDir").as_deref(), dir("managedFile").as_deref(), port_arg(o.get("port")), user.as_ref()).iter().map(check_js).collect())
        }
        "doctor.globMatch" => Js::Bool(settings_checks::glob_match(&real(&arg(0)), &real(&arg(1)))),
        "doctor.checkLocatorLog" => {
            let log = std::fs::read(path_arg(&arg(0)).join("shell.log")).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
            Js::Arr(bellwether_brain::doctor::check_locator_log(log.as_deref()).iter().map(check_js).collect())
        }
        "doctor.nextStep" => {
            let checks: Vec<settings_checks::Check> = arg(0)
                .as_arr()
                .map(|cs| cs.iter().map(|c| {
                    let status = match c.get("status").as_str() { Some("PASS") => "PASS", Some("FAIL") => "FAIL", Some("WARN") => "WARN", _ => "INFO" };
                    settings_checks::Check::new(real(c.get("name")), status, real(c.get("detail")), c.get("exitCode").as_num().unwrap_or(0.0) as i32)
                }).collect())
                .unwrap_or_default();
            Js::Str(bellwether_core::jsstr::from_str(&bellwether_brain::doctor::next_step(&checks, arg(1).truthy())))
        }
        _ => return Err(err(format!("unknown call {f}"), 1)),
    })
}

/// `__session --in <path> --out <path>` (FIFOs the caller made), or stdin and stdout.
pub fn run(args: &[String]) -> i32 {
    let (mut input, mut output): (Box<dyn BufRead>, Box<dyn Write>) = (Box::new(BufReader::new(std::io::stdin())), Box::new(std::io::stdout()));
    let mut i = 0;
    while i < args.len() {
        match (args[i].as_str(), args.get(i + 1)) {
            ("--in", Some(p)) => match std::fs::File::open(p) {
                Ok(f) => input = Box::new(BufReader::new(f)),
                Err(e) => {
                    eprintln!("bellwether: __session: {p}: {e}");
                    return exit::UNEXPECTED;
                }
            },
            ("--out", Some(p)) => match std::fs::OpenOptions::new().write(true).open(p) {
                Ok(f) => output = Box::new(f),
                Err(e) => {
                    eprintln!("bellwether: __session: {p}: {e}");
                    return exit::UNEXPECTED;
                }
            },
            _ => {
                eprintln!("bellwether: __session takes --in <path> --out <path>");
                return exit::BAD_CONFIG;
            }
        }
        i += 2;
    }
    let mut s = Session::default();
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => return exit::OK,
            Ok(_) => {}
        }
        let reply = match json::parse(line.trim_end_matches(['\n', '\r'])) {
            Ok(call) => {
                let f = call.get("fn").as_str().unwrap_or("").to_string();
                let args = call.get("args").as_arr().cloned().unwrap_or_default();
                match s.call(&f, &args) {
                    Ok(v) => Js::Obj(Obj::new().with("ok", mark_undefined(v))),
                    Err(e) => e,
                }
            }
            Err(e) => err(format!("not a JSON call: {e}"), 1),
        };
        if output.write_all((json::stringify(&reply) + "\n").as_bytes()).and_then(|_| output.flush()).is_err() {
            return exit::OK;
        }
    }
}
