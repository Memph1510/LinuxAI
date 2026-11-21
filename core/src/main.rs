//! linuxai-core: the LinuxAI system service.
//!
//!   linuxai-core daemon [--socket PATH] [--db PATH] [--group NAME]   run the core
//!   linuxai-core ctl '<json request>'                                talk to a running core
//!   linuxai-core follow                                              every event as it happens, one per line
//!   linuxai-core check PLAN.json                                     check a plan without running it
//!   linuxai-core snapshot                                            print what the host senses right now
//!   linuxai-core verbs                                               list verbs
//!   linuxai-core reach                                               message the human when they're needed
//!   linuxai-core mail ingest FILE.eml... | mail watch                email in, claims out (never instructions)
//!   linuxai-core browser [--headless] [--profile DIR]                 the browser the AI and the human share
//!   linuxai-core image caps | image do '<json>'                       make and edit pictures
//!   linuxai-core voice caps | listen | transcribe FILE | say          talk to it, hear it answer
//!   linuxai-core compositor                                          the screen, through Hyprland
//!   linuxai-core calendar import FILE.ics | calendar watch           subscribed calendars and invitations in

mod browser;
mod agreements;
mod blender;
mod calendar;
mod client;
mod compositor;
mod craft;
mod core;
mod crash;
mod engine;
mod host;
mod jobs;
mod images;
mod mail;
mod memory;
mod models;
mod playbooks;
mod prompts;
mod reach;
mod send;
mod tasks;
mod verbs;
mod voice;
mod watch;
mod workspace;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use crate::crash::Locked;

pub(crate) fn default_socket() -> PathBuf {
    if let Ok(p) = std::env::var("LINUXAI_CORE_SOCKET") {
        return p.into();
    }
    // The system core (on the OS) listens here; anyone in the linuxai group may use it.
    if is_root() || std::path::Path::new("/run/linuxai/core.sock").exists() {
        "/run/linuxai/core.sock".into()
    } else {
        let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(dir).join("linuxai-core.sock")
    }
}

fn default_db() -> PathBuf {
    if let Ok(p) = std::env::var("LINUXAI_CORE_DB") {
        return p.into();
    }
    if is_root() {
        "/var/lib/linuxai/core.db".into()
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".local/share/linuxai/core.db")
    }
}

fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(1) == Some("0")))
        .unwrap_or(false)
}

fn group_id(name: &str) -> Option<u32> {
    std::fs::read_to_string("/etc/group").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.first() == Some(&name)).then(|| f.get(2)?.parse().ok()).flatten()
    })
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn serve(core: Arc<core::Core>, stream: UnixStream) {
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(writer, "{}", json!({"ok": false, "error": format!("bad json: {e}")}));
                continue;
            }
        };
        if req["op"] == "follow" {
            // Stream every event from now on, one JSON object per line.
            let (tx, rx) = mpsc::channel();
            core.subscribers.lk().push(tx);
            let _ = writeln!(writer, "{}", json!({"ok": true, "following": true}));
            for ev in rx {
                if writeln!(writer, "{ev}").is_err() {
                    return;
                }
            }
            return;
        }
        // One request hitting a bug fails alone: the caller gets an error, the human is told, the core keeps serving.
        let resp = match crash::catch(|| core.handle(&req)) {
            Ok(r) => r,
            Err(msg) => {
                let op = req["op"].as_str().unwrap_or("?").to_string();
                core.emit("linuxai.crash", crash::event("core", &format!("request '{op}'"), &msg));
                json!({"ok": false, "error": format!("LinuxAI hit an internal error handling '{op}' (recorded): {msg}")})
            }
        };
        if writeln!(writer, "{resp}").is_err() {
            return;
        }
    }
}

fn daemon(args: &[String]) -> std::io::Result<()> {
    let socket = arg(args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket);
    let db = arg(args, "--db").map(PathBuf::from).unwrap_or_else(default_db);
    let interval: f64 = arg(args, "--interval").and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let mem = memory::Memory::open(&db).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut core = core::Core::new(mem, Box::new(host::LinuxHost));
    core.background = true;
    core.playbook_dir = arg(args, "--playbooks").map(PathBuf::from)
        .or_else(|| std::env::var("LINUXAI_PLAYBOOKS").ok().map(PathBuf::from))
        .unwrap_or_else(|| db.parent().map(|d| d.join("playbooks")).unwrap_or_else(|| "playbooks".into()));
    core.prompt_dir = arg(args, "--prompts").map(PathBuf::from)
        .or_else(|| std::env::var("LINUXAI_PROMPTS").ok().map(PathBuf::from))
        .unwrap_or_else(|| db.parent().map(|d| d.join("prompts")).unwrap_or_else(|| "prompts".into()));
    core.file_gid = arg(args, "--group").or_else(|| std::env::var("LINUXAI_GROUP").ok()).as_deref().and_then(group_id);
    let core = Arc::new(core);
    core.recover();
    core.notice_unexpected_restarts();

    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let group = arg(args, "--group").or_else(|| std::env::var("LINUXAI_GROUP").ok());
        if let Some(gid) = group.as_deref().and_then(group_id) {
            let _ = std::os::unix::fs::chown(&socket, None, Some(gid));
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660))?;
        } else {
            std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    eprintln!("linuxai-core {} listening on {} (memory: {})", env!("CARGO_PKG_VERSION"), socket.display(), db.display());

    // Senses + timers.
    let c = core.clone();
    std::thread::spawn(move || {
        let mut n: u64 = 0;
        loop {
            c.sense(n % 5 == 0);
            c.tick();
            // Prune old history at startup and then about once an hour.
            let every = std::env::var("LINUXAI_PRUNE_EVERY_SECS").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(3600.0);
            if n % ((every / interval).max(1.0) as u64) == 0 {
                c.handle(&serde_json::json!({"op": "prune"}));
            }
            n += 1;
            std::thread::sleep(Duration::from_secs_f64(interval));
        }
    });

    sd_notify_ready();
    for stream in listener.incoming().flatten() {
        let c = core.clone();
        std::thread::spawn(move || serve(c, stream));
    }
    Ok(())
}

/// Tell systemd we're up (Type=notify), without a libsystemd dependency.
fn sd_notify_ready() {
    if let Ok(path) = std::env::var("NOTIFY_SOCKET") {
        if let Ok(sock) = std::os::unix::net::UnixDatagram::unbound() {
            let addr = if let Some(abs) = path.strip_prefix('@') {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(abs.as_bytes()).ok()
            } else {
                std::os::unix::net::SocketAddr::from_pathname(&path).ok()
            };
            if let Some(a) = addr {
                let _ = sock.send_to_addr(b"READY=1", &a);
            }
        }
    }
}

fn ctl(args: &[String]) -> std::io::Result<()> {
    let socket = arg(args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket);
    let req = args.iter().skip(2).find(|a| a.starts_with('{')).cloned().unwrap_or_else(|| "{\"op\":\"ping\"}".into());
    let mut s = UnixStream::connect(&socket)?;
    writeln!(s, "{req}")?;
    let mut reader = BufReader::new(s);
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        let v: Value = serde_json::from_str(&line).unwrap_or(Value::String(line.clone()));
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        if !req.contains("\"follow\"") {
            break;
        }
        line.clear();
    }
    Ok(())
}

/// linuxai-core mail ingest FILE.eml... | linuxai-core mail watch
fn mail_cmd(args: &[String]) -> std::io::Result<()> {
    let client = client::Client::new(arg(args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket));
    let cfg = mail::MailConfig::from_env();
    match args.get(2).map(String::as_str) {
        Some("ingest") => {
            // Files are the arguments that aren't a --flag or a flag's value.
            let mut files = Vec::new();
            let mut it = args.iter().skip(3);
            while let Some(a) = it.next() {
                if a.starts_with("--") {
                    it.next();
                } else {
                    files.push(a);
                }
            }
            for f in files {
                let raw = std::fs::read(f)?;
                match mail::ingest(&raw, &client, &cfg) {
                    Ok(r) => println!("{f}: {}", r),
                    Err(e) => eprintln!("{f}: {e}"),
                }
            }
            Ok(())
        }
        Some("watch") => {
            client.wait_until_up();
            let every: u64 = std::env::var("LINUXAI_MAIL_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
            loop {
                match mail::poll_imap(&client, &cfg) {
                    Ok(0) => {}
                    Ok(n) => eprintln!("linuxai mail: ingested {n} message(s)"),
                    Err(e) => eprintln!("linuxai mail: {e}"),
                }
                std::thread::sleep(Duration::from_secs(every));
            }
        }
        _ => {
            eprintln!("usage: linuxai-core mail ingest FILE.eml... | linuxai-core mail watch");
            std::process::exit(2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    crash::install(args.get(1).map(String::as_str).unwrap_or("linuxai-core"));
    let result = match args.get(1).map(String::as_str) {
        Some("daemon") => daemon(&args),
        Some("ctl") => ctl(&args),
        Some("follow") => (|| -> std::io::Result<()> {
            // Every event from now on, one JSON object per line (for the screen's always-on layer).
            let c = client::Client::new(arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket));
            c.wait_until_up();
            let mut out = std::io::stdout().lock();
            for ev in c.follow()? {
                if writeln!(out, "{ev}").and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
            Ok(())
        })(),
        Some("check") => {
            // linuxai-core check [FILE.json | '<plan json>']   (no argument: the plan on stdin)
            let given = args.get(2).map(String::as_str).unwrap_or("/dev/stdin");
            let text = if given.trim_start().starts_with('{') { given.to_string() } else {
                match std::fs::read_to_string(given) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("linuxai-core check: can't read {given}: {e}");
                        std::process::exit(2);
                    }
                }
            };
            let plan: Value = match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("linuxai-core check: that plan isn't valid JSON: {e}");
                    std::process::exit(2);
                }
            };
            let mem = memory::Memory::in_memory();
            let r = engine::check(&plan, &mem, "ai", 0.8);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
            Ok(())
        }
        Some("reach") => reach::run(client::Client::new(arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket))),
        Some("mail") => mail_cmd(&args),
        Some("browser") => browser::run(
            arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(browser::default_socket),
            arg(&args, "--profile").map(PathBuf::from).unwrap_or_else(browser::default_profile),
            args.iter().any(|a| a == "--headless"),
            Some(client::Client::new(arg(&args, "--core").map(PathBuf::from).unwrap_or_else(default_socket))),
        ),
        Some("image") => {
            let client = client::Client::new(arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket));
            let cfg = images::ImageConfig::from_env();
            let req: Value = match args.get(2).map(String::as_str) {
                Some("caps") => json!({"op": "caps"}),
                Some("do") => serde_json::from_str(args.get(3).map(String::as_str).unwrap_or("")).unwrap_or(json!({})),
                _ => {
                    eprintln!("usage: linuxai-core image caps | image do '<json>'");
                    std::process::exit(2);
                }
            };
            println!("{}", images::handle(&req, &client, &cfg));
            Ok(())
        }
        Some("craft") => {
            // linuxai-core craft [--socket PATH]  |  craft do '<json>'  |  craft apps
            let socket = arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(craft::default_socket);
            match args.get(2).map(String::as_str) {
                Some("do") => {
                    let req: Value = serde_json::from_str(args.get(3).map(String::as_str).unwrap_or("")).unwrap_or(json!({}));
                    println!("{}", craft::request(&socket, &req));
                    Ok(())
                }
                Some("apps") => {
                    println!("{}", craft::Organ::new(craft::CraftConfig::from_env(), None).handle(&json!({"op": "apps"})));
                    Ok(())
                }
                _ => {
                    let core = client::Client::new(arg(&args, "--core").map(PathBuf::from).unwrap_or_else(default_socket));
                    craft::run(socket, Some(Box::new(core)))
                }
            }
        }
        Some("blender") => {
            let client = client::Client::new(arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket));
            let cfg = blender::BlenderConfig::from_env();
            let req: Value = match args.get(2).map(String::as_str) {
                Some("caps") => json!({"op": "caps"}),
                Some("do") => serde_json::from_str(args.get(3).map(String::as_str).unwrap_or("")).unwrap_or(json!({})),
                _ => {
                    eprintln!("usage: linuxai-core blender caps | blender do '<json>'");
                    std::process::exit(2);
                }
            };
            println!("{}", blender::handle(&req, &client, &cfg));
            Ok(())
        }
        Some("voice") => {
            let r = match args.get(2).map(String::as_str) {
                Some("caps") => voice::caps(),
                Some("listen") => voice::listen(std::time::Duration::from_secs(arg(&args, "--max").and_then(|m| m.parse().ok()).unwrap_or(60))),
                Some("transcribe") => match voice::transcribe(std::path::Path::new(args.get(3).map(String::as_str).unwrap_or(""))) {
                    Ok(t) => json!({"ok": true, "text": t}),
                    Err(e) => json!({"ok": false, "error": e}),
                },
                Some("say") => {
                    let mut text = String::new();
                    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text);
                    match voice::say(&text) {
                        Ok(()) => json!({"ok": true}),
                        Err(e) => json!({"ok": false, "error": e}),
                    }
                }
                _ => {
                    eprintln!("usage: linuxai-core voice caps | listen [--max SECS] | transcribe FILE | say");
                    std::process::exit(2);
                }
            };
            println!("{r}");
            Ok(())
        }
        Some("calendar") => (|| -> std::io::Result<()> {
            // linuxai-core calendar import FILE.ics [--name NAME] [--invite] | calendar watch
            let client = client::Client::new(arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(default_socket));
            match args.get(2).map(String::as_str) {
                Some("import") => {
                    let file = args.get(3).ok_or_else(|| std::io::Error::other("which .ics file?"))?;
                    let ics = std::fs::read_to_string(file)?;
                    let invite = args.iter().any(|a| a == "--invite");
                    let name = arg(&args, "--name").unwrap_or_else(|| std::path::Path::new(file).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                    let r = client.request("calendar_import", json!({"ics": ics, "kind": if invite { "invite" } else { "feed" },
                        "source": {"name": name, "file": file}}))?;
                    println!("{r}");
                    Ok(())
                }
                Some("watch") => {
                    // Subscribed calendars: LINUXAI_CALENDARS="work=https://...ics;school=https://...ics", every 15 min.
                    client.wait_until_up();
                    let every: u64 = std::env::var("LINUXAI_CALENDAR_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(900);
                    loop {
                        for pair in std::env::var("LINUXAI_CALENDARS").unwrap_or_default().split(';').filter(|p| !p.trim().is_empty()) {
                            let (name, url) = pair.split_once('=').unwrap_or(("subscribed", pair));
                            match std::process::Command::new("curl").args(["-fsSL", "--max-time", "60", url.trim()]).output() {
                                Ok(o) if o.status.success() => {
                                    let r = client.request("calendar_import", json!({"ics": String::from_utf8_lossy(&o.stdout), "kind": "feed",
                                        "source": {"name": name.trim(), "url": url.trim()}}));
                                    eprintln!("linuxai calendar: {}: {}", name.trim(), r.map(|v| v.to_string()).unwrap_or_else(|e| e.to_string()));
                                }
                                Ok(o) => eprintln!("linuxai calendar: {}: {}", name.trim(), String::from_utf8_lossy(&o.stderr).trim()),
                                Err(e) => eprintln!("linuxai calendar: {e}"),
                            }
                        }
                        std::thread::sleep(Duration::from_secs(every));
                    }
                }
                _ => {
                    eprintln!("usage: linuxai-core calendar import FILE.ics [--name NAME] [--invite] | calendar watch");
                    std::process::exit(2);
                }
            }
        })(),
        Some("compositor") => compositor::run(
            arg(&args, "--socket").map(PathBuf::from).unwrap_or_else(compositor::default_socket),
            Some(client::Client::new(arg(&args, "--core").map(PathBuf::from).unwrap_or_else(default_socket))),
        ),
        Some("snapshot") => {
            println!("{}", serde_json::to_string_pretty(&host::snapshot(true, &Value::Null)).unwrap());
            Ok(())
        }
        Some("verbs") => {
            println!("{}", serde_json::to_string_pretty(&verbs::registry_json()).unwrap());
            Ok(())
        }
        _ => {
            eprintln!("usage: linuxai-core daemon|ctl|check|snapshot|verbs|reach|mail|browser|image|voice|compositor|calendar|blender|craft  (see source header)");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("linuxai-core: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
