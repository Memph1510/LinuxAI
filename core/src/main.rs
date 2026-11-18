//! linuxai-core: the LinuxAI system service.
//!
//!   linuxai-core daemon [--socket PATH] [--db PATH] [--group NAME]   run the core
//!   linuxai-core ctl '<json request>'                                talk to a running core
//!   linuxai-core check PLAN.json                                     check a plan without running it
//!   linuxai-core snapshot                                            print what the host senses right now
//!   linuxai-core verbs                                               list verbs

mod core;
mod engine;
mod host;
mod memory;
mod playbooks;
mod tasks;
mod verbs;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;

fn default_socket() -> PathBuf {
    if let Ok(p) = std::env::var("LINUXAI_CORE_SOCKET") {
        return p.into();
    }
    if is_root() {
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
            core.subscribers.lock().unwrap().push(tx);
            let _ = writeln!(writer, "{}", json!({"ok": true, "following": true}));
            for ev in rx {
                if writeln!(writer, "{ev}").is_err() {
                    return;
                }
            }
            return;
        }
        let resp = core.handle(&req);
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
    core.file_gid = arg(args, "--group").or_else(|| std::env::var("LINUXAI_GROUP").ok()).as_deref().and_then(group_id);
    let core = Arc::new(core);
    core.recover();

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("daemon") => daemon(&args),
        Some("ctl") => ctl(&args),
        Some("check") => {
            let text = std::fs::read_to_string(args.get(2).map(String::as_str).unwrap_or("/dev/stdin")).unwrap_or_default();
            let plan: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let mem = memory::Memory::in_memory();
            let r = engine::check(&plan, &mem, "ai", 0.8);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
            Ok(())
        }
        Some("snapshot") => {
            println!("{}", serde_json::to_string_pretty(&host::snapshot(true, &Value::Null)).unwrap());
            Ok(())
        }
        Some("verbs") => {
            println!("{}", serde_json::to_string_pretty(&verbs::registry_json()).unwrap());
            Ok(())
        }
        _ => {
            eprintln!("usage: linuxai-core daemon|ctl|check|snapshot|verbs  (see source header)");
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
