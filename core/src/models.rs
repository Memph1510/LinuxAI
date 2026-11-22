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

// ---------------------------------------------------------------------------------------------
// Local models: small models that run on this machine (llama.cpp's server), for jobs like reading email
// ---------------------------------------------------------------------------------------------

/// (name, what, URL pinned to a repository revision, SHA-256, bytes). Measured on 20 synthetic emails with the core's
/// code check: qwen3-4b 16/20 categories, 6/6 injections; qwen3-8b 17/20, 6/6 (docs/MAIL.md).
pub const LOCAL_MODELS: [(&str, &str, &str, &str, u64); 2] = [
    ("qwen3-4b", "Qwen3 4B (4-bit), reads email: the smaller, faster choice",
     "https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/bc640142c66e1fdd12af0bd68f40445458f3869b/Qwen3-4B-Q4_K_M.gguf",
     "7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5", 2_497_280_256),
    ("qwen3-8b", "Qwen3 8B (4-bit), reads email: slightly more accurate, twice the size",
     "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/7c41481f57cb95916b40956ab2f0b139b296d974/Qwen3-8B-Q4_K_M.gguf",
     "d98cdcbd03e17ce47681435b5150e34c1417f50b5c0019dd560e4882c5745785", 5_027_783_488),
];

pub fn local_models_dir() -> std::path::PathBuf {
    std::env::var("LINUXAI_MODELS_DIR").map(Into::into).unwrap_or_else(|_| "/var/lib/linuxai/models".into())
}

/// The local models LinuxAI knows, and which are on this machine.
pub fn local_models() -> Value {
    let dir = local_models_dir();
    let list: Vec<Value> = LOCAL_MODELS.iter().map(|(name, what, _, _, bytes)| json!({
        "name": name, "what": what, "use_as": format!("local:{name}"), "gigabytes": (*bytes as f64 / 1e9 * 10.0).round() / 10.0,
        "installed": dir.join(format!("{name}.gguf")).exists()})).collect();
    json!({"ok": true, "dir": dir, "models": list})
}

/// Download a local model and keep it only if its size and SHA-256 are exactly the pinned ones.
pub fn fetch_local_model(name: &str, download: &dyn Fn(&str, &std::path::Path) -> Result<(), String>) -> Result<Value, String> {
    let (_, _, url, sha, bytes) = LOCAL_MODELS.iter().find(|m| m.0 == name)
        .ok_or_else(|| format!("no local model {name:?}: {}", LOCAL_MODELS.iter().map(|m| m.0).collect::<Vec<_>>().join(", ")))?;
    let dir = local_models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dest = dir.join(format!("{name}.gguf"));
    if dest.exists() && crate::craft::sha256_file(&dest)? == *sha {
        return Ok(json!({"ok": true, "model": name, "path": dest, "already": true}));
    }
    let part = dir.join(format!("{name}.gguf.part"));
    download(url, &part)?;
    let size = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let got = crate::craft::sha256_file(&part)?;
    if size != *bytes || got != *sha {
        let _ = std::fs::remove_file(&part);
        return Err(format!("{name}: the download doesn't match the pinned file ({size} bytes, sha256 {got}); not installed"));
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644));
    Ok(json!({"ok": true, "model": name, "path": dest, "sha256": got, "use_as": format!("local:{name}")}))
}

/// curl over HTTPS only, resuming a partial download.
pub fn curl_download(url: &str, to: &std::path::Path) -> Result<(), String> {
    let out = std::process::Command::new("curl")
        .args(["-fL", "--progress-bar", "--proto", "=https", "--retry", "5", "-C", "-", "-o"]).arg(to).arg(url)
        .status().map_err(|e| format!("curl: {e}"))?;
    if out.success() { Ok(()) } else { Err(format!("download failed ({out})")) }
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

    #[test]
    fn a_local_model_is_kept_only_if_it_is_exactly_the_pinned_file() {
        let dir = std::env::temp_dir().join(format!("linuxai-models-{}", std::process::id()));
        std::env::set_var("LINUXAI_MODELS_DIR", &dir);
        let wrong = |_: &str, to: &std::path::Path| std::fs::write(to, b"not the model").map_err(|e| e.to_string());
        let r = fetch_local_model("qwen3-4b", &wrong);
        assert!(r.is_err(), "{r:?}");
        assert!(!dir.join("qwen3-4b.gguf").exists() && !dir.join("qwen3-4b.gguf.part").exists());
        assert!(fetch_local_model("gpt-9000", &wrong).is_err());
        assert_eq!(local_models()["models"][0]["installed"], false);
        assert!(LOCAL_MODELS.iter().all(|m| m.2.starts_with("https://") && m.2.contains("/resolve/") && !m.2.contains("/main/") && m.3.len() == 64));
        std::env::remove_var("LINUXAI_MODELS_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
