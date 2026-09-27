//! Terminal key pump: a dedicated OS thread bridging crossterm's blocking
//! event API into the async event loop.
//!
//! Regression history: the first implementation used
//! `while poll(100ms).unwrap_or(false)`, which **exits on the first idle
//! timeout** — the thread died ~100ms after boot and every key (including
//! `q` and `Ctrl+C`) stopped working while playback kept going. The pump is
//! extracted here with a scripted-poll seam so that failure mode has a test.

use crossterm::event::{Event, KeyEvent};
use tokio::sync::mpsc::UnboundedSender;

/// Poll timeout per loop iteration (matches the UI tick cadence).
pub const POLL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// Core pump loop: poll → read → forward. An idle poll (`Ok(false)`) keeps
/// looping; only a channel close or a read failure ends the thread.
pub fn pump<P, R>(
    mut poll: P,
    mut read: R,
    timeout: std::time::Duration,
    tx: UnboundedSender<KeyEvent>,
) where
    P: FnMut(std::time::Duration) -> std::io::Result<bool>,
    R: FnMut() -> std::io::Result<Event>,
{
    loop {
        match poll(timeout) {
            Ok(true) => match read() {
                Ok(Event::Key(k)) => {
                    if tx.send(k).is_err() {
                        break; // UI loop is gone
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {} // idle: keep polling — this is the whole fix
            Err(_) => break,
        }
    }
}

/// Spawn the real reader thread for the TUI lifetime.
pub fn spawn_key_thread(tx: UnboundedSender<KeyEvent>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        pump(
            crossterm::event::poll,
            crossterm::event::read,
            POLL_TIMEOUT,
            tx,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::Duration;

    /// The pump must survive idle poll timeouts and still forward keys.
    /// The old `while poll(...)` structure would have stopped after poll #1
    /// and never delivered the key at all.
    #[test]
    fn pump_survives_idle_timeouts_and_forwards_keys() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<KeyEvent>();
        let handle = std::thread::spawn(move || {
            let mut polls = 0usize;
            pump(
                |_| {
                    polls += 1;
                    // idle, idle, then events available on every odd poll ≥ 3.
                    Ok(polls > 2 && polls % 2 == 1)
                },
                || {
                    Ok(Event::Key(KeyEvent::new(
                        KeyCode::Char('q'),
                        KeyModifiers::empty(),
                    )))
                },
                Duration::from_millis(1),
                tx,
            );
            polls
        });

        let got = rx
            .blocking_recv()
            .expect("key must arrive despite idle polls");
        assert_eq!(got.code, KeyCode::Char('q'));
        drop(rx); // next send fails → pump exits cleanly
        let final_polls = handle.join().expect("join");
        assert!(
            final_polls >= 3,
            "pump died on an idle timeout (only {final_polls} polls)"
        );
    }
}
