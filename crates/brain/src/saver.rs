//! state.json's writer (M13, deviation 130's follow-up): the state machine hands each
//! snapshot to a thread of its own, so a slow or stalled disk never holds up the runtime
//! that answers hooks. Only the latest snapshot matters: one still waiting is replaced by a
//! newer one, so a stalled disk costs one write when it comes back, not a queue of them.
//! [`Saver::finish`] writes whatever is waiting, and waits for it (up to a limit) before
//! the brain exits.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use bellwether_core::json::Js;

use crate::Logger;

/// Writes one snapshot to the file. The brain's is [`default_writer`]; tests put in a
/// slow or blocked one.
pub type Writer = Arc<dyn Fn(&Path, &Js) -> std::io::Result<()> + Send + Sync>;

/// How long a stopping brain waits for the last write before it exits anyway (a stalled
/// disk must not keep a quitting app's brain alive).
pub const FLUSH_LIMIT: Duration = Duration::from_secs(5);

/// `persist::save_state`: 0600, a temporary file flushed to disk, then a rename.
pub fn default_writer() -> Writer {
    Arc::new(|file: &Path, value: &Js| bellwether_core::persist::save_state(file, value))
}

#[derive(Default)]
struct Slot {
    next: Option<Js>,
    closing: bool,
}

pub struct Saver {
    shared: Arc<(Mutex<Slot>, Condvar)>,
    /// Signalled when the thread has written everything and stopped.
    done: std::sync::mpsc::Receiver<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Used only when the thread couldn't be started: then saves are written in place.
    inline: Option<(PathBuf, Writer, Logger)>,
}

fn write_one(file: &Path, writer: &Writer, log: &Logger, value: &Js) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| writer(file, value)));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log.log(&format!("warning: could not save {} ({})", file.display(), bellwether_core::config::io_code(&e))),
        Err(_) => log.log(&format!("warning: could not save {} (the writer panicked)", file.display())),
    }
}

impl Saver {
    pub fn start(file: PathBuf, writer: Writer, log: Logger) -> Saver {
        let shared = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let (done_tx, done) = std::sync::mpsc::channel();
        let (s, f, w, l) = (shared.clone(), file.clone(), writer.clone(), log.clone());
        let spawned = std::thread::Builder::new().name("bellwether-state.json".into()).spawn(move || {
            loop {
                let next = {
                    let (m, cv) = &*s;
                    let mut slot = m.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(v) = slot.next.take() {
                            break Some(v);
                        }
                        if slot.closing {
                            break None;
                        }
                        slot = cv.wait(slot).unwrap_or_else(|e| e.into_inner());
                    }
                };
                // The lock is let go before the write: a save never waits for the disk.
                let Some(v) = next else { break };
                write_one(&f, &w, &l, &v);
            }
            let _ = done_tx.send(());
        });
        match spawned {
            Ok(t) => Saver { shared, done, thread: Some(t), inline: None },
            Err(e) => {
                log.log(&format!("warning: cannot start the state.json writer ({e}); saving on the serving thread"));
                Saver { shared, done, thread: None, inline: Some((file, writer, log)) }
            }
        }
    }

    /// Hands the snapshot over and returns at once; a snapshot not yet written is replaced.
    pub fn save(&self, value: Js) {
        if let Some((f, w, l)) = &self.inline {
            write_one(f, w, l, &value);
            return;
        }
        let (m, cv) = &*self.shared;
        m.lock().unwrap_or_else(|e| e.into_inner()).next = Some(value);
        cv.notify_one();
    }

    /// Writes what is waiting and stops the thread. False when the write was still going
    /// after `limit` (the thread is left to finish on its own).
    pub fn finish(mut self, limit: Duration) -> bool {
        self.close();
        let Some(t) = self.thread.take() else { return true };
        match self.done.recv_timeout(limit) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
            _ => {
                let _ = t.join();
                true
            }
        }
    }

    fn close(&self) {
        let (m, cv) = &*self.shared;
        m.lock().unwrap_or_else(|e| e.into_inner()).closing = true;
        cv.notify_one();
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn value(n: f64) -> Js {
        bellwether_core::json::parse(&format!(r#"{{"version":1,"muted":false,"identities":{{}},"n":{n}}}"#)).unwrap()
    }

    /// A writer that waits for the test's go before each write, and records what it wrote.
    /// The writer, its go, what it wrote, and how many writes it started.
    type Gated = (Writer, std::sync::mpsc::SyncSender<()>, Arc<Mutex<Vec<Js>>>, Arc<AtomicUsize>);

    fn gated() -> Gated {
        let (go_tx, go_rx) = std::sync::mpsc::sync_channel::<()>(100);
        let go_rx = Mutex::new(go_rx);
        let written = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(AtomicUsize::new(0));
        let (w, s) = (written.clone(), started.clone());
        let writer: Writer = Arc::new(move |_f: &Path, v: &Js| {
            s.fetch_add(1, Ordering::SeqCst);
            let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(10));
            w.lock().unwrap().push(v.clone());
            Ok(())
        });
        (writer, go_tx, written, started)
    }

    #[test]
    fn a_save_never_waits_for_the_disk_and_only_the_latest_waiting_one_is_written() {
        let (writer, go, written, started) = gated();
        let saver = Saver::start(std::env::temp_dir().join("bw-saver-unused.json"), writer, Logger::stderr_only());
        saver.save(value(1.0));
        let t0 = std::time::Instant::now();
        while started.load(Ordering::SeqCst) == 0 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // The first write is now blocked on the disk; more saves return at once.
        let t1 = std::time::Instant::now();
        for n in 2..=50 {
            saver.save(value(n as f64));
        }
        assert!(t1.elapsed() < Duration::from_millis(50), "{:?}", t1.elapsed());
        for _ in 0..10 {
            let _ = go.try_send(());
        }
        assert!(saver.finish(Duration::from_secs(5)));
        let w = written.lock().unwrap();
        assert_eq!(w.len(), 2, "the blocked write, then only the latest of the 49 that waited");
        assert_eq!(w[0], value(1.0));
        assert_eq!(w[1], value(50.0));
    }

    #[test]
    fn finish_writes_what_is_waiting_and_gives_up_on_a_stalled_disk() {
        let file = std::env::temp_dir().join(format!("bw-saver-{}", std::process::id())).join("state.json");
        let saver = Saver::start(file.clone(), default_writer(), Logger::stderr_only());
        saver.save(value(7.0));
        assert!(saver.finish(FLUSH_LIMIT));
        assert_eq!(bellwether_core::persist::load_state(&file).0, value(7.0));
        let _ = std::fs::remove_dir_all(file.parent().unwrap());

        let (writer, _go, written, _) = gated();
        let saver = Saver::start(std::env::temp_dir().join("bw-saver-unused.json"), writer, Logger::stderr_only());
        saver.save(value(1.0));
        let t0 = std::time::Instant::now();
        assert!(!saver.finish(Duration::from_millis(200)), "the disk never answered");
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        assert!(written.lock().unwrap().is_empty());
    }
}
