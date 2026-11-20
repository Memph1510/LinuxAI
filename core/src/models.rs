//! Which model does which job: one table, yours to set.
//!
//! Every job that uses a model (thinking with you, working behind the scenes, reading email, making pictures,
//! turning speech into text, speaking) looks here when it picks one. The table says what each job uses and why:
//! your choice, a setting in the environment, or LinuxAI's default. Only you change it (the Workspace, or the
//! session as the human); the AI can read it and suggest, and every change is an event in the journal.
//!
//! Precedence for each job: your choice in the table, then the environment variable that job has always read,
//! then the default.

use crate::engine::Bus;
use crate::memory::Memory;
use serde_json::{json, Map, Value};

const ID: &str = "obj:settings/models";

pub const PICTURE_BACKENDS: [&str; 3] = ["local_gpu", "paid", "local_cpu"];
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// (job, what it's for, the environment variable it has always read, the default)
pub const JOBS: [(&str, &str, &str, &str); 7] = [
    ("thinking", "the AI you talk with", "LINUXAI_MODEL", "claude-opus-5-5"),
    ("thinking_effort", "how hard it thinks", "LINUXAI_EFFORT", "high"),
    ("background", "the AI working behind the scenes (email, events, plans that need judgment)", "LINUXAI_MIND_MODEL", ""),
    ("reading_email", "turning each email into claims", "LINUXAI_EXTRACT_MODEL", ""),
    ("pictures", "making pictures from a description, in order of preference", "LINUXAI_IMAGE_ORDER", "local_gpu,paid,local_cpu"),
    ("speech_to_text", "your voice to text (a whisper.cpp model file)", "LINUXAI_WHISPER_MODEL", "~/.local/share/linuxai/models/ggml-base.en.bin"),
    ("speaking", "the AI's voice (a Piper .onnx voice file, or espeak-ng)", "LINUXAI_PIPER_VOICE", "espeak-ng"),
];

fn stored(mem: &Memory) -> Map<String, Value> {
    mem.object(ID).and_then(|o| o["fields"]["choices"].as_object().cloned())
        .or_else(|| mem.object(ID).and_then(|o| o["choices"].as_object().cloned()))
        .unwrap_or_default()
}

/// What a job uses now, and why. An empty default means "the same as thinking".
pub fn effective(mem: &Memory, job: &str) -> Option<(String, &'static str)> {
    let (_, _, var, default) = JOBS.iter().find(|j| j.0 == job)?;
    if let Some(v) = stored(mem).get(job).and_then(Value::as_str).filter(|v| !v.is_empty()) {
        return Some((v.to_string(), "your choice"));
    }
    if let Some(v) = std::env::var(var).ok().filter(|v| !v.trim().is_empty()) {
        return Some((v, "environment"));
    }
    if default.is_empty() {
        return effective(mem, "thinking").map(|(v, _)| (v, "same as thinking"));
    }
    Some((default.to_string(), "default"))
}

pub fn table(mem: &Memory) -> Value {
    let jobs: Vec<Value> = JOBS.iter().map(|(job, what, var, default)| {
        let (value, from) = effective(mem, job).unwrap_or_default();
        json!({"job": job, "what": what, "uses": value, "from": from, "env": var, "default": default})
    }).collect();
    json!({"ok": true, "jobs": jobs})
}

fn check(job: &str, v: &str) -> Result<(), String> {
    let v = v.trim();
    match job {
        _ if v.is_empty() => Ok(()), // back to the environment or default
        "thinking_effort" if !EFFORTS.contains(&v) => Err(format!("effort is one of {}", EFFORTS.join(", "))),
        "pictures" => {
            let parts: Vec<&str> = v.split(',').map(str::trim).collect();
            match parts.iter().find(|p| !PICTURE_BACKENDS.contains(p)) {
                Some(bad) => Err(format!("{bad:?} isn't a picture backend ({})", PICTURE_BACKENDS.join(", "))),
                None => Ok(()),
            }
        }
        _ if v.chars().any(|c| c.is_control()) || v.len() > 300 => Err("that doesn't look like a model name or file".into()),
        _ => Ok(()),
    }
}

/// The human sets a job's model (empty: back to the environment or default).
pub fn set(mem: &Memory, bus: &Bus, job: &str, value: &str) -> Value {
    if !JOBS.iter().any(|j| j.0 == job) {
        return json!({"ok": false, "error": format!("no job {job:?}: {}", JOBS.iter().map(|j| j.0).collect::<Vec<_>>().join(", "))});
    }
    if let Err(e) = check(job, value) {
        return json!({"ok": false, "error": e});
    }
    let before = effective(mem, job).map(|(v, _)| v);
    let mut choices = stored(mem);
    if value.trim().is_empty() {
        choices.remove(job);
    } else {
        choices.insert(job.into(), json!(value.trim()));
    }
    mem.put_object_with_id(ID, "settings", &json!({"choices": choices}));
    let after = effective(mem, job);
    bus.emit(mem, "models.changed", json!({"job": job, "was": before, "now": after.as_ref().map(|a| &a.0), "from": after.as_ref().map(|a| a.1)}));
    json!({"ok": true, "job": job, "uses": after.as_ref().map(|a| &a.0), "from": after.map(|a| a.1)})
}

/// Organs outside the core process ask it (a short request on the default socket).
pub fn ask(client: &crate::client::Client, job: &str) -> Option<String> {
    let r = client.request("models", json!({})).ok()?;
    r["jobs"].as_array()?.iter().find(|j| j["job"] == job)?["uses"].as_str().filter(|s| !s.is_empty()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Core;
    use std::sync::Arc;

    struct NoOrgans;
    impl crate::verbs::Organs for NoOrgans {
        fn invoke(&self, _: &str, _: &Value) -> Result<Value, String> { Ok(json!({})) }
    }

    #[test]
    fn one_table_yours_to_set() {
        let c = Arc::new(Core::new(Memory::in_memory(), Box::new(NoOrgans)));
        let job = |t: &Value, j: &str| t["jobs"].as_array().unwrap().iter().find(|x| x["job"] == j).cloned().unwrap();
        let t = c.handle(&json!({"op": "models"}));
        assert_eq!(job(&t, "speaking")["uses"], "espeak-ng");
        let bg = job(&t, "background");
        assert_eq!(bg["uses"], job(&t, "thinking")["uses"], "background follows thinking unless set");
        assert_eq!(bg["from"], "same as thinking");
        // The AI can't change it; the human can, and it's journaled.
        assert_eq!(c.handle(&json!({"op": "models_set", "job": "reading_email", "uses": "claude-haiku-5-5"}))["ok"], false);
        let r = c.handle(&json!({"op": "models_set", "source": "human", "job": "reading_email", "uses": "claude-haiku-5-5"}));
        assert_eq!(r["ok"], true, "{r}");
        let t = c.handle(&json!({"op": "models"}));
        assert_eq!(job(&t, "reading_email")["uses"], "claude-haiku-5-5");
        assert_eq!(job(&t, "reading_email")["from"], "your choice");
        let evs = c.handle(&json!({"op": "events", "since": 0, "limit": 100}))["events"].as_array().unwrap().clone();
        assert!(evs.iter().any(|e| e["kind"] == "models.changed" && e["data"]["now"] == "claude-haiku-5-5"));
        // Checked: unknown jobs, bad picture backends, bad effort; empty goes back to the default.
        assert!(c.handle(&json!({"op": "models_set", "source": "human", "job": "dreams", "uses": "x"}))["error"].as_str().unwrap().contains("no job"));
        assert_eq!(c.handle(&json!({"op": "models_set", "source": "human", "job": "pictures", "uses": "paid,cloud"}))["ok"], false);
        assert_eq!(c.handle(&json!({"op": "models_set", "source": "human", "job": "thinking_effort", "uses": "turbo"}))["ok"], false);
        assert_eq!(c.handle(&json!({"op": "models_set", "source": "human", "job": "pictures", "uses": "local_cpu,paid"}))["ok"], true);
        c.handle(&json!({"op": "models_set", "source": "human", "job": "reading_email", "uses": ""}));
        assert_eq!(job(&c.handle(&json!({"op": "models"})), "reading_email")["from"], "same as thinking");
    }
}
