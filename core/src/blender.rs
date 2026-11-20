//! The Blender organ: 3D work as part of the OS. The AI builds, fixes, renders and exports scenes, and works with
//! the human in the Blender they have open.
//!
//! `linuxai-core blender caps`           what's here: Blender's version, open Blender windows the AI can reach
//! `linuxai-core blender do '<json>'`    one job; prints the result as JSON
//!
//! How it keeps the human's work safe:
//! - **Staged copies.** The AI never saves over a file. Changes go to a new file next to the original
//!   (`scene.blend` → `scene.ai-1.blend`), so textures and links keep working. Putting the result in place of the
//!   original (`replace`) asks the human first and keeps the original as `scene.before-ai-<time>.blend`.
//! - **Checked results.** After a change the organ reopens the new file, compares it with the original (what was
//!   added, removed, changed), renders a preview the AI looks at, and checks the picture isn't blank. It says what
//!   it checked and what it didn't.
//! - **Outside files can't run code.** Every job runs with Blender's auto-run of scripts off, without the user's
//!   add-ons (`--factory-startup --disable-autoexec`). Scripts inside a file are listed, never run.
//! - **Python the AI writes** (the `python` op, for what the other ops can't do) can do anything the user can, so it
//!   needs the human's OK, like the AI's other commands.
//! - **The human's open Blender** (the live bridge) is read freely; changing it needs the human's OK, and each
//!   change is one undo step.
//!
//! The work inside Blender is `linuxai_blender.py` (Blender's own Python, the only way to drive it).
//!
//!   LINUXAI_BLENDER          the blender program (default: `blender` on PATH)
//!   LINUXAI_BLENDER_ENGINE   the engine script (default /usr/share/linuxai/blender/linuxai_blender.py)
//!   LINUXAI_BLENDER_DIR      where new scenes, renders and exports go (default ~/blender)

use crate::mail::Api;
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct BlenderConfig {
    pub program: Option<String>,
    pub engine: PathBuf,
    pub dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl BlenderConfig {
    pub fn from_env() -> BlenderConfig {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
        BlenderConfig {
            program: std::env::var("LINUXAI_BLENDER").ok().filter(|v| !v.is_empty()).or_else(|| which("blender")),
            engine: std::env::var("LINUXAI_BLENDER_ENGINE").map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/usr/share/linuxai/blender/linuxai_blender.py")),
            dir: std::env::var("LINUXAI_BLENDER_DIR").map(PathBuf::from).unwrap_or_else(|_| home.join("blender")),
            runtime_dir: std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(format!("/tmp/linuxai-{}", unsafe_uid()))),
        }
    }
}

fn unsafe_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status").ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|u| u.parse().ok()))
        .unwrap_or(0)
}

fn which(name: &str) -> Option<String> {
    std::env::var("PATH").ok()?.split(':').map(|d| PathBuf::from(d).join(name)).find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

// ------------------------------------------------------------------------------------------------
// Running Blender headless
// ------------------------------------------------------------------------------------------------

/// Run one engine job in a fresh, headless Blender. `file` is opened (read into memory only).
fn run_job(cfg: &BlenderConfig, file: Option<&Path>, job: &Value, timeout: Duration) -> Result<Value, String> {
    let program = cfg.program.as_deref().ok_or("Blender isn't installed (pacman -S blender)")?;
    if !cfg.engine.is_file() {
        return Err(format!("the LinuxAI engine script is missing: {}", cfg.engine.display()));
    }
    let work = std::env::temp_dir().join(format!("linuxai-blender-{}-{}", std::process::id(), rand_id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let (job_path, result_path) = (work.join("job.json"), work.join("result.json"));
    std::fs::write(&job_path, job.to_string()).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(program);
    cmd.args(["-b", "--factory-startup", "--disable-autoexec"]);
    if let Some(f) = file {
        cmd.arg(f);
    }
    cmd.arg("--python").arg(&cfg.engine).arg("--").arg(&job_path).arg(&result_path)
        .env("LINUXAI_BLENDER_BRIDGE", "0")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start Blender: {e}"))?;
    // Drain output on threads so a chatty Blender never blocks on a full pipe.
    let tail = |r: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut lines: Vec<String> = Vec::new();
            if let Some(r) = r {
                for l in BufReader::new(r).lines().map_while(Result::ok) {
                    lines.push(l);
                    if lines.len() > 60 {
                        lines.remove(0);
                    }
                }
            }
            lines
        })
    };
    let out_t = tail(child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));
    let err_t = tail(child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(s);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut log = out_t.join().unwrap_or_default();
    log.extend(err_t.join().unwrap_or_default());
    let result = std::fs::read_to_string(&result_path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let _ = std::fs::remove_dir_all(&work);
    match (status, result) {
        (None, _) => Err(format!("Blender took longer than {}s and was stopped", timeout.as_secs())),
        (Some(_), Some(r)) => Ok(r),
        (Some(s), None) if s.to_string().contains("SIGILL") => Err(
            "Blender can't run on this processor: it needs newer CPU instructions (x86-64-v2 or later) than this machine (or virtual machine) offers".into()),
        (Some(s), None) => {
            let why: Vec<&String> = log.iter().filter(|l| l.contains("Error") || l.contains("error") || l.contains("Traceback")).collect();
            Err(format!("Blender stopped ({s}) without an answer: {}", if why.is_empty() { log.iter().rev().take(5).cloned().collect::<Vec<_>>().join(" | ") }
                else { why.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(" | ") }))
        }
    }
}

fn rand_id() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) & 0xffff_ffff)
}

fn version(cfg: &BlenderConfig) -> Option<String> {
    let out = Command::new(cfg.program.as_deref()?).args(["-b", "--factory-startup", "--version"])
        .env("LINUXAI_BLENDER_BRIDGE", "0").stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find(|l| l.starts_with("Blender ")).map(|l| l.trim().to_string())
}

// ------------------------------------------------------------------------------------------------
// Files: inputs, staged copies, outputs
// ------------------------------------------------------------------------------------------------

fn blend_path(p: &str) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(p).map_err(|e| format!("{p}: {e}"))?;
    if !path.is_file() || path.extension().map(|e| e != "blend").unwrap_or(true) {
        return Err(format!("{p} isn't a .blend file"));
    }
    Ok(path)
}

/// `scene.blend` → `scene.ai-N.blend` next to it (the next free N). A staged copy of a staged copy counts on.
fn staged_path(original: &Path) -> PathBuf {
    let stem = original.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let base = match stem.rsplit_once(".ai-") {
        Some((b, n)) if n.parse::<u32>().is_ok() => b.to_string(),
        _ => stem,
    };
    let dir = original.parent().unwrap_or(Path::new("."));
    (1..).map(|n| dir.join(format!("{base}.ai-{n}.blend"))).find(|p| !p.exists()).unwrap()
}

/// The original a staged copy was made from: `scene.ai-2.blend` → `scene.blend`.
fn original_of(staged: &Path) -> Option<PathBuf> {
    let stem = staged.file_stem()?.to_string_lossy().to_string();
    let (base, n) = stem.rsplit_once(".ai-")?;
    n.parse::<u32>().ok()?;
    Some(staged.with_file_name(format!("{base}.blend")))
}

fn safe_name(s: &str, fallback: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(48).collect();
    let n = n.trim_matches('-').to_string();
    if n.is_empty() { fallback.into() } else { n }
}

fn out_dir(cfg: &BlenderConfig, sub: &str) -> Result<PathBuf, String> {
    let d = cfg.dir.join(sub);
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    Ok(d)
}

fn record(api: &dyn Api, fields: Value) -> Value {
    api.call("object_put", json!({"type": "creation", "fields": fields}))["id"].clone()
}

// ------------------------------------------------------------------------------------------------
// What changed: the before and after of a scene
// ------------------------------------------------------------------------------------------------

fn by_name(scene: &Value) -> Map<String, Value> {
    scene["objects"].as_array().map(|a| a.iter().map(|o| (o["name"].as_str().unwrap_or("").to_string(), o.clone())).collect())
        .unwrap_or_default()
}

pub fn diff(before: &Value, after: &Value) -> Value {
    let (b, a) = (by_name(before), by_name(after));
    let added: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let removed: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let mut changed = Map::new();
    for (name, now) in &a {
        if let Some(was) = b.get(name) {
            let fields: Map<String, Value> = now.as_object().into_iter().flatten()
                .filter(|(k, v)| was.get(k.as_str()) != Some(*v))
                .map(|(k, v)| (k.clone(), json!({"was": was.get(k.as_str()).cloned().unwrap_or(Value::Null), "now": v})))
                .collect();
            let gone: Vec<&String> = was.as_object().into_iter().flatten().map(|(k, _)| k).filter(|k| now.get(k.as_str()).is_none()).collect();
            let mut f = fields;
            for k in gone {
                f.insert(k.clone(), json!({"was": was[k.as_str()], "now": null}));
            }
            if !f.is_empty() {
                changed.insert(name.clone(), Value::Object(f));
            }
        }
    }
    let mut other = Map::new();
    for k in ["render", "units", "materials", "collections"] {
        if before[k] != after[k] {
            other.insert(k.into(), json!({"was": before[k], "now": after[k]}));
        }
    }
    json!({"added": added, "removed": removed, "changed": changed, "scene": other})
}

// ------------------------------------------------------------------------------------------------
// Jobs
// ------------------------------------------------------------------------------------------------

fn needs_python_ok(req: &Value) -> Option<Value> {
    let code: Vec<&str> = req["ops"].as_array()?.iter().filter(|o| o["op"] == "python").filter_map(|o| o["code"].as_str()).collect();
    if code.is_empty() || req["approved"] == true {
        return None;
    }
    Some(json!({"ok": false, "needs_ok": true, "code": code,
        "error": "This runs Python you wrote inside Blender, which can do anything the user can. Ask the human (show them the code), then send approved: true."}))
}

/// Make changes to a scene. With `file`, the changes go to a staged copy next to it; without, a new scene is made
/// in ~/blender/scenes. Either way: reopened, compared, previewed and checked.
fn apply(req: &Value, api: &dyn Api, cfg: &BlenderConfig) -> Result<Value, String> {
    if let Some(r) = needs_python_ok(req) {
        return Ok(r);
    }
    let ops = req["ops"].as_array().filter(|o| !o.is_empty()).ok_or("ops: a list of changes, like [{\"op\": \"add\", \"type\": \"cube\"}]")?;
    let original = match req["file"].as_str() {
        Some(f) => Some(blend_path(f)?),
        None => None,
    };
    let save_as = match &original {
        Some(o) => staged_path(o),
        None => {
            let d = out_dir(cfg, "scenes")?;
            let name = safe_name(req["name"].as_str().unwrap_or("scene"), "scene");
            (0..).map(|n| d.join(if n == 0 { format!("{name}.blend") } else { format!("{name}-{n}.blend") })).find(|p| !p.exists()).unwrap()
        }
    };
    let preview = out_dir(cfg, "previews")?.join(format!("{}-{}.png", save_as.file_stem().unwrap_or_default().to_string_lossy(), stamp()));
    let job = json!({"op": "apply", "ops": ops, "save_as": save_as, "preview": {"output": preview}});
    let r = run_job(cfg, original.as_deref(), &job, Duration::from_secs(req["timeout"].as_u64().unwrap_or(600)))?;
    if r["ok"] != true {
        return Err(r["error"].as_str().unwrap_or("the change failed").to_string() + &r["trace"].as_str().map(|t| format!("\n{t}")).unwrap_or_default());
    }
    // Verify: the saved file opens, and holds what we think it does.
    let reopened = run_job(cfg, Some(&save_as), &json!({"op": "inspect"}), Duration::from_secs(300))?;
    let after = reopened["scene"].clone();
    let mut checked = vec![format!("{} reopens in Blender", save_as.file_name().unwrap_or_default().to_string_lossy())];
    let mut not_checked = vec!["how it looks from angles other than the preview camera".to_string()];
    let preview_v = r["preview"].clone();
    match preview_v["check"]["verdict"].as_str() {
        Some("ok") => checked.push("the preview shows a picture (not blank)".into()),
        Some(v) => checked.push(format!("the preview is {v}: check the lights and the camera")),
        None => not_checked.push("no preview: the scene has no camera".into()),
    }
    if after["objects"].as_array().map(|o| o.iter().any(|o| o["animated"] == true)).unwrap_or(false) {
        not_checked.push("the animation (only one frame was rendered)".into());
    }
    let missing: Vec<&Value> = after["images"].as_array().map(|i| i.iter().filter(|i| i["missing"] == true).collect()).unwrap_or_default();
    if !missing.is_empty() {
        checked.push(format!("{} texture file(s) are missing", missing.len()));
    }
    let changes = diff(&r["before"], &after);
    let id = record(api, json!({"kind": "blender_scene", "path": save_as, "from": original, "ops": ops.len(),
                                "preview": preview_v["path"], "changes": changes}));
    Ok(json!({"ok": true, "saved": save_as, "original": original, "id": id, "done": r["done"], "changes": changes,
              "preview": preview_v, "checked": checked, "not_checked": not_checked,
              "next": if original.is_some() { "the original is untouched; to put this in its place, use replace (asks the human)" } else { "a new scene" }}))
}

fn render(req: &Value, api: &dyn Api, cfg: &BlenderConfig) -> Result<Value, String> {
    let file = blend_path(req["file"].as_str().ok_or("file: the .blend to render")?)?;
    let output = out_dir(cfg, "renders")?.join(format!("{}-{}.png", file.file_stem().unwrap_or_default().to_string_lossy(), stamp()));
    let mut job = json!({"op": "render", "output": output});
    for k in ["frame", "width", "height", "samples", "engine", "denoise"] {
        if !req[k].is_null() {
            job[k] = req[k].clone();
        }
    }
    let r = run_job(cfg, Some(&file), &job, Duration::from_secs(req["timeout"].as_u64().unwrap_or(3600)))?;
    if r["ok"] != true {
        return Err(r["error"].as_str().unwrap_or("the render failed").into());
    }
    let id = record(api, json!({"kind": "render", "path": r["path"], "from": file, "seconds": r["seconds"]}));
    Ok(json!({"id": id, "file": file, "ok": true, "path": r["path"], "seconds": r["seconds"], "engine": r["engine"],
              "size": r["size"], "check": r["check"]}))
}

const FORMATS: [&str; 7] = ["glb", "gltf", "fbx", "obj", "stl", "ply", "usd"];

fn export(req: &Value, api: &dyn Api, cfg: &BlenderConfig) -> Result<Value, String> {
    let file = blend_path(req["file"].as_str().ok_or("file: the .blend to export")?)?;
    let fmt = req["format"].as_str().map(str::to_lowercase).filter(|f| FORMATS.contains(&f.as_str()))
        .ok_or(format!("format: one of {}", FORMATS.join(", ")))?;
    let output = out_dir(cfg, "exports")?.join(format!("{}.{fmt}", file.file_stem().unwrap_or_default().to_string_lossy()));
    let output = if output.exists() { output.with_file_name(format!("{}-{}.{fmt}", file.file_stem().unwrap_or_default().to_string_lossy(), stamp())) } else { output };
    let mut job = json!({"op": "export", "format": fmt, "output": output});
    if req["selection"].is_array() {
        job["selection"] = req["selection"].clone();
    }
    let r = run_job(cfg, Some(&file), &job, Duration::from_secs(900))?;
    if r["ok"] != true {
        return Err(r["error"].as_str().unwrap_or("the export failed").into());
    }
    let id = record(api, json!({"kind": "export", "path": r["path"], "from": file, "format": fmt}));
    Ok(json!({"ok": true, "id": id, "path": r["path"], "bytes": r["bytes"]}))
}

/// Put a staged copy in place of the original it came from. The human's OK first; the original is kept.
fn replace(req: &Value, api: &dyn Api) -> Result<Value, String> {
    let staged = blend_path(req["staged"].as_str().ok_or("staged: the .ai-N.blend to use")?)?;
    let original = original_of(&staged).ok_or("that isn't a staged copy (name.ai-N.blend)")?;
    if !original.is_file() {
        return Err(format!("the original {} isn't there anymore", original.display()));
    }
    if req["approved"] != true {
        return Ok(json!({"ok": false, "needs_ok": true, "error": format!(
            "This replaces {} with {} (the original is kept as a backup). Ask the human, then send approved: true.",
            original.display(), staged.display())}));
    }
    let backup = original.with_file_name(format!("{}.before-ai-{}.blend", original.file_stem().unwrap_or_default().to_string_lossy(), stamp()));
    std::fs::rename(&original, &backup).map_err(|e| format!("couldn't keep the original: {e}"))?;
    if let Err(e) = std::fs::copy(&staged, &original) {
        let _ = std::fs::rename(&backup, &original);
        return Err(format!("couldn't put the new version in place (the original is back): {e}"));
    }
    let id = record(api, json!({"kind": "blender_replace", "path": original, "from": staged, "backup": backup}));
    Ok(json!({"ok": true, "id": id, "replaced": original, "backup": backup}))
}

// ------------------------------------------------------------------------------------------------
// The human's open Blender (the live bridge)
// ------------------------------------------------------------------------------------------------

/// Open Blender windows that answer: (pid, socket).
pub fn live_sessions(cfg: &BlenderConfig) -> Vec<(u32, PathBuf)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(&cfg.runtime_dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(pid) = name.strip_prefix("linuxai-blender-").and_then(|s| s.strip_suffix(".sock")).and_then(|p| p.parse::<u32>().ok()) else { continue };
        if Path::new(&format!("/proc/{pid}")).exists() {
            out.push((pid, e.path()));
        } else {
            let _ = std::fs::remove_file(e.path()); // left behind by a Blender that crashed
        }
    }
    out.sort();
    out
}

fn live_call(sock: &Path, req: &Value, timeout: Duration) -> Result<Value, String> {
    let mut s = std::os::unix::net::UnixStream::connect(sock).map_err(|e| format!("Blender isn't answering: {e}"))?;
    s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    s.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| format!("no answer from Blender: {e}"))?;
    serde_json::from_str(&line).map_err(|e| format!("Blender's answer didn't parse: {e}"))
}

fn live(req: &Value, cfg: &BlenderConfig) -> Result<Value, String> {
    let sessions = live_sessions(cfg);
    let (pid, sock) = match req["session"].as_u64() {
        Some(p) => sessions.iter().find(|(pid, _)| *pid as u64 == p).cloned().ok_or(format!("no open Blender with pid {p}"))?,
        None => match sessions.as_slice() {
            [] => return Err("Blender isn't open (or was started without LinuxAI's bridge)".into()),
            [one] => one.clone(),
            many => return Err(format!("{} Blender windows are open; say which (session: one of {:?})", many.len(),
                                       many.iter().map(|(p, _)| p).collect::<Vec<_>>())),
        },
    };
    let op = req["op"].as_str().unwrap_or("").trim_start_matches("live_");
    let mut msg = json!({"op": op});
    match op {
        "view" | "inspect" => {}
        "apply" => {
            if req["ops"].as_array().map(|o| o.iter().any(|o| o["op"] == "python")).unwrap_or(false) && req["approved"] != true {
                return Ok(needs_python_ok(req).unwrap());
            }
            if req["approved"] != true {
                return Ok(json!({"ok": false, "needs_ok": true, "error":
                    "This changes the scene the human has open in Blender (one undo step). Do it only when they asked; confirm with them, then send approved: true."}));
            }
            msg["ops"] = req["ops"].clone();
            msg["why"] = req["why"].clone();
        }
        "render" => {
            msg["output"] = json!(out_dir(cfg, "previews")?.join(format!("live-{pid}-{}.png", stamp())));
            for k in ["width", "height", "samples", "engine", "frame"] {
                if !req[k].is_null() {
                    msg[k] = req[k].clone();
                }
            }
        }
        other => return Err(format!("live op {other:?}: live_view, live_inspect, live_apply, live_render")),
    }
    let mut r = live_call(&sock, &msg, Duration::from_secs(if op == "render" { 600 } else { 60 }))?;
    r["session"] = json!(pid);
    Ok(r)
}

// ------------------------------------------------------------------------------------------------

pub fn capabilities(cfg: &BlenderConfig) -> Value {
    json!({"blender": cfg.program, "version": version(cfg), "engine": cfg.engine, "engine_present": cfg.engine.is_file(),
           "dir": cfg.dir, "open": live_sessions(cfg).iter().map(|(p, _)| p).collect::<Vec<_>>(), "export_formats": FORMATS})
}

pub fn handle(req: &Value, api: &dyn Api, cfg: &BlenderConfig) -> Value {
    let r = match req["op"].as_str().unwrap_or("") {
        "caps" => Ok(json!({"ok": true, "caps": capabilities(cfg)})),
        "inspect" => blend_path(req["file"].as_str().unwrap_or(""))
            .and_then(|f| run_job(cfg, Some(&f), &json!({"op": "inspect"}), Duration::from_secs(300))),
        "apply" | "new" => apply(req, api, cfg),
        "render" => render(req, api, cfg),
        "export" => export(req, api, cfg),
        "replace" => replace(req, api),
        op if op.starts_with("live_") => live(req, cfg),
        other => Err(format!("unknown blender op {other:?}: caps, inspect, new, apply, render, export, replace, live_view, live_inspect, live_apply, live_render")),
    };
    r.unwrap_or_else(|e| json!({"ok": false, "error": e}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Core;
    use crate::memory::Memory;
    use std::sync::Arc;

    struct NoHost;
    impl crate::verbs::Organs for NoHost {
        fn invoke(&self, _verb: &str, _args: &Value) -> Result<Value, String> {
            Ok(json!({"ok": true}))
        }
    }

    fn setup(name: &str) -> (Arc<Core>, BlenderConfig, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lx-blend-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../image/airootfs/usr/share/linuxai/blender/linuxai_blender.py");
        let cfg = BlenderConfig { program: which("blender"), engine, dir: dir.join("blender"), runtime_dir: dir.join("run") };
        (Arc::new(Core::new(Memory::in_memory(), Box::new(NoHost))), cfg, dir)
    }

    #[test]
    fn staged_copies_never_take_the_originals_name() {
        let dir = std::env::temp_dir().join(format!("lx-blend-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let o = dir.join("kitchen.blend");
        assert_eq!(staged_path(&o), dir.join("kitchen.ai-1.blend"));
        std::fs::write(dir.join("kitchen.ai-1.blend"), b"x").unwrap();
        assert_eq!(staged_path(&o), dir.join("kitchen.ai-2.blend"));
        assert_eq!(staged_path(&dir.join("kitchen.ai-1.blend")), dir.join("kitchen.ai-2.blend"), "a copy of a copy counts on");
        assert_eq!(original_of(&dir.join("kitchen.ai-2.blend")), Some(dir.join("kitchen.blend")));
        assert_eq!(original_of(&dir.join("kitchen.blend")), None);
        assert_eq!(original_of(&dir.join("my.ai-notes.blend")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_diff_says_what_changed() {
        let before = json!({"objects": [{"name": "Cube", "location": [0, 0, 0]}, {"name": "Light", "type": "LIGHT"}], "render": {"width": 1920}});
        let after = json!({"objects": [{"name": "Cube", "location": [0, 0, 1], "materials": ["Red"]}, {"name": "Ball"}], "render": {"width": 1280}});
        let d = diff(&before, &after);
        assert_eq!(d["added"], json!(["Ball"]));
        assert_eq!(d["removed"], json!(["Light"]));
        assert_eq!(d["changed"]["Cube"]["location"]["now"], json!([0, 0, 1]));
        assert_eq!(d["changed"]["Cube"]["materials"]["was"], Value::Null);
        assert_eq!(d["scene"]["render"]["now"]["width"], 1280);
    }

    #[test]
    fn python_and_the_humans_open_scene_need_their_ok() {
        let (core, cfg, dir) = setup("asks");
        let r = handle(&json!({"op": "new", "ops": [{"op": "python", "code": "import os"}]}), &core, &cfg);
        assert_eq!(r["needs_ok"], true, "{r}");
        assert_eq!(r["code"][0], "import os");
        assert!(handle(&json!({"op": "live_view"}), &core, &cfg)["error"].as_str().unwrap().contains("isn't open"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_asks_first_and_keeps_the_original() {
        let (core, _cfg, dir) = setup("replace");
        std::fs::write(dir.join("room.blend"), b"original").unwrap();
        std::fs::write(dir.join("room.ai-1.blend"), b"improved").unwrap();
        let staged = dir.join("room.ai-1.blend");
        let r = handle(&json!({"op": "replace", "staged": staged}), &core, &_cfg);
        assert_eq!(r["needs_ok"], true, "{r}");
        assert_eq!(std::fs::read(dir.join("room.blend")).unwrap(), b"original");
        let r = handle(&json!({"op": "replace", "staged": staged, "approved": true}), &core, &_cfg);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(std::fs::read(dir.join("room.blend")).unwrap(), b"improved");
        assert_eq!(std::fs::read(r["backup"].as_str().unwrap()).unwrap(), b"original");
        assert!(handle(&json!({"op": "replace", "staged": dir.join("room.blend")}), &core, &_cfg)["error"].as_str().unwrap().contains("staged"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real thing, when Blender is installed: build a scene, change a copy, check, render, export.
    #[test]
    fn builds_changes_checks_renders_and_exports_with_real_blender() {
        let (core, cfg, dir) = setup("real");
        if cfg.program.is_none() {
            eprintln!("no Blender; skipping");
            return;
        }
        let r = handle(&json!({"op": "new", "name": "room", "ops": [
            {"op": "delete", "name": ["Cube", "Light", "Camera"]},
            {"op": "add", "type": "plane", "name": "Floor", "size": 10},
            {"op": "add", "type": "sphere", "name": "Ball", "location": [0, 0, 1]},
            {"op": "material", "name": "Red", "color": "#d03a2f", "assign_to": ["Ball"]},
            {"op": "light", "kind": "sun", "energy": 4, "rotation": [40, 10, 30]},
            {"op": "camera", "name": "Cam", "location": [6, -6, 4], "look_at": "Ball"}]}), &core, &cfg);
        assert_eq!(r["ok"], true, "{r}");
        let scene = PathBuf::from(r["saved"].as_str().unwrap());
        assert!(scene.ends_with("scenes/room.blend"));
        assert_eq!(r["preview"]["check"]["verdict"], "ok", "{r}");
        assert!(r["changes"]["added"].as_array().unwrap().contains(&json!("Ball")));
        let before = std::fs::read(&scene).unwrap();

        // A change goes to a staged copy; the original is byte-for-byte untouched.
        let r = handle(&json!({"op": "apply", "file": scene, "ops": [
            {"op": "transform", "name": "Ball", "location": [1, 0, 1]},
            {"op": "material", "name": "Red", "color": "#2f6fd0"}]}), &core, &cfg);
        assert_eq!(r["ok"], true, "{r}");
        assert!(r["saved"].as_str().unwrap().ends_with("room.ai-1.blend"));
        assert_eq!(std::fs::read(&scene).unwrap(), before, "the original is untouched");
        assert_eq!(r["changes"]["changed"]["Ball"]["location"]["now"], json!([1.0, 0.0, 1.0]), "{r}");
        assert!(r["checked"][0].as_str().unwrap().contains("reopens"));

        // A failing step stops the job and says which one; nothing is saved.
        let r = handle(&json!({"op": "apply", "file": scene, "ops": [{"op": "transform", "name": "Nope", "location": [0, 0, 0]}]}), &core, &cfg);
        assert!(r["error"].as_str().unwrap().contains("step 0 (transform): no object named 'Nope'"), "{r}");
        assert!(!scene.with_file_name("room.ai-2.blend").exists());

        // An outside file with a script in it: listed, never run.
        let marker = dir.join("ran.txt");
        let r = handle(&json!({"op": "apply", "file": scene, "approved": true, "ops": [{"op": "python",
            "code": format!("t = bpy.data.texts.new('evil.py'); t.write('open({:?}, \"w\").write(\"x\")'); t.use_module = True", marker.display().to_string())}]}), &core, &cfg);
        assert_eq!(r["ok"], true, "{r}");
        let evil = r["saved"].as_str().unwrap().to_string();
        let i = handle(&json!({"op": "inspect", "file": evil}), &core, &cfg);
        assert_eq!(i["scene"]["scripts_in_file"], json!(["evil.py"]), "{i}");
        assert!(!marker.exists(), "a script inside a file never runs");

        let r = handle(&json!({"op": "render", "file": scene, "width": 160, "height": 90, "samples": 4, "engine": "cycles"}), &core, &cfg);
        assert_eq!(r["check"]["verdict"], "ok", "{r}");
        let r = handle(&json!({"op": "export", "file": scene, "format": "glb"}), &core, &cfg);
        assert!(r["bytes"].as_u64().unwrap() > 1000, "{r}");
        let found = core.handle(&json!({"op": "find", "spec": {"type": "creation"}}));
        assert!(found["found"].as_array().unwrap().len() >= 5, "{found}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
