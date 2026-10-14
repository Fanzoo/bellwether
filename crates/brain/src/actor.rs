//! The state machine's task (distribution plan D1): one task owns the flock and is its
//! only writer. Hooks and `/control` reach it over a bounded channel; snapshots leave it on
//! a `watch` channel carrying `rev`, which is exactly what `/state?after=<rev>&wait=` waits
//! on. Its timers (the flock tick, the publish throttle, the state.json save) live here
//! too. A port of brain/brain.mjs.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bellwether_core::flock::{ControlResult, Flock, FlockConfig};
use bellwether_core::json::{self, Js, Obj};
use bellwether_core::normalise::{self, HookEvent};
use bellwether_core::persist;
use bellwether_core::publisher::{Published, Publisher};
use bellwether_core::redact::Cleaner;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{now_ms, Counters, Logger};

/// What reaches the state machine.
pub enum Msg {
    /// A hook body that parsed as a JSON object, with its headers as Node has them.
    Hook { payload: Js, headers: Obj, received_at: f64 },
    /// A `/control` message; the answer goes back on `reply`.
    Control { msg: Js, reply: oneshot::Sender<ControlResult> },
}

/// How many events may wait for the state machine before `/hook` counts them `dropped`.
pub const QUEUE: usize = 4096;

/// The timings brain.mjs takes as options (tests shorten them).
#[derive(Clone, Copy, Debug)]
pub struct Timings {
    pub tick: Duration,
    pub persist: Duration,
    pub publish_ms: f64,
}

impl Default for Timings {
    fn default() -> Timings {
        Timings { tick: Duration::from_millis(1000), persist: Duration::from_millis(2000), publish_ms: 250.0 }
    }
}

pub struct Setup {
    pub merged: Js,
    pub cleaner: Cleaner,
    pub state_file: PathBuf,
    pub record_to: Option<std::fs::File>,
    pub timings: Timings,
    pub log: Logger,
    pub counters: Arc<Counters>,
}

/// Starts the state machine. Returns its inbox and the snapshot channel.
pub fn start(setup: Setup, stop: oneshot::Receiver<()>) -> (mpsc::Sender<Msg>, watch::Receiver<Arc<Published>>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(QUEUE);
    let (persisted, warnings) = persist::load_state(&setup.state_file);
    for w in warnings {
        setup.log.log(&format!("warning: {w}"));
    }
    let flock = Flock::new(FlockConfig::from_merged(&setup.merged), &persisted, now_ms() as f64);
    let publisher = Publisher::with_limits(&flock.snapshot(), now_ms() as f64, setup.timings.publish_ms, 4, 1000.0);
    let (snap_tx, snap_rx) = watch::channel(Arc::new(publisher.current().clone()));
    let handle = tokio::task::spawn(run(setup, flock, publisher, rx, snap_tx, stop));
    (tx, snap_rx, handle)
}

struct State {
    setup: Setup,
    flock: Flock,
    publisher: Publisher,
    snap_tx: watch::Sender<Arc<Published>>,
}

impl State {
    fn mark_dirty(&mut self, urgent: bool) {
        let flock = &self.flock;
        self.publisher.mark_dirty(urgent, now_ms() as f64, || flock.snapshot());
        self.send();
    }

    fn send(&mut self) {
        if self.snap_tx.borrow().rev != self.publisher.current().rev {
            self.snap_tx.send_replace(Arc::new(self.publisher.current().clone()));
        }
    }

    fn save(&mut self) {
        if !self.flock.persist_dirty {
            return;
        }
        self.flock.persist_dirty = false;
        if let Err(e) = persist::save_state(&self.setup.state_file, &self.flock.persistable()) {
            self.setup.log.log(&format!("warning: could not save {} ({})", self.setup.state_file.display(), bellwether_core::config::io_code(&e)));
        }
    }

    fn hook(&mut self, payload: Js, headers: Obj, received_at: f64) {
        if let Some(f) = self.setup.record_to.as_mut() {
            let line = Obj::new()
                .with("normalised", normalise::normalise(&payload, &headers, received_at, None).to_js())
                .with("rawKeys", normalise::raw_keys(&payload))
                .with("headersPresent", normalise::headers_present(&headers));
            let _ = f.write_all((json::stringify(&Js::Obj(line)) + "\n").as_bytes());
        }
        let ev = normalise::normalise(&payload, &headers, received_at, Some(&self.setup.cleaner));
        self.setup.log.log(&describe(&ev));
        let before = self.flock.alert_marks();
        if self.flock.apply(&ev) {
            let raised = self.flock.alert_marks().iter().any(|m| !before.contains(m));
            self.mark_dirty(raised);
        }
    }
}

async fn run(setup: Setup, flock: Flock, publisher: Publisher, mut rx: mpsc::Receiver<Msg>, snap_tx: watch::Sender<Arc<Published>>, mut stop: oneshot::Receiver<()>) {
    let mut ticker = tokio::time::interval(setup.timings.tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    let mut saver = tokio::time::interval(setup.timings.persist);
    saver.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    saver.tick().await;
    let mut st = State { setup, flock, publisher, snap_tx };
    loop {
        let due = st.publisher.timer_at().map(|at| Duration::from_millis((at - now_ms() as f64).max(0.0).min(3.6e6) as u64));
        let publish_timer = async {
            match due {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else { break };
                let counters = st.setup.counters.clone();
                // A panic in the state machine counts as a consumer error and is survived,
                // as a throwing consumer is in Node.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match msg {
                    Msg::Hook { payload, headers, received_at } => {
                        st.hook(payload, headers, received_at);
                        None
                    }
                    Msg::Control { msg, reply } => {
                        let r = st.flock.control(&msg, now_ms() as f64);
                        // Someone asked (the tray, `bellwether mute`): show it now.
                        if r.ok {
                            st.mark_dirty(true);
                        }
                        Some((reply, r))
                    }
                }));
                match outcome {
                    Ok(Some((reply, r))) => {
                        let _ = reply.send(r);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        counters.consumer_errors.fetch_add(1, Ordering::Relaxed);
                        st.setup.log.log("event consumer threw: a panic in the state machine");
                    }
                }
            }
            _ = ticker.tick() => {
                st.flock.tick(now_ms() as f64);
                st.mark_dirty(false);
            }
            _ = saver.tick() => st.save(),
            _ = publish_timer => {
                let flock = &st.flock;
                st.publisher.on_timer(now_ms() as f64, || flock.snapshot());
                st.send();
            }
            _ = &mut stop => break,
        }
    }
    st.save();
}

/// One content-free log line per event: event, short session id, agent, tool.
pub fn describe(ev: &HookEvent) -> String {
    let sid = ev.session_id().map(|s| bellwether_core::jsstr::prefix(s, 8).to_string()).unwrap_or_else(|| "--------".into());
    let who = match ev.agent_id() {
        Some(a) => format!("{}#{}", ev.agent_type().unwrap_or("agent"), bellwether_core::jsstr::prefix(a, 6)),
        None => "main".into(),
    };
    let what: Vec<&str> = [ev.tool_name(), ev.notification_type(), ev.scalar("source"), ev.reason(), ev.scalar("trigger"), ev.error_type.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
    bellwether_core::jsstr::to_string_lossy(&format!("{:<18} s:{sid}  {who:<16} {}", ev.event, what.join(" ")))
}
