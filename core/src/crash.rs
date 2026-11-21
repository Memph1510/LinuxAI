//! Crashes: contained, recorded, and told to the human.
//!
//! - `Locked::lk()`: take a lock even if a thread panicked while holding it (Rust "poisons" the lock then, and every
//!   later `.lock().unwrap()` would panic too: one bug would take the whole core down with it). The data behind our
//!   locks stays consistent across a panic (plain maps and queues, updated in single statements), so recovering is safe.
//! - `catch()`: run something and turn a panic into an error, so one bad request or plan fails alone.
//! - `install()`: a panic hook for every LinuxAI process. A panic that isn't caught is reported to the core as a
//!   `linuxai.crash` event (the human is texted), as well as printed for the journal.

use serde_json::{json, Value};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard};

pub trait Locked<T> {
    fn lk(&self) -> MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn lk(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The message of a panic payload.
pub fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".into())
}

/// Run `f`; a panic becomes `Err(message)` instead of unwinding further.
pub fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| message(p.as_ref()))
}

/// The event that tells the human (through reach) that part of LinuxAI hit an internal error.
pub fn event(component: &str, what: &str, msg: &str) -> Value {
    json!({"component": component, "what": what, "message": msg.chars().take(500).collect::<String>()})
}

/// For every LinuxAI process: report panics that nothing caught. Organs send them to the core; the core's own are
/// caught where they happen (requests, plans), so this hook mostly matters for the organs.
pub fn install(component: &str) {
    let component = component.to_string();
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        previous(info); // still printed for the journal
        let msg = format!("{}{}", message(info.payload()),
                          info.location().map(|l| format!(" (at {}:{})", l.file(), l.line())).unwrap_or_default());
        if component != "daemon" {
            // Briefly: a hung or absent core mustn't hold up the crashing process.
            use std::io::Write;
            if let Ok(mut s) = std::os::unix::net::UnixStream::connect(crate::default_socket()) {
                let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
                let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let req = json!({"op": "event", "kind": "linuxai.crash", "data": event(&component, "panic", &msg)});
                if writeln!(s, "{req}").is_ok() {
                    let mut buf = [0u8; 256];
                    let _ = std::io::Read::read(&mut s, &mut buf);
                }
            }
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_lock_held_during_a_panic_still_works_afterwards() {
        let m = Arc::new(Mutex::new(vec![1]));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("boom while holding the lock");
        }).join();
        assert!(m.lock().is_err(), "the standard lock is poisoned now");
        m.lk().push(2);
        assert_eq!(*m.lk(), vec![1, 2], "lk() recovers it");
        assert_eq!(catch(|| -> i32 { panic!("bad input") }), Err("bad input".to_string()));
        assert_eq!(catch(|| 7), Ok(7));
    }
}
