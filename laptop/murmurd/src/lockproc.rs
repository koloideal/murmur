//! Process-locker adapter: "locked" == a known screen locker is alive.
//!
//! Screen lockers on wlroots (hyprlock, swaylock, gtklock, …) lock via
//! ext-session-lock-v1, which observers can't read — and on greeter-class
//! logind sessions `LockedHint` never moves. The practical signal is the
//! locker process itself: scan `/proc/*/comm` once a second and emit a
//! transition when the set gains/loses its last member.
//!
//! The name list is configurable (`lock_procs`) — it's the one part of
//! presence that's machine-dependent.

use std::time::Duration;
use tokio::sync::mpsc;

pub use crate::logind::Lock;

const POLL: Duration = Duration::from_secs(1);

/// Start polling `/proc` for locker processes. Always succeeds — worst
/// case the scan finds nothing and no events are ever emitted.
pub fn spawn(names: &[String]) -> mpsc::UnboundedReceiver<Lock> {
    let names: Vec<Vec<u8>> = names.iter().map(|n| n.as_bytes().to_vec()).collect();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut locked = false;
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            let now = locker_running(&names);
            if now != locked {
                locked = now;
                let ev = if now { Lock::Locked } else { Lock::Unlocked };
                if tx.send(ev).is_err() {
                    return;
                }
            }
        }
    });
    rx
}

/// `/proc/*/comm` is a 15-byte name — exact match against the watch list.
fn locker_running(names: &[Vec<u8>]) -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return false;
    };
    procs.filter_map(Result::ok).any(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|n| n.as_bytes().iter().all(u8::is_ascii_digit))
            && std::fs::read(e.path().join("comm")).is_ok_and(|c| {
                let c = c.trim_ascii_end();
                names.iter().any(|n| n.as_slice() == c)
            })
    })
}
