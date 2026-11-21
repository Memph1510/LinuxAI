//! The craft organ: the ArtCraft Craft apps as tools the AI uses to finish tasks. Photo editing, RAW photos,
//! vector art, video, motion graphics, audio, PDF, documents, spreadsheets, slides, page layout, interface design,
//! CAD, texture painting and materials: each app's own engine, driven over MCP.
//!
//! `linuxai-core craft [--socket PATH]`   the organ (a user service): keeps the AI's sessions with the apps
//! `linuxai-core craft do '<json>'`        one request to it (or, for apps/install, done directly)
//!
//! How it keeps the human's work and machine safe:
//! - **Verified installs.** An app is installed the first time a task needs it, from the pinned manifest
//!   (/usr/share/linuxai/craft/apps.json, reviewed in the repository): the download must match the pinned SHA-256
//!   and carry a good GPG signature from the ArtCraft release key with the pinned fingerprint. Only the CLI is kept;
//!   it must answer MCP before it's put in place. A failed step leaves nothing behind.
//! - **Staged copies.** A session gets copies of the files it was given (`in/`) and one folder it may write
//!   (`out/`). Originals are never touched. Results the AI keeps go next to the original as `name.ai-N.ext`;
//!   replacing an original asks the human and keeps a backup.
//! - **A sandbox for every app** (bubblewrap): no network, nothing visible but the system's programs, the app and
//!   its session folders. Four apps (FilmCraft, LightCraft, GridCraft, CADCraft) have no folder limits of their
//!   own; the sandbox is what confines them.
//! - **Outside content.** What comes back from a document (its text, layer names, metadata) is information, never
//!   instructions; the AI's tool marks it as outside.
//!
//!   LINUXAI_CRAFT_MANIFEST   the manifest (default /usr/share/linuxai/craft/apps.json)
//!   LINUXAI_CRAFT_HOME       installed apps and sessions (default ~/.local/share/linuxai/craft)
//!   LINUXAI_CRAFT_SANDBOX    "off" runs apps without bubblewrap (tests only; refused otherwise if bwrap is missing)

use crate::mail::Api;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use crate::crash::Locked;

pub struct CraftConfig {
    pub manifest: PathBuf,
    pub home: PathBuf,
    /// Where results go when a session had no files of the human's to put them next to.
    pub outputs: PathBuf,
    pub sandbox: bool,
    pub download: Box<dyn Fn(&str, &Path) -> Result<(), String> + Send + Sync>,
}

impl CraftConfig {
    pub fn from_env() -> CraftConfig {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
        CraftConfig {
            manifest: std::env::var("LINUXAI_CRAFT_MANIFEST").map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/usr/share/linuxai/craft/apps.json")),
            home: std::env::var("LINUXAI_CRAFT_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local/share/linuxai/craft")),
            outputs: std::env::var("LINUXAI_CRAFT_OUT").map(PathBuf::from).unwrap_or_else(|_| home.join("craft")),
            sandbox: std::env::var("LINUXAI_CRAFT_SANDBOX").map(|v| v != "off").unwrap_or(true),
            download: Box::new(curl),
        }
    }
}

fn curl(url: &str, to: &Path) -> Result<(), String> {
    let out = Command::new("curl").args(["-fsSL", "--proto", "=https", "--max-time", "900", "--max-filesize", "600000000", "-o"])
        .arg(to).arg(url).output().map_err(|e| format!("curl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("download failed: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

fn manifest(cfg: &CraftConfig) -> Result<Value, String> {
    let text = std::fs::read_to_string(&cfg.manifest).map_err(|e| format!("{}: {e}", cfg.manifest.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("the craft manifest doesn't parse: {e}"))
}

fn app_spec(m: &Value, app: &str) -> Result<Value, String> {
    m["apps"].get(app).cloned().ok_or_else(|| format!("no craft app {app:?}: {}",
        m["apps"].as_object().map(|a| a.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()))
}

fn app_dir(cfg: &CraftConfig, app: &str, spec: &Value) -> PathBuf {
    cfg.home.join("apps").join(format!("{app}-{}", spec["version"].as_str().unwrap_or("")))
}

fn installed_cli(cfg: &CraftConfig, app: &str, spec: &Value) -> Option<PathBuf> {
    let p = app_dir(cfg, app, spec).join(spec["cli"].as_str().unwrap_or(""));
    p.is_file().then_some(p)
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The signature must be good and made by the pinned key (primary fingerprint).
fn verify_signature(dir: &Path, key: &Path, sig: &Path, file: &Path, fingerprint: &str) -> Result<(), String> {
    let gnupg = dir.join("gnupg");
    std::fs::create_dir_all(&gnupg).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&gnupg, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    let gpg = |args: &[&str]| Command::new("gpg").arg("--homedir").arg(&gnupg).args(["--batch", "--no-tty", "--status-fd", "1"]).args(args).output();
    let imp = gpg(&["--import", key.to_str().unwrap_or("")]).map_err(|e| format!("gpg isn't available: {e}"))?;
    if !imp.status.success() {
        return Err("couldn't read the release signing key".into());
    }
    let v = gpg(&["--verify", sig.to_str().unwrap_or(""), file.to_str().unwrap_or("")]).map_err(|e| e.to_string())?;
    let status = String::from_utf8_lossy(&v.stdout);
    let want = fingerprint.to_uppercase();
    // [GNUPG:] VALIDSIG <signing-key-fpr> <date> ... <primary-key-fpr>
    let good = status.lines().any(|l| l.starts_with("[GNUPG:] VALIDSIG ") && l.split_whitespace().last().map(|p| p.eq_ignore_ascii_case(&want)).unwrap_or(false));
    if good { Ok(()) } else { Err(format!("the signature isn't a good one from the ArtCraft release key {want}")) }
}

/// Download, check, extract only the CLI, probe it, then put it in place. Nothing is left behind on failure.
pub fn install(cfg: &CraftConfig, api: Option<&dyn Api>, app: &str) -> Result<Value, String> {
    let m = manifest(cfg)?;
    let spec = app_spec(&m, app)?;
    if let Some(cli) = installed_cli(cfg, app, &spec) {
        return Ok(json!({"ok": true, "app": app, "version": spec["version"], "cli": cli, "already": true}));
    }
    let (ver, asset, sha, cli) = (spec["version"].as_str().unwrap_or(""), spec["asset"].as_str().unwrap_or(""),
                                  spec["sha256"].as_str().unwrap_or(""), spec["cli"].as_str().unwrap_or(""));
    if [ver, asset, sha, cli].iter().any(|s| s.is_empty() || s.contains('/') || s.contains("..")) {
        return Err(format!("the manifest entry for {app} is incomplete"));
    }
    let url = |a: &str| m["release_url"].as_str().unwrap_or("").replace("{app}", app).replace("{version}", ver).replace("{asset}", a);
    let stage = cfg.home.join("staging").join(format!("{app}-{ver}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let r = (|| -> Result<Value, String> {
        let (tarball, sig, key) = (stage.join(asset), stage.join(format!("{asset}.asc")), stage.join("release-signing-key.asc"));
        (cfg.download)(&url(asset), &tarball)?;
        let got = sha256_file(&tarball)?;
        if got != sha {
            return Err(format!("{asset} doesn't match its pinned hash (got {got})"));
        }
        (cfg.download)(&url(&format!("{asset}.asc")), &sig)?;
        (cfg.download)(&url(m["signing_key_asset"].as_str().unwrap_or("release-signing-key.asc")), &key)?;
        verify_signature(&stage, &key, &sig, &tarball, m["signing_key_fingerprint"].as_str().unwrap_or(""))?;
        let x = Command::new("tar").arg("-xzf").arg(&tarball).arg("-C").arg(&stage).arg("--wildcards").arg(format!("*/{cli}")).arg(cli)
            .output().map_err(|e| e.to_string())?;
        let found = walk_find(&stage, cli).ok_or_else(|| format!("{asset} has no {cli} in it ({})", String::from_utf8_lossy(&x.stderr).trim()))?;
        let dest = app_dir(cfg, app, &spec);
        let tmp_dest = stage.join("app");
        std::fs::create_dir_all(&tmp_dest).map_err(|e| e.to_string())?;
        std::fs::rename(&found, tmp_dest.join(cli)).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(tmp_dest.join(cli), std::os::unix::fs::PermissionsExt::from_mode(0o755));
        // Probe: it must start and list its tools over MCP, in the sandbox, before it's put in place.
        let probe_dir = stage.join("probe");
        let tools = {
            let mut s = Session::start(cfg, app, &spec, &tmp_dest.join(cli), &probe_dir, &[])?;
            let t = s.tools()?;
            s.stop();
            t
        };
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp_dest, &dest).map_err(|e| e.to_string())?;
        let size = std::fs::metadata(dest.join(cli)).map(|m| m.len()).unwrap_or(0);
        if let Some(api) = api {
            api.call("event", json!({"kind": "craft.installed", "data": {"app": app, "version": ver, "sha256": sha, "tools": tools.len()}}));
        }
        Ok(json!({"ok": true, "app": app, "version": ver, "cli": dest.join(cli), "mb": size / 1_000_000, "tools": tools.len(),
                  "verified": ["pinned SHA-256", "GPG signature from the ArtCraft release key (pinned fingerprint)", "answers MCP in the sandbox"]}))
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

/// The CLI in an extracted release: a Linux executable (ELF) with that name. Release trees also carry shell
/// completions with the same name, which aren't it.
fn walk_find(dir: &Path, name: &str) -> Option<PathBuf> {
    let is_elf = |p: &Path| std::fs::File::open(p).ok().and_then(|mut f| {
        let mut m = [0u8; 4];
        // (Tests use a script as their stand-in app.)
        f.read_exact(&mut m).ok().map(|_| &m == b"\x7fELF" || (cfg!(test) && m.starts_with(b"#!")))
    }).unwrap_or(false);
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() && !p.ends_with("gnupg") {
            if let Some(f) = walk_find(&p, name) {
                return Some(f);
            }
        } else if p.file_name().map(|n| n == name).unwrap_or(false) && p.is_file() && is_elf(&p) {
            return Some(p);
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------
// A session: the app's MCP server in its sandbox, with its in/ and out/ folders
// ------------------------------------------------------------------------------------------------

pub struct Session {
    pub app: String,
    pub dir: PathBuf,
    pub originals: Vec<PathBuf>,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
    pub last_used: Instant,
}

fn sandboxed(cfg: &CraftConfig, cli: &Path, dir: &Path, args: &[String]) -> Result<Command, String> {
    let (inside_in, inside_out, inside_work) = ("/work/in", "/work/out", "/work");
    let args: Vec<String> = args.iter().map(|a| a.replace("{in}", inside_in).replace("{out}", inside_out).replace("{work}", inside_work)).collect();
    if !cfg.sandbox {
        let local: Vec<String> = args.iter().map(|a| a.replace(inside_in, &dir.join("in").to_string_lossy())
            .replace(inside_out, &dir.join("out").to_string_lossy()).replace(inside_work, &dir.to_string_lossy())).collect();
        let mut c = Command::new(cli);
        c.args(local).current_dir(dir).env("HOME", dir.join("home"));
        return Ok(c);
    }
    if !Path::new("/usr/bin/bwrap").exists() {
        return Err("the sandbox (bubblewrap) isn't installed; craft apps don't run without it".into());
    }
    let mut c = Command::new("/usr/bin/bwrap");
    // Not --die-with-parent: that follows the *thread* that started the app, and each request has its own. When
    // the organ goes, the app's stdin closes and the MCP server ends with it.
    c.args(["--unshare-all", "--new-session", "--ro-bind", "/usr", "/usr",
            "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib", "/lib64", "--symlink", "usr/bin", "/bin",
            "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--dir", "/work", "--dir", "/home/craft"]);
    for etc in ["/etc/fonts", "/etc/ld.so.cache", "/etc/localtime", "/etc/passwd"] {
        if Path::new(etc).exists() {
            c.args(["--ro-bind", etc, etc]);
        }
    }
    c.arg("--ro-bind").arg(cli).arg("/app/craft-cli")
        .arg("--ro-bind").arg(dir.join("in")).arg(inside_in)
        .arg("--bind").arg(dir.join("out")).arg(inside_out)
        .args(["--setenv", "HOME", "/home/craft", "--chdir", "/work", "/app/craft-cli"]).args(args);
    Ok(c)
}

impl Session {
    /// Start the app on copies of `files` (they go to in/). Nothing else of the human's is visible to it.
    pub fn start(cfg: &CraftConfig, app: &str, spec: &Value, cli: &Path, dir: &Path, files: &[PathBuf]) -> Result<Session, String> {
        for d in ["in", "out", "home"] {
            std::fs::create_dir_all(dir.join(d)).map_err(|e| e.to_string())?;
        }
        let mut originals = Vec::new();
        for f in files {
            let f = std::fs::canonicalize(f).map_err(|e| format!("{}: {e}", f.display()))?;
            if !f.is_file() {
                return Err(format!("{} isn't a file", f.display()));
            }
            let name = f.file_name().unwrap_or_default();
            if dir.join("in").join(name).exists() {
                return Err(format!("two files named {}: give them one at a time", name.to_string_lossy()));
            }
            std::fs::copy(&f, dir.join("in").join(name)).map_err(|e| format!("copying {}: {e}", f.display()))?;
            originals.push(f);
        }
        let mut args = vec!["mcp".to_string()];
        match spec["roots"].as_str().unwrap_or("automation") {
            // One read root (some apps take the flag only once): the session folder, holding in/ (copies) and out/.
            "automation" => args.extend(["--automation-read-root", "{work}", "--automation-write-root", "{out}"].map(String::from)),
            "material" => args.extend(["--read-root", "{in}", "--write-root", "{out}"].map(String::from)),
            "pdf" => args.extend(["--automation-read-root", "{in}", "--automation-write-root", "{out}", "--root", "{work}"].map(String::from)),
            _ => {} // no folder limits of its own: the sandbox is the limit
        }
        args.extend(spec["args"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from));
        let log = std::fs::File::create(dir.join("app.log")).map_err(|e| e.to_string())?;
        let mut child = sandboxed(cfg, cli, dir, &args)?.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(log)
            .spawn().map_err(|e| format!("couldn't start {app}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut s = Session { app: app.into(), dir: dir.into(), originals, child, stdin, lines: rx, next_id: 1, last_used: Instant::now() };
        s.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {},
                                       "clientInfo": {"name": "linuxai", "version": env!("CARGO_PKG_VERSION")}}), Duration::from_secs(120))
            .map_err(|e| {
                s.stop();
                let log = std::fs::read_to_string(s.dir.join("app.log")).unwrap_or_default();
                let tail: Vec<&str> = log.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
                format!("{app} didn't start its MCP server: {e}{}", if tail.is_empty() { String::new() } else { format!(" ({})", tail.join(" | ")) })
            })?;
        s.notify("notifications/initialized");
        Ok(s)
    }

    fn notify(&mut self, method: &str) {
        let _ = writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "method": method}));
        let _ = self.stdin.flush();
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .and_then(|_| self.stdin.flush()).map_err(|e| format!("the app stopped: {e}"))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if v["id"] != json!(id) {
                        continue; // notifications, logs, progress
                    }
                    self.last_used = Instant::now();
                    if let Some(e) = v.get("error") {
                        return Err(e["message"].as_str().unwrap_or("error").to_string());
                    }
                    return Ok(v["result"].clone());
                }
                Err(RecvTimeoutError::Timeout) => return Err(format!("no answer in {}s", timeout.as_secs())),
                Err(RecvTimeoutError::Disconnected) => return Err("the app stopped".into()),
            }
        }
    }

    pub fn tools(&mut self) -> Result<Vec<Value>, String> {
        let mut all = Vec::new();
        let mut cursor = Value::Null;
        for _ in 0..20 {
            let r = self.request("tools/list", if cursor.is_null() { json!({}) } else { json!({"cursor": cursor}) }, Duration::from_secs(60))?;
            all.extend(r["tools"].as_array().cloned().unwrap_or_default());
            cursor = r["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        Ok(all)
    }

    pub fn call(&mut self, tool: &str, args: Value, timeout: Duration) -> Result<Value, String> {
        self.request("tools/call", json!({"name": tool, "arguments": if args.is_object() { args } else { json!({}) }}), timeout)
    }

    pub fn outputs(&self) -> Vec<Value> {
        let mut v: Vec<Value> = std::fs::read_dir(self.dir.join("out")).into_iter().flatten().flatten()
            .filter(|e| e.path().is_file())
            .map(|e| json!({"name": e.file_name().to_string_lossy(), "bytes": e.metadata().map(|m| m.len()).unwrap_or(0)}))
            .collect();
        v.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        v
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `report.pdf` next to `contract.pdf` → `contract.ai-N.pdf` (the next free N), keeping the output's extension.
fn staged_next_to(original: &Path, output_name: &str) -> PathBuf {
    let stem = original.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let base = match stem.rsplit_once(".ai-") {
        Some((b, n)) if n.parse::<u32>().is_ok() => b.to_string(),
        _ => stem,
    };
    let ext = Path::new(output_name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = original.parent().unwrap_or(Path::new("."));
    (1..).map(|n| dir.join(format!("{base}.ai-{n}{ext}"))).find(|p| !p.exists()).unwrap()
}

// ------------------------------------------------------------------------------------------------
// The organ
// ------------------------------------------------------------------------------------------------

pub struct Organ {
    pub cfg: CraftConfig,
    pub api: Option<Box<dyn Api + Send + Sync>>,
    sessions: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
    seq: Mutex<u64>,
}

impl Organ {
    pub fn new(cfg: CraftConfig, api: Option<Box<dyn Api + Send + Sync>>) -> Organ {
        Organ { cfg, api, sessions: Mutex::new(HashMap::new()), seq: Mutex::new(0) }
    }

    fn api(&self) -> Option<&dyn Api> {
        self.api.as_deref().map(|a| a as &dyn Api)
    }

    fn session(&self, id: &str) -> Result<Arc<Mutex<Session>>, String> {
        self.sessions.lk().get(id).cloned().ok_or_else(|| format!("no open session {id:?} (they close after 20 idle minutes)"))
    }

    pub fn apps(&self) -> Result<Value, String> {
        let m = manifest(&self.cfg)?;
        let apps: Vec<Value> = m["apps"].as_object().into_iter().flatten().map(|(name, s)| json!({
            "app": name, "what": s["what"], "version": s["version"], "installed": installed_cli(&self.cfg, name, s).is_some(),
            "confined_by": if s["roots"] == "none" { "the sandbox only" } else { "its own folder limits and the sandbox" }})).collect();
        let open: Vec<Value> = self.sessions.lk().iter().map(|(id, s)| {
            let s = s.lk();
            json!({"session": id, "app": s.app, "files": s.originals})
        }).collect();
        Ok(json!({"ok": true, "apps": apps, "open": open}))
    }

    pub fn handle(&self, req: &Value) -> Value {
        let r = (|| -> Result<Value, String> {
            match req["op"].as_str().unwrap_or("") {
                "apps" => self.apps(),
                "install" => install(&self.cfg, self.api(), req["app"].as_str().unwrap_or("")),
                "open" => {
                    let app = req["app"].as_str().unwrap_or("");
                    let m = manifest(&self.cfg)?;
                    let spec = app_spec(&m, app)?;
                    let cli = match installed_cli(&self.cfg, app, &spec) {
                        Some(c) => c,
                        None => {
                            install(&self.cfg, self.api(), app)?;
                            installed_cli(&self.cfg, app, &spec).ok_or("installed, but the CLI isn't there")?
                        }
                    };
                    let files: Vec<PathBuf> = req["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PathBuf::from).collect();
                    let id = {
                        let mut n = self.seq.lk();
                        *n += 1;
                        format!("{app}-{}-{}", std::process::id(), *n)
                    };
                    let dir = self.cfg.home.join("sessions").join(&id);
                    let mut s = Session::start(&self.cfg, app, &spec, &cli, &dir, &files)?;
                    let tools = s.tools()?;
                    let names: Vec<Value> = tools.iter().map(|t| json!({"name": t["name"], "about": t["description"].as_str().map(|d| d.chars().take(160).collect::<String>())})).collect();
                    // The paths the app sees (the same for every app): copies of the files under /work/in, and
                    // /work/out, the only place it can write.
                    let (inp, out) = if self.cfg.sandbox { ("/work/in".to_string(), "/work/out".to_string()) }
                        else { (dir.join("in").to_string_lossy().to_string(), dir.join("out").to_string_lossy().to_string()) };
                    let inputs: Vec<String> = s.originals.iter().map(|f| format!("{inp}/{}", f.file_name().unwrap_or_default().to_string_lossy())).collect();
                    self.sessions.lk().insert(id.clone(), Arc::new(Mutex::new(s)));
                    Ok(json!({"ok": true, "session": id, "app": app, "inputs": inputs, "write_to": out,
                              "note": "the inputs are copies of the human's files; save results under write_to, then keep the ones that matter",
                              "tools": names}))
                }
                "tool_info" => {
                    let s = self.session(req["session"].as_str().unwrap_or(""))?;
                    let mut s = s.lk();
                    let want = req["tool"].as_str().unwrap_or("");
                    let t = s.tools()?.into_iter().find(|t| t["name"] == want).ok_or(format!("no tool {want:?}"))?;
                    Ok(json!({"ok": true, "tool": t}))
                }
                "call" => {
                    let s = self.session(req["session"].as_str().unwrap_or(""))?;
                    let mut s = s.lk();
                    let timeout = Duration::from_secs(req["timeout"].as_u64().unwrap_or(600).min(3600));
                    let r = s.call(req["tool"].as_str().unwrap_or(""), req["args"].clone(), timeout)?;
                    Ok(json!({"ok": r["isError"] != true, "result": r, "outputs": s.outputs()}))
                }
                "keep" => {
                    // A result the AI keeps: next to the original it came from, as name.ai-N.ext.
                    let s = self.session(req["session"].as_str().unwrap_or(""))?;
                    let s = s.lk();
                    let name = req["output"].as_str().unwrap_or("");
                    if name.is_empty() || name.contains('/') || name.starts_with('.') {
                        return Err("output: a file name from out/".into());
                    }
                    let from = s.dir.join("out").join(name);
                    let meta = std::fs::metadata(&from).map_err(|_| format!("no {name} in out/: {:?}", s.outputs()))?;
                    if meta.len() == 0 {
                        return Err(format!("{name} is empty"));
                    }
                    let dest = match req["next_to"].as_str() {
                        Some(o) => staged_next_to(&s.originals.iter().find(|f| f.file_name().map(|n| n == o).unwrap_or(false) || f.to_string_lossy() == o)
                            .cloned().ok_or(format!("{o} isn't one of this session's files"))?, name),
                        None => match s.originals.as_slice() {
                            [one] => staged_next_to(one, name),
                            [] => {
                                let d = self.cfg.outputs.join(&s.app);
                                std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
                                if d.join(name).exists() { staged_next_to(&d.join(name), name) } else { d.join(name) }
                            }
                            _ => return Err("this session has several files: say which one this belongs next to (next_to)".into()),
                        },
                    };
                    std::fs::copy(&from, &dest).map_err(|e| e.to_string())?;
                    let same = sha256_file(&from)? == sha256_file(&dest)?;
                    if !same {
                        let _ = std::fs::remove_file(&dest);
                        return Err("the copy didn't come out the same; nothing kept".into());
                    }
                    if let Some(api) = self.api() {
                        api.call("object_put", json!({"type": "creation", "fields": {"kind": "craft", "app": s.app, "path": dest,
                            "from": s.originals, "output": name, "bytes": meta.len()}}));
                    }
                    Ok(json!({"ok": true, "kept": dest, "bytes": meta.len(),
                              "checked": ["the file is there, not empty, and identical to the app's output"],
                              "next": "originals are untouched; to put this in place of one, use replace (asks the human)"}))
                }
                "replace" => {
                    let staged = std::fs::canonicalize(req["staged"].as_str().unwrap_or("")).map_err(|e| format!("staged: {e}"))?;
                    let stem = staged.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    let (base, n) = stem.rsplit_once(".ai-").ok_or("that isn't a kept result (name.ai-N.ext)")?;
                    n.parse::<u32>().map_err(|_| "that isn't a kept result (name.ai-N.ext)")?;
                    let ext = staged.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    let original = staged.with_file_name(format!("{base}{ext}"));
                    if !original.is_file() {
                        return Err(format!("{} isn't there to replace (results in a different format are kept beside it)", original.display()));
                    }
                    if req["approved"] != true {
                        return Ok(json!({"ok": false, "needs_ok": true, "error": format!(
                            "This replaces {} with {} (the original is kept as a backup). Ask the human, then send approved: true.",
                            original.display(), staged.display())}));
                    }
                    let backup = original.with_file_name(format!("{base}.before-ai-{}{ext}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
                    std::fs::rename(&original, &backup).map_err(|e| e.to_string())?;
                    if let Err(e) = std::fs::copy(&staged, &original) {
                        let _ = std::fs::rename(&backup, &original);
                        return Err(format!("couldn't put it in place (the original is back): {e}"));
                    }
                    Ok(json!({"ok": true, "replaced": original, "backup": backup}))
                }
                "close" => {
                    let id = req["session"].as_str().unwrap_or("");
                    let s = self.sessions.lk().remove(id).ok_or(format!("no open session {id:?}"))?;
                    let dir = s.lk().dir.clone();
                    drop(s);
                    let _ = std::fs::remove_dir_all(&dir);
                    Ok(json!({"ok": true, "closed": id}))
                }
                other => Err(format!("unknown craft op {other:?}: apps, install, open, tool_info, call, keep, replace, close")),
            }
        })();
        r.unwrap_or_else(|e| json!({"ok": false, "error": e}))
    }

    /// Sessions nobody used for 20 minutes are closed (their copies and outputs go with them).
    pub fn sweep(&self) {
        let idle: Vec<String> = self.sessions.lk().iter()
            .filter(|(_, s)| s.try_lock().map(|s| s.last_used.elapsed() > Duration::from_secs(1200)).unwrap_or(false))
            .map(|(k, _)| k.clone()).collect();
        for id in idle {
            self.handle(&json!({"op": "close", "session": id}));
        }
    }
}

pub fn default_socket() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir()).join("linuxai-craft.sock")
}

fn serve(organ: Arc<Organ>, stream: UnixStream) {
    let Ok(mut w) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let resp = serde_json::from_str::<Value>(&line).map(|r| organ.handle(&r)).unwrap_or_else(|e| json!({"ok": false, "error": format!("bad json: {e}")}));
        if writeln!(w, "{resp}").is_err() {
            return;
        }
    }
}

pub fn run(socket: PathBuf, api: Option<Box<dyn Api + Send + Sync>>) -> std::io::Result<()> {
    let organ = Arc::new(Organ::new(CraftConfig::from_env(), api));
    let _ = std::fs::remove_dir_all(organ.cfg.home.join("sessions")); // left by an earlier run
    let o = organ.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(60));
        o.sweep();
    });
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    let _ = std::fs::set_permissions(&socket, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    eprintln!("linuxai craft: on {}", socket.display());
    for s in listener.incoming().flatten() {
        let o = organ.clone();
        std::thread::spawn(move || serve(o, s));
    }
    Ok(())
}

/// One request to the running organ.
pub fn request(socket: &Path, req: &Value) -> Value {
    let r = (|| -> std::io::Result<Value> {
        let mut s = UnixStream::connect(socket)?;
        writeln!(s, "{req}")?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line)?;
        serde_json::from_str(&line).map_err(std::io::Error::other)
    })();
    r.unwrap_or_else(|e| json!({"ok": false, "error": format!("the craft organ isn't answering ({e}); is linuxai-craft running?")}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in Craft app: an MCP server on stdio with two tools, writing only under its write root.
    const FAKE_APP: &str = r#"#!/usr/bin/env python3
import json, sys, os
a = sys.argv
wr = a[a.index("--automation-write-root") + 1] if "--automation-write-root" in a else None
rd = [a[i + 1] for i, x in enumerate(a) if x == "--automation-read-root"]
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    r = None
    if m["method"] == "initialize": r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "fakecraft"}}
    elif m["method"] == "tools/list": r = {"tools": [{"name": "doc_inspect", "description": "what is open", "inputSchema": {"type": "object"}},
                                                     {"name": "doc_export", "description": "write a file", "inputSchema": {"type": "object"}}]}
    elif m["method"] == "tools/call":
        args = m["params"]["arguments"]
        if m["params"]["name"] == "doc_inspect":
            r = {"content": [{"type": "text", "text": json.dumps({"inputs": sorted(os.listdir(os.path.join(rd[0], "in"))), "write_root": wr})}]}
        else:
            src = open(os.path.join(rd[0], "in", args["from"])).read()
            open(os.path.join(wr, args["to"]), "w").write(src.upper())
            r = {"content": [{"type": "text", "text": "exported " + args["to"]}]}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

    fn gpg(home: &Path, args: &[&str]) -> std::process::Output {
        Command::new("gpg").arg("--homedir").arg(home).args(["--batch", "--no-tty", "--pinentry-mode", "loopback", "--passphrase", ""]).args(args).output().unwrap()
    }

    /// A release like ArtCraft's: a tarball with the CLI, its .asc, and the signing key; a manifest pinning both.
    fn release(dir: &Path) -> (PathBuf, String) {
        let rel = dir.join("release");
        let pkg = rel.join("fakecraft-1.0.0-linux-x86_64");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("fakecraft-cli"), FAKE_APP).unwrap();
        std::fs::set_permissions(pkg.join("fakecraft-cli"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::write(pkg.join("README.md"), "docs").unwrap();
        assert!(Command::new("tar").args(["-czf", "fakecraft-1.0.0-linux-x86_64.tar.gz", "fakecraft-1.0.0-linux-x86_64"]).current_dir(&rel).status().unwrap().success());
        let keys = dir.join("signer");
        std::fs::create_dir_all(&keys).unwrap();
        let _ = std::fs::set_permissions(&keys, std::os::unix::fs::PermissionsExt::from_mode(0o700));
        assert!(gpg(&keys, &["--quick-gen-key", "Test Release Key <release@example.com>", "ed25519", "sign", "never"]).status.success());
        let fpr = String::from_utf8_lossy(&gpg(&keys, &["--with-colons", "--fingerprint"]).stdout).lines()
            .find(|l| l.starts_with("fpr:")).unwrap().split(':').nth(9).unwrap().to_string();
        let tar = rel.join("fakecraft-1.0.0-linux-x86_64.tar.gz");
        assert!(gpg(&keys, &["--armor", "--detach-sign", "--output", rel.join("fakecraft-1.0.0-linux-x86_64.tar.gz.asc").to_str().unwrap(), tar.to_str().unwrap()]).status.success());
        std::fs::write(rel.join("release-signing-key.asc"), gpg(&keys, &["--armor", "--export"]).stdout).unwrap();
        let manifest = json!({"release_url": format!("file://{}/{{asset}}", rel.display()), "signing_key_fingerprint": fpr,
            "signing_key_asset": "release-signing-key.asc", "apps": {"fakecraft": {"version": "1.0.0", "asset": "fakecraft-1.0.0-linux-x86_64.tar.gz",
            "sha256": sha256_file(&tar).unwrap(), "cli": "fakecraft-cli", "roots": "automation", "args": [], "smoke": "doc_inspect", "what": "a test"}}});
        let mpath = dir.join("apps.json");
        std::fs::write(&mpath, manifest.to_string()).unwrap();
        (mpath, fpr)
    }

    fn cfg(dir: &Path, manifest: PathBuf) -> CraftConfig {
        CraftConfig { manifest, home: dir.join("craft"), outputs: dir.join("out"), sandbox: false,
                      download: Box::new(|url: &str, to: &Path| std::fs::copy(url.trim_start_matches("file://"), to).map(|_| ()).map_err(|e| e.to_string())) }
    }

    #[test]
    fn installs_only_what_is_pinned_and_signed_then_works_on_copies() {
        if Command::new("gpg").arg("--version").output().is_err() || Command::new("python3").arg("--version").output().is_err() {
            eprintln!("no gpg/python3; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("lx-craft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mpath, _fpr) = release(&dir);
        let organ = Organ::new(cfg(&dir, mpath.clone()), None);

        // A tampered download is refused, and nothing is left installed.
        let tar = dir.join("release/fakecraft-1.0.0-linux-x86_64.tar.gz");
        let good = std::fs::read(&tar).unwrap();
        let mut bad = good.clone();
        bad.extend_from_slice(b"x");
        std::fs::write(&tar, &bad).unwrap();
        let r = organ.handle(&json!({"op": "install", "app": "fakecraft"}));
        assert!(r["error"].as_str().unwrap().contains("pinned hash"), "{r}");
        std::fs::write(&tar, &good).unwrap();
        assert!(!dir.join("craft/apps/fakecraft-1.0.0").exists());

        // Signed by some other key: refused, even though the hash is right.
        let pinned = std::fs::read_to_string(&mpath).unwrap();
        std::fs::write(&mpath, pinned.replace(&_fpr, "0000000000000000000000000000000000000000")).unwrap();
        let r = organ.handle(&json!({"op": "install", "app": "fakecraft"}));
        assert!(r["error"].as_str().unwrap().contains("signature"), "{r}");
        std::fs::write(&mpath, &pinned).unwrap();

        // Pinned and signed: installed (only the CLI), after it answered MCP.
        let r = organ.handle(&json!({"op": "install", "app": "fakecraft"}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["tools"], 2);
        assert!(!dir.join("craft/apps/fakecraft-1.0.0/README.md").exists(), "only the CLI is kept");
        assert_eq!(organ.handle(&json!({"op": "apps"}))["apps"][0]["installed"], true);

        // A session works on copies; it writes only to out/; a result is kept next to the original.
        let doc = dir.join("letter.txt");
        std::fs::write(&doc, "dear dana").unwrap();
        let r = organ.handle(&json!({"op": "open", "app": "fakecraft", "files": [doc]}));
        assert_eq!(r["ok"], true, "{r}");
        let sid = r["session"].as_str().unwrap().to_string();
        let r = organ.handle(&json!({"op": "call", "session": sid, "tool": "doc_inspect", "args": {}}));
        let seen: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(seen["inputs"], json!(["letter.txt"]), "{r}");
        assert!(seen["write_root"].as_str().unwrap().ends_with("/out"));
        let r = organ.handle(&json!({"op": "call", "session": sid, "tool": "doc_export", "args": {"from": "letter.txt", "to": "letter.txt"}}));
        assert_eq!(r["outputs"][0]["name"], "letter.txt", "{r}");
        let r = organ.handle(&json!({"op": "keep", "session": sid, "output": "letter.txt"}));
        assert_eq!(r["ok"], true, "{r}");
        let kept = PathBuf::from(r["kept"].as_str().unwrap());
        assert_eq!(kept, dir.join("letter.ai-1.txt"));
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "DEAR DANA");
        assert_eq!(std::fs::read_to_string(&doc).unwrap(), "dear dana", "the original is untouched");
        assert!(organ.handle(&json!({"op": "keep", "session": sid, "output": "../../etc/passwd"}))["error"].is_string());

        // Replacing the original asks first, and keeps it.
        assert_eq!(organ.handle(&json!({"op": "replace", "staged": kept}))["needs_ok"], true);
        let r = organ.handle(&json!({"op": "replace", "staged": kept, "approved": true}));
        assert_eq!(std::fs::read_to_string(&doc).unwrap(), "DEAR DANA", "{r}");
        assert_eq!(std::fs::read_to_string(r["backup"].as_str().unwrap()).unwrap(), "dear dana");

        // Closing removes the session's copies and outputs.
        let sdir = dir.join("craft/sessions").join(&sid);
        assert!(sdir.exists());
        assert_eq!(organ.handle(&json!({"op": "close", "session": sid}))["ok"], true);
        assert!(!sdir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kept_results_take_the_next_free_name_beside_the_original() {
        let dir = std::env::temp_dir().join(format!("lx-craft-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let o = dir.join("contract.pdf");
        assert_eq!(staged_next_to(&o, "out.pdf"), dir.join("contract.ai-1.pdf"));
        std::fs::write(dir.join("contract.ai-1.pdf"), b"x").unwrap();
        assert_eq!(staged_next_to(&o, "page1.png"), dir.join("contract.ai-1.png"), "a different format keeps its extension");
        assert_eq!(staged_next_to(&o, "out.pdf"), dir.join("contract.ai-2.pdf"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
