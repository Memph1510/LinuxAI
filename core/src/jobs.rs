//! Long-running work (an update, a build, a render, a download) started in the background. The plan that started
//! it carries on, and waits for the job's `job.finished` event with the outcomes it decided in advance: finished
//! cleanly, a failure it knows how to fix, or anything else (think). Nobody polls, and the model isn't involved
//! while the job runs.
//!
//! `{"do": "run", "program": "pacman", "args": ["-Syu", "--noconfirm"], "background": true, "as": "job"}` returns
//! `{job, started}` at once; later `job.finished` carries {job, program, code, ok, output (its end), seconds, error}.
//! If the core restarts while a job runs, its result is lost: the waiting plan's `max` covers that.

use crate::verbs::Organs;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

static NEXT: AtomicU64 = AtomicU64::new(1);
const KEEP_OUTPUT: usize = 4000;

/// Wraps the organs: a `run` with `background: true` goes to a thread; everything else passes straight through.
pub struct Jobs {
    inner: Arc<dyn Organs>,
    pub done: Arc<Mutex<Vec<Value>>>,
}

impl Jobs {
    pub fn new(inner: Box<dyn Organs>) -> Jobs {
        Jobs { inner: Arc::from(inner), done: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Jobs that have finished since the last call.
    pub fn take_finished(&self) -> Vec<Value> {
        std::mem::take(&mut *self.done.lock().unwrap())
    }
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut cut = s.len() - n;
    while !s.is_char_boundary(cut) {
        cut += 1;
    }
    format!("…{}", &s[cut..])
}

impl Organs for Jobs {
    fn invoke(&self, verb: &str, args: &Value) -> Result<Value, String> {
        if verb != "run" || args["background"] != true {
            return self.inner.invoke(verb, args);
        }
        let id = format!("job-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let mut a = args.clone();
        if let Value::Object(m) = &mut a {
            m.remove("background");
            m.entry("timeout").or_insert(json!(4 * 3600)); // long work: hours, not the 5 minutes of a quick command
        }
        let (inner, done, job) = (self.inner.clone(), self.done.clone(), id.clone());
        std::thread::spawn(move || {
            let start = Instant::now();
            let r = inner.invoke("run", &a);
            let (code, output, error) = match &r {
                Ok(v) => (v["code"].as_i64(), v["output"].as_str().unwrap_or("").to_string(), None),
                Err(e) => (None, String::new(), Some(e.clone())),
            };
            let rehearsal = r.as_ref().map(|v| v["rehearsal"] == true).unwrap_or(false);
            done.lock().unwrap().push(json!({"job": job, "program": a["program"], "args": a["args"], "code": code,
                "ok": error.is_none() && (code == Some(0) || rehearsal), "output": tail(&output, KEEP_OUTPUT),
                "error": error, "seconds": start.elapsed().as_secs_f64().round(), "rehearsal": rehearsal}));
        });
        Ok(json!({"job": id, "started": true}))
    }
}
