//! Logs when the main thread stops answering, and what it was doing (W4: the strip froze
//! three times on the maintainer's PC mid-drag, "not responding", with nothing in the log).
//!
//! Every second a thread posts a beat to the main thread. If no beat has run for
//! `STUCK`, the log says so, with the step the main thread last marked (`doing`); it says
//! so again every `AGAIN` while it lasts, and once more when the main thread answers.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Runtime};

use crate::log::log;

const STUCK: Duration = Duration::from_secs(2);
const AGAIN: Duration = Duration::from_secs(10);

/// What the main thread is doing now (the latest mark), and since when.
static DOING: Mutex<Option<(u64, String, Instant)>> = Mutex::new(None);
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Marks a step on the main thread until the returned guard drops.
pub struct Doing(u64);

pub fn doing(what: impl Into<String>) -> Doing {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut d) = DOING.lock() {
        *d = Some((n, what.into(), Instant::now()));
    }
    Doing(n)
}

impl Drop for Doing {
    fn drop(&mut self) {
        if let Ok(mut d) = DOING.lock() {
            // Only its own mark: a later one (a step inside this one) has already replaced it.
            if d.as_ref().is_some_and(|(n, _, _)| *n == self.0) {
                *d = None;
            }
        }
    }
}

fn doing_now() -> String {
    match DOING.lock().ok().and_then(|d| d.clone()) {
        Some((_, what, since)) => format!("{what}, for {} ms", since.elapsed().as_millis()),
        None => "no marked step (inside tao, WebView2 or Windows)".into(),
    }
}

/// What to log, from how long the main thread has been silent and what was said.
#[derive(Debug, PartialEq)]
enum Say {
    Nothing,
    Stuck,
    StillStuck,
    Back,
}

fn verdict(silent: Duration, stuck_since_said: Option<Duration>) -> Say {
    match stuck_since_said {
        None if silent >= STUCK => Say::Stuck,
        None => Say::Nothing,
        Some(_) if silent < STUCK => Say::Back,
        Some(said) if said >= AGAIN => Say::StillStuck,
        Some(_) => Say::Nothing,
    }
}

pub fn start<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let beat = Arc::new(Mutex::new(Instant::now()));
        // When the main thread was last heard, as the stuck line said it; and when it said it.
        let mut said: Option<(Instant, Instant)> = None;
        loop {
            let slept = Instant::now();
            std::thread::sleep(Duration::from_secs(1));
            if slept.elapsed() >= STUCK && said.is_none() {
                // This thread overslept too: the computer slept, or the whole process was
                // paused. Not the main thread's doing; start counting afresh.
                *beat.lock().unwrap() = Instant::now();
            }
            let b = beat.clone();
            if app.run_on_main_thread(move || *b.lock().unwrap() = Instant::now()).is_err() {
                return; // the app is quitting
            }
            let last = *beat.lock().unwrap();
            let silent = last.elapsed();
            match verdict(silent, said.map(|(_, at)| at.elapsed())) {
                Say::Stuck => {
                    log(&format!("main thread: no answer for {:.1} s (the strip can't redraw or take clicks); last marked: {}", silent.as_secs_f64(), doing_now()));
                    said = Some((last, Instant::now()));
                }
                Say::StillStuck => {
                    log(&format!("main thread: still no answer, for {:.0} s; last marked: {}", silent.as_secs_f64(), doing_now()));
                    said = said.map(|(l, _)| (l, Instant::now()));
                }
                Say::Back => {
                    let (heard, _) = said.take().unwrap();
                    log(&format!("main thread: answering again after {:.1} s", last.duration_since(heard).as_secs_f64()));
                }
                Say::Nothing => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_said_once_then_every_ten_seconds_then_its_end() {
        let s = Duration::from_secs_f64;
        assert_eq!(verdict(s(1.2), None), Say::Nothing);
        assert_eq!(verdict(s(2.0), None), Say::Stuck);
        assert_eq!(verdict(s(5.0), Some(s(3.0))), Say::Nothing);
        assert_eq!(verdict(s(12.0), Some(s(10.0))), Say::StillStuck);
        assert_eq!(verdict(s(0.3), Some(s(4.0))), Say::Back);
    }

    #[test]
    fn a_step_inside_another_keeps_the_inner_mark_until_it_ends() {
        let outer = doing("outer");
        {
            let _inner = doing("inner");
            assert!(doing_now().starts_with("inner"));
        }
        // The inner one cleared its own mark only if it was still the latest.
        assert!(doing_now().starts_with("no marked step"));
        drop(outer);
        assert!(doing_now().starts_with("no marked step"));
    }
}
