//! The image organ: create and edit pictures as part of the OS, on whatever machine it's running on.
//!
//! `linuxai-core image caps`              what this machine can do (graphics card and its memory, tools, backends)
//! `linuxai-core image do '<json>'`       one job; prints the result as JSON
//!
//! It runs as the user (never as root), like mail, and records every picture it makes in the core's memory
//! (type "image": path, what made it, from what, at what cost) so the photo view can show results and take comments.
//!
//! How each job is done:
//! - **Precise edits** (crop, resize, rotate, flip, brightness/contrast, grayscale, text, format, strip metadata)
//!   are always local, with ImageMagick: exact, instant, free. Arguments are passed directly, never through a shell.
//! - **Making or changing a picture from a description** goes to the best backend available, in this order:
//!   a local model on the graphics card (e.g. FLUX or Qwen-Image-Edit through ComfyUI) → a paid service, within the
//!   monthly budget → a local model on the CPU (e.g. stable-diffusion.cpp; slow but free). Nothing is off the table:
//!   each is a command you configure, so any tool or service can be plugged in.
//!
//! The backend contract: the command gets JSON on stdin {"prompt", "size", "inputs": [paths], "output": path},
//! writes the picture to `output`, and may print JSON such as {"cost_cents": 4, "model": "..."}. Exit 0 = done.
//!
//!   LINUXAI_IMAGES                 where pictures go (default ~/images)
//!   LINUXAI_IMAGE_LOCAL_CMD        local model on the graphics card
//!   LINUXAI_IMAGE_PAID_CMD         paid service; LINUXAI_IMAGE_PAID_CENTS = estimated cost per picture (default 5)
//!   LINUXAI_IMAGE_BUDGET_CENTS     monthly budget for paid pictures (default 1000 = $10); past it, it asks first
//!   LINUXAI_IMAGE_CPU_CMD          local model on the CPU

use crate::mail::Api;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn which(name: &str) -> Option<String> {
    std::env::var("PATH").ok()?.split(':').map(|d| PathBuf::from(d).join(name)).find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

/// ImageMagick 7 (`magick`) or 6 (`convert`/`identify`).
fn magick() -> Option<(String, Vec<String>)> {
    if let Some(m) = which("magick") {
        return Some((m, vec![]));
    }
    which("convert").map(|c| (c, vec![]))
}

fn identify_cmd() -> Option<(String, Vec<String>)> {
    if let Some(m) = which("magick") {
        return Some((m, vec!["identify".into()]));
    }
    which("identify").map(|c| (c, vec![]))
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Graphics cards: vendor, name and memory (GB) where the driver says.
pub fn gpus() -> Vec<Value> {
    let mut out = Vec::new();
    // NVIDIA's own tool knows the memory exactly.
    if let Some(smi) = which("nvidia-smi") {
        if let Ok(o) = Command::new(smi).args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]).output() {
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some((name, mem)) = l.split_once(',') {
                    let gb = mem.trim().parse::<f64>().map(|m| (m / 1024.0 * 10.0).round() / 10.0).ok();
                    out.push(json!({"vendor": "nvidia", "name": name.trim(), "vram_gb": gb}));
                }
            }
        }
    }
    let Ok(cards) = std::fs::read_dir("/sys/class/drm") else { return out };
    for c in cards.flatten() {
        let name = c.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = c.path().join("device");
        let vendor = match read_trim(&dev.join("vendor")).as_deref() {
            Some("0x10de") => "nvidia",
            Some("0x1002") => "amd",
            Some("0x8086") => "intel",
            Some(_) => "other",
            None => continue,
        };
        if vendor == "nvidia" && !out.is_empty() {
            continue; // already listed by nvidia-smi
        }
        let vram = read_trim(&dev.join("mem_info_vram_total")).and_then(|b| b.parse::<f64>().ok())
            .map(|b| (b / 1e9 * 10.0).round() / 10.0);
        out.push(json!({"vendor": vendor, "card": name, "vram_gb": vram}));
    }
    out
}

fn ram_gb() -> Option<f64> {
    let m = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: f64 = m.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some((kb / 1048576.0 * 10.0).round() / 10.0)
}

pub struct ImageConfig {
    pub dir: PathBuf,
    pub local_cmd: Option<String>,
    pub paid_cmd: Option<String>,
    pub paid_cents: i64,
    pub budget_cents: i64,
    pub cpu_cmd: Option<String>,
}

impl ImageConfig {
    pub fn from_env() -> ImageConfig {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
        ImageConfig {
            dir: env("LINUXAI_IMAGES").map(PathBuf::from).unwrap_or_else(|| home.join("images")),
            local_cmd: env("LINUXAI_IMAGE_LOCAL_CMD"),
            paid_cmd: env("LINUXAI_IMAGE_PAID_CMD"),
            paid_cents: env("LINUXAI_IMAGE_PAID_CENTS").and_then(|v| v.parse().ok()).unwrap_or(5),
            budget_cents: env("LINUXAI_IMAGE_BUDGET_CENTS").and_then(|v| v.parse().ok()).unwrap_or(1000),
            cpu_cmd: env("LINUXAI_IMAGE_CPU_CMD"),
        }
    }
}

pub fn capabilities(cfg: &ImageConfig) -> Value {
    let tools: Vec<&str> = ["magick", "convert", "gimp", "inkscape", "blender", "comfyui", "sd"].into_iter()
        .filter(|t| which(t).is_some()).collect();
    let gpus = gpus();
    let best_vram = gpus.iter().filter_map(|g| g["vram_gb"].as_f64()).fold(0.0, f64::max);
    json!({
        "gpus": gpus, "ram_gb": ram_gb(), "tools": tools,
        "precise_edits": magick().is_some(),
        "backends": {
            "local_gpu": cfg.local_cmd.is_some(),
            "paid": cfg.paid_cmd.as_ref().map(|_| json!({"cents_per_picture": cfg.paid_cents, "monthly_budget_cents": cfg.budget_cents})),
            "local_cpu": cfg.cpu_cmd.is_some(),
        },
        "advice": advice(best_vram, cfg),
    })
}

/// What would make this machine better at pictures, in plain words.
fn advice(vram: f64, cfg: &ImageConfig) -> String {
    let mut a = Vec::new();
    if vram >= 12.0 && cfg.local_cmd.is_none() {
        a.push(format!("This graphics card ({vram} GB) can run FLUX or Qwen-Image-Edit locally: set up ComfyUI and LINUXAI_IMAGE_LOCAL_CMD."));
    } else if vram >= 6.0 && cfg.local_cmd.is_none() {
        a.push(format!("This graphics card ({vram} GB) can run smaller or quantized models (SDXL, FLUX GGUF) locally."));
    }
    if cfg.local_cmd.is_none() && cfg.paid_cmd.is_none() && cfg.cpu_cmd.is_none() {
        a.push("Nothing can make pictures from a description yet: configure a paid service (fastest to set up), a local model, or a CPU model.".into());
    }
    if magick().is_none() {
        a.push("ImageMagick is missing, so precise edits can't run.".into());
    }
    a.join(" ")
}

fn month() -> String {
    chrono::Local::now().format("%Y-%m").to_string()
}

/// What's been spent on paid pictures this month (from the image records).
pub fn spent_this_month(api: &dyn Api) -> i64 {
    let r = api.call("find", json!({"spec": {"type": "image", "where": {"month": month()}, "last": 10000}}));
    r["found"].as_array().map(|a| a.iter().map(|o| o["cost_cents"].as_i64().unwrap_or(0)).sum()).unwrap_or(0)
}

/// Turn one precise operation into ImageMagick arguments. Only known operations, only checked values.
fn op_args(op: &Value) -> Result<Vec<String>, String> {
    let geom = |s: &str, allow_offset: bool| {
        let ok = !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || "x%!<>^".contains(c) || (allow_offset && "+-".contains(c)));
        if ok { Ok(s.to_string()) } else { Err(format!("bad size {s:?} (like 1200x800, 50%, or 800x600+10+20 for crop)")) }
    };
    let o = op.as_object().ok_or("each operation is an object, like {\"resize\": \"1200x800\"}")?;
    // The operation is the key that names one (others, like gravity or size for text, are its settings).
    const OPS: [&str; 12] = ["resize", "crop", "rotate", "flip", "brightness", "contrast", "grayscale", "sharpen", "blur",
                             "strip", "quality", "text"];
    let k = OPS.iter().find(|k| o.contains_key(**k)).copied()
        .or_else(|| o.keys().next().map(String::as_str)).ok_or("empty operation")?;
    let v = o.get(k).unwrap_or(&Value::Null);
    let num = |v: &Value| v.as_f64().ok_or(format!("{k} needs a number"));
    Ok(match k {
        "resize" => vec!["-resize".into(), geom(v.as_str().unwrap_or(""), false)?],
        "crop" => vec!["-crop".into(), geom(v.as_str().unwrap_or(""), true)?, "+repage".into()],
        "rotate" => vec!["-rotate".into(), num(v)?.to_string()],
        "flip" => vec![if v == "horizontal" || v == "h" { "-flop" } else { "-flip" }.into()],
        "brightness" | "contrast" => {
            let b = o.get("brightness").and_then(Value::as_f64).unwrap_or(0.0);
            let c = o.get("contrast").and_then(Value::as_f64).unwrap_or(0.0);
            vec!["-brightness-contrast".into(), format!("{b}x{c}")]
        }
        "grayscale" => vec!["-colorspace".into(), "Gray".into()],
        "sharpen" => vec!["-sharpen".into(), format!("0x{}", num(v)?)],
        "blur" => vec!["-blur".into(), format!("0x{}", num(v)?)],
        "strip" => vec!["-strip".into()],
        "quality" => vec!["-quality".into(), num(v)?.to_string()],
        "text" => {
            let text = v.as_str().ok_or("text needs words")?;
            let gravity = o.get("gravity").and_then(Value::as_str).unwrap_or("south");
            if !["north", "south", "east", "west", "center", "northeast", "northwest", "southeast", "southwest"].contains(&gravity) {
                return Err("gravity: north, south, east, west, center, or a corner like southeast".into());
            }
            let size = o.get("size").and_then(Value::as_f64).unwrap_or(36.0);
            let color = o.get("color").and_then(Value::as_str).unwrap_or("white");
            if !color.chars().all(|c| c.is_ascii_alphanumeric() || c == '#') {
                return Err("color: a name like white or a hex like #ff8800".into());
            }
            // ImageMagick reads text starting with @ from a file: never let it.
            let safe = if text.starts_with('@') { format!(" {text}") } else { text.to_string() };
            vec!["-gravity".into(), gravity.into(), "-pointsize".into(), size.to_string(), "-fill".into(), color.into(),
                 "-annotate".into(), "+0+20".into(), safe]
        }
        _ => return Err(format!("unknown edit {k:?}: resize, crop, rotate, flip, brightness/contrast, grayscale, sharpen, blur, text, quality, strip")),
    })
}

fn input_path(p: &str) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(p).map_err(|e| format!("{p}: {e}"))?;
    if !path.is_file() {
        return Err(format!("{p} isn't a file"));
    }
    Ok(path)
}

fn out_path(cfg: &ImageConfig, name: &str, ext: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&cfg.dir).map_err(|e| e.to_string())?;
    let stem: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).take(40).collect();
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "picture" } else { stem };
    for n in 0..10000 {
        let p = cfg.dir.join(if n == 0 { format!("{stem}.{ext}") } else { format!("{stem}-{n}.{ext}") });
        if !p.exists() {
            return Ok(p);
        }
    }
    Err("too many pictures with that name".into())
}

pub fn info(path: &Path) -> Value {
    let Some((bin, pre)) = identify_cmd() else { return json!({}) };
    let out = Command::new(bin).args(pre).args(["-format", "%w %h %m"]).arg(format!("{}[0]", path.display())).output();
    let s = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let f: Vec<&str> = s.split_whitespace().collect();
    json!({"width": f.first().and_then(|v| v.parse::<u64>().ok()), "height": f.get(1).and_then(|v| v.parse::<u64>().ok()),
           "format": f.get(2)})
}

fn record(api: &dyn Api, fields: Value) -> Value {
    let r = api.call("object_put", json!({"type": "image", "fields": fields}));
    r["id"].clone()
}

fn edit(req: &Value, api: &dyn Api, cfg: &ImageConfig) -> Result<Value, String> {
    let (bin, pre) = magick().ok_or("ImageMagick isn't installed")?;
    let input = input_path(req["input"].as_str().ok_or("input: the picture to edit")?)?;
    let ops = req["ops"].as_array().ok_or("ops: a list of edits, like [{\"resize\": \"1200x800\"}]")?;
    let mut args: Vec<String> = pre;
    args.push(format!("{}[0]", input.display()));
    for op in ops {
        args.extend(op_args(op)?);
    }
    let ext = req["format"].as_str().map(str::to_lowercase)
        .or_else(|| input.extension().map(|e| e.to_string_lossy().to_lowercase())).unwrap_or_else(|| "png".into());
    if !["png", "jpg", "jpeg", "webp", "gif", "tif", "tiff", "bmp", "avif"].contains(&ext.as_str()) {
        return Err(format!("can't save as {ext}"));
    }
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let out = out_path(cfg, req["name"].as_str().unwrap_or(&format!("{stem}-edited")), &ext)?;
    args.push(out.display().to_string());
    let o = Command::new(bin).args(&args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if !o.status.success() || !out.exists() {
        return Err(format!("the edit failed: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let id = record(api, json!({"path": out, "kind": "edit", "from": input, "ops": ops, "backend": "imagemagick",
                                "cost_cents": 0, "month": month()}));
    Ok(json!({"ok": true, "id": id, "path": out, "backend": "imagemagick", "info": info(&out)}))
}

/// The backends that can do a generative job here, best first, with why.
pub fn route(cfg: &ImageConfig, api: &dyn Api) -> Vec<Value> {
    let mut r = Vec::new();
    if let Some(c) = &cfg.local_cmd {
        r.push(json!({"backend": "local_gpu", "cmd": c, "cents": 0, "why": "on this machine's graphics card: free and private"}));
    }
    if let Some(c) = &cfg.paid_cmd {
        let left = cfg.budget_cents - spent_this_month(api);
        r.push(json!({"backend": "paid", "cmd": c, "cents": cfg.paid_cents, "budget_left_cents": left,
                      "why": "a paid service: fast and high quality, costs money"}));
    }
    if let Some(c) = &cfg.cpu_cmd {
        r.push(json!({"backend": "local_cpu", "cmd": c, "cents": 0, "why": "on the CPU: free but slow"}));
    }
    r
}

fn generate(req: &Value, api: &dyn Api, cfg: &ImageConfig) -> Result<Value, String> {
    let prompt = req["prompt"].as_str().filter(|p| !p.trim().is_empty()).ok_or("prompt: describe the picture")?;
    let size = req["size"].as_str().unwrap_or("1024x1024");
    if !size.split_once('x').map(|(w, h)| w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok()).unwrap_or(false) {
        return Err("size like 1024x1024".into());
    }
    let inputs: Vec<PathBuf> = req["inputs"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(input_path).collect())
        .transpose()?.unwrap_or_default();
    let mut options = route(cfg, api);
    if options.is_empty() {
        return Err(format!("nothing here can make pictures from a description yet. {}", advice(0.0, cfg)));
    }
    if let Some(want) = req["backend"].as_str() {
        options.retain(|o| o["backend"] == want);
        if options.is_empty() {
            return Err(format!("backend {want} isn't set up"));
        }
    }
    let ext = req["format"].as_str().unwrap_or("png");
    let out = out_path(cfg, req["name"].as_str().unwrap_or(&prompt.chars().take(40).collect::<String>().to_lowercase()), ext)?;
    let mut tried = Vec::new();
    for opt in &options {
        let backend = opt["backend"].as_str().unwrap_or("");
        let cents = opt["cents"].as_i64().unwrap_or(0);
        if backend == "paid" && cents > opt["budget_left_cents"].as_i64().unwrap_or(0) && req["approved"] != true {
            // Past the budget: don't spend without the human's OK.
            if options.len() == 1 || tried.len() + 1 == options.len() {
                return Ok(json!({"ok": false, "needs_ok": true,
                    "error": format!("this picture would cost about ${:.2} and this month's picture budget has ${:.2} left. Ask the human; if they agree, send it again with approved: true.",
                                     cents as f64 / 100.0, opt["budget_left_cents"].as_i64().unwrap_or(0).max(0) as f64 / 100.0),
                    "tried": tried}));
            }
            tried.push(json!({"backend": backend, "skipped": "over this month's budget"}));
            continue;
        }
        let job = json!({"prompt": prompt, "size": size, "inputs": inputs, "output": out});
        let child = Command::new("sh").args(["-c", opt["cmd"].as_str().unwrap_or("false")])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let Ok(mut child) = child else {
            tried.push(json!({"backend": backend, "error": "couldn't start"}));
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(job.to_string().as_bytes());
        }
        let Ok(o) = child.wait_with_output() else {
            tried.push(json!({"backend": backend, "error": "crashed"}));
            continue;
        };
        if !o.status.success() || !out.exists() {
            tried.push(json!({"backend": backend, "error": String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>()}));
            let _ = std::fs::remove_file(&out);
            continue;
        }
        let said: Value = serde_json::from_slice(&o.stdout).unwrap_or(json!({}));
        let cost = said["cost_cents"].as_i64().unwrap_or(if backend == "paid" { cents } else { 0 });
        let id = record(api, json!({"path": out, "kind": if inputs.is_empty() { "generated" } else { "changed" }, "prompt": prompt,
                                    "from": inputs, "backend": backend, "model": said["model"], "cost_cents": cost, "month": month()}));
        return Ok(json!({"ok": true, "id": id, "path": out, "backend": backend, "why": opt["why"], "cost_cents": cost,
                         "info": info(&out), "tried": tried}));
    }
    Ok(json!({"ok": false, "error": "every backend failed", "tried": tried}))
}

/// One job: {"op": "caps" | "info" | "edit" | "generate", ...}.
pub fn handle(req: &Value, api: &dyn Api, cfg: &ImageConfig) -> Value {
    let r = match req["op"].as_str().unwrap_or("") {
        "caps" => Ok(json!({"ok": true, "caps": capabilities(cfg), "routes": route(cfg, api).iter()
            .map(|o| json!({"backend": o["backend"], "why": o["why"], "cents": o["cents"], "budget_left_cents": o["budget_left_cents"]}))
            .collect::<Vec<_>>()})),
        "info" => input_path(req["input"].as_str().unwrap_or("")).map(|p| json!({"ok": true, "path": p, "info": info(&p)})),
        "edit" => edit(req, api, cfg),
        "generate" => generate(req, api, cfg),
        other => Err(format!("unknown image op {other:?}: caps, info, edit, generate")),
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

    fn setup(name: &str) -> (Arc<Core>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lx-img-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Arc::new(Core::new(Memory::in_memory(), Box::new(NoHost))), dir)
    }

    fn cfg(dir: &Path) -> ImageConfig {
        ImageConfig { dir: dir.join("images"), local_cmd: None, paid_cmd: None, paid_cents: 5, budget_cents: 1000, cpu_cmd: None }
    }

    #[test]
    fn edit_args_are_checked() {
        assert!(op_args(&json!({"resize": "1200x800"})).is_ok());
        assert!(op_args(&json!({"resize": "50%"})).is_ok());
        assert!(op_args(&json!({"crop": "100x100+5+5"})).is_ok());
        assert!(op_args(&json!({"resize": "1x1; rm -rf /"})).is_err());
        assert!(op_args(&json!({"text": "hi", "color": "red;x"})).is_err());
        assert!(op_args(&json!({"write": "/etc/passwd"})).is_err());
        let a = op_args(&json!({"text": "@/etc/passwd"})).unwrap();
        assert_eq!(a.last().unwrap(), " @/etc/passwd", "text is never read from a file");
    }

    #[test]
    fn precise_edits_run_locally_and_are_recorded() {
        if magick().is_none() {
            eprintln!("no ImageMagick; skipping");
            return;
        }
        let (core, dir) = setup("edit");
        let c = cfg(&dir);
        let src = dir.join("photo.png");
        let (bin, pre) = magick().unwrap();
        assert!(Command::new(bin).args(pre).args(["-size", "400x300", "xc:skyblue"]).arg(&src).status().unwrap().success());
        let r = handle(&json!({"op": "edit", "input": src, "ops": [{"crop": "200x150+0+0"}, {"resize": "50%"},
                       {"text": "LinuxAI", "gravity": "south", "size": 12}, {"grayscale": true}], "format": "jpg"}), &core, &c);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["info"]["width"], 100, "{r}");
        assert_eq!(r["info"]["height"], 75);
        assert!(r["path"].as_str().unwrap().ends_with("photo-edited.jpg"));
        let found = core.handle(&json!({"op": "find", "spec": {"type": "image"}}));
        assert_eq!(found["found"][0]["backend"], "imagemagick", "{found}");
        // A second edit doesn't overwrite the first.
        let r2 = handle(&json!({"op": "edit", "input": src, "ops": [{"rotate": 90}]}), &core, &c);
        assert_ne!(r2["path"], r["path"]);
        assert_eq!(r2["info"]["width"], 300);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generation_routes_falls_back_and_keeps_to_budget() {
        let (core, dir) = setup("gen");
        let mut c = cfg(&dir);
        assert!(handle(&json!({"op": "generate", "prompt": "a truck at dawn"}), &core, &c)["error"].as_str().unwrap().contains("nothing here"));

        // A local GPU model that fails, a paid service that works: it falls back and records the cost.
        c.local_cmd = Some("echo 'out of memory' >&2; exit 1".into());
        c.paid_cmd = Some(format!("cat > {0}/job.json; python3 -c \"import json,sys; j=json.load(open('{0}/job.json')); open(j['output'],'wb').write(b'PNG')\"; echo '{{\"cost_cents\": 4, \"model\": \"fake\"}}'", dir.display()));
        c.budget_cents = 10;
        let r = handle(&json!({"op": "generate", "prompt": "a truck at dawn", "size": "512x512"}), &core, &c);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["backend"], "paid");
        assert_eq!(r["tried"][0]["backend"], "local_gpu");
        assert_eq!(r["cost_cents"], 4);
        let job: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("job.json")).unwrap()).unwrap();
        assert_eq!(job["prompt"], "a truck at dawn");
        assert_eq!(job["size"], "512x512");
        assert_eq!(spent_this_month(&core), 4);

        // Past the budget, it asks first; with the human's OK it goes ahead.
        c.local_cmd = None;
        assert_eq!(handle(&json!({"op": "generate", "prompt": "two"}), &core, &c)["ok"], true); // 8 of 10 spent
        let r = handle(&json!({"op": "generate", "prompt": "three"}), &core, &c);
        assert_eq!(r["needs_ok"], true, "{r}");
        assert!(r["error"].as_str().unwrap().contains("budget"));
        assert_eq!(handle(&json!({"op": "generate", "prompt": "three", "approved": true}), &core, &c)["ok"], true);

        // With a free CPU model as well, an over-budget job goes there instead of asking.
        c.cpu_cmd = Some("python3 -c \"import json,sys; j=json.load(sys.stdin); open(j['output'],'wb').write(b'PNG')\"".into());
        let r = handle(&json!({"op": "generate", "prompt": "four"}), &core, &c);
        assert_eq!(r["backend"], "local_cpu", "{r}");
        assert_eq!(r["tried"][0]["skipped"], "over this month's budget");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
