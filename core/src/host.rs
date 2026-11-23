//! The Linux host organ: senses the machine through /proc and /sys, and controls it through the
//! kernel's steering surfaces (power profiles, sched_ext scheduler policy, cgroups via systemd, sysctl).

use crate::verbs::Organs;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct LinuxHost;

fn read(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(prog).is_file()))
        .unwrap_or(false)
}

fn cmd(prog: &str, args: &[&str], timeout: Duration) -> Result<(i32, String), String> {
    if !which(prog) {
        return Err(format!("{prog} is not installed"));
    }
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                return Err(format!("{prog} timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if text.len() > 16000 {
        let cut = text.len() - 8000;
        text = format!("{}\n…\n{}", &text[..8000.min(cut)], &text[cut..]);
    }
    Ok((out.status.code().unwrap_or(-1), text.trim().to_string()))
}

// ---- what needs root (independent review M4) -----------------------------------------------
// On the image the core runs as its own account (linuxai-core), never root. An action that needs root (an approved
// plan's program, a service, a power setting) runs as a transient systemd unit, which polkit allows that account
// (/etc/polkit-1/rules.d/50-linuxai-core.rules): one narrow path, each action its own unit in the journal. Run as
// root (development, tests), the core does it directly.

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// Through systemd only as the core's own account (the image), for which polkit allows it. Anywhere else (a
/// developer's machine, CI) the core does what it can as whoever runs it, as before.
fn via_systemd() -> bool {
    // SAFETY: geteuid has no preconditions.
    !is_root() && crate::peer::user_id(crate::peer::CORE_USER) == Some(unsafe { libc::geteuid() })
}

/// The command line that runs `prog args` as root: through systemd-run as the core's own account, else itself.
fn root_argv(prog: &str, args: &[&str], timeout: Duration) -> (String, Vec<String>) {
    if !via_systemd() {
        return (prog.to_string(), args.iter().map(|a| a.to_string()).collect());
    }
    let mut v: Vec<String> = ["--uid=0", "--pipe", "--wait", "--quiet", "--collect", "--service-type=exec"].iter().map(|s| s.to_string()).collect();
    v.push(format!("--property=RuntimeMaxSec={}", timeout.as_secs().max(1)));
    v.push("--".into());
    v.push(prog.to_string());
    v.extend(args.iter().map(|a| a.to_string()));
    ("systemd-run".to_string(), v)
}

/// `cmd`, as root.
fn root_cmd(prog: &str, args: &[&str], timeout: Duration) -> Result<(i32, String), String> {
    if !which(prog) {
        return Err(format!("{prog} is not installed"));
    }
    let (p, a) = root_argv(prog, args, timeout);
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    cmd(&p, &refs, timeout + Duration::from_secs(5))
}

/// `ok_cmd`, as root.
fn root_ok(prog: &str, args: &[&str]) -> Result<Value, String> {
    let (code, out) = root_cmd(prog, args, Duration::from_secs(60))?;
    if code == 0 {
        Ok(json!({"ok": true, "output": out}))
    } else {
        Err(format!("{prog} exited {code}: {out}"))
    }
}

/// Write a system file (sysfs, /proc/sys) as root.
fn root_write(path: &Path, value: &str) -> Result<(), String> {
    if !via_systemd() {
        return fs::write(path, value).map_err(|e| format!("{}: {e}", path.display()));
    }
    let p = path.display().to_string();
    let (code, out) = root_cmd("sh", &["-c", "printf '%s' \"$1\" > \"$2\"", "sh", value, &p], Duration::from_secs(30))?;
    if code == 0 { Ok(()) } else { Err(format!("{p}: {out}")) }
}

// ---------------------------------------------------------------------------------------------
// Senses
// ---------------------------------------------------------------------------------------------

fn pressure(name: &str) -> Value {
    // "some avg10=0.00 avg60=0.00 avg300=0.00 total=0"
    let Some(text) = read(format!("/proc/pressure/{name}")) else { return Value::Null };
    let mut out = Map::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let which = parts.next().unwrap_or("");
        for kv in parts {
            if let Some(v) = kv.strip_prefix("avg10=") {
                out.insert(format!("{which}_avg10"), json!(v.parse::<f64>().unwrap_or(0.0)));
            }
        }
    }
    Value::Object(out)
}

fn meminfo() -> Value {
    let text = read("/proc/meminfo").unwrap_or_default();
    let get = |k: &str| {
        text.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb / 1024)
    };
    json!({"total_mb": get("MemTotal:"), "available_mb": get("MemAvailable:"), "swap_free_mb": get("SwapFree:")})
}

fn power() -> Value {
    let mut ac = Value::Null;
    let mut bat = Value::Null;
    let mut status = Value::Null;
    if let Ok(dir) = fs::read_dir("/sys/class/power_supply") {
        for e in dir.flatten() {
            let p = e.path();
            match read(p.join("type")).as_deref() {
                Some("Mains") | Some("USB") => {
                    if let Some(o) = read(p.join("online")) {
                        ac = json!(o == "1" || ac == json!(true));
                    }
                }
                Some("Battery") => {
                    bat = read(p.join("capacity")).and_then(|c| c.parse::<i64>().ok()).map(|c| json!(c)).unwrap_or(Value::Null);
                    status = read(p.join("status")).map(Value::String).unwrap_or(Value::Null);
                }
                _ => {}
            }
        }
    }
    json!({"ac": ac, "battery": bat, "battery_status": status})
}

fn thermal_max() -> Value {
    let mut max: Option<f64> = None;
    if let Ok(dir) = fs::read_dir("/sys/class/thermal") {
        for e in dir.flatten() {
            if let Some(t) = read(e.path().join("temp")).and_then(|t| t.parse::<f64>().ok()) {
                let c = t / 1000.0;
                if c > 0.0 && c < 150.0 {
                    max = Some(max.map_or(c, |m: f64| m.max(c)));
                }
            }
        }
    }
    max.map(|m| json!(m)).unwrap_or(Value::Null)
}

fn failed_units() -> Value {
    match cmd("systemctl", &["--failed", "--no-legend", "--plain", "--no-pager"], Duration::from_secs(5)) {
        Ok((_, out)) => Value::Array(
            out.lines().filter_map(|l| l.split_whitespace().next()).filter(|u| u.contains('.')).map(|u| json!(u)).collect(),
        ),
        Err(_) => Value::Null,
    }
}

fn sched() -> Value {
    json!({"ext_state": read("/sys/kernel/sched_ext/state"),
           "ext_ops": read("/sys/kernel/sched_ext/root/ops"),
           "governor": read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")})
}

fn power_profile() -> Value {
    match cmd("powerprofilesctl", &["get"], Duration::from_secs(3)) {
        Ok((0, out)) => json!(out),
        _ => Value::Null,
    }
}

/// A snapshot of the machine. `full` adds the slower probes (spawning systemctl etc.).
pub fn snapshot(full: bool, prev: &Value) -> Value {
    let load = read("/proc/loadavg").unwrap_or_default();
    let load1 = load.split_whitespace().next().and_then(|v| v.parse::<f64>().ok());
    let mut s = json!({
        "host": read("/proc/sys/kernel/hostname"),
        "kernel": read("/proc/sys/kernel/osrelease"),
        "uptime_s": read("/proc/uptime").and_then(|u| u.split_whitespace().next().and_then(|v| v.parse::<f64>().ok())),
        "cpu": {"count": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
                "load1": load1, "pressure": pressure("cpu")},
        "memory": meminfo(),
        "memory_pressure": pressure("memory"),
        "io_pressure": pressure("io"),
        "power": power(),
        "temperature_max_c": thermal_max(),
        "sched": sched(),
    });
    let m = s.as_object_mut().unwrap();
    if full {
        m.insert("failed_units".into(), failed_units());
        m.insert("power_profile".into(), power_profile());
    } else {
        for k in ["failed_units", "power_profile"] {
            m.insert(k.into(), prev.get(k).cloned().unwrap_or(Value::Null));
        }
    }
    s
}

fn f(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_f64()
}

/// Turn the difference between two snapshots into events.
pub fn derive_events(prev: &Value, cur: &Value) -> Vec<(String, Value)> {
    let mut ev = Vec::new();
    if prev.is_null() {
        ev.push(("system.started".to_string(), json!({"kernel": cur["kernel"], "host": cur["host"]})));
        return ev;
    }
    let ac0 = prev["power"]["ac"].as_bool();
    let ac1 = cur["power"]["ac"].as_bool();
    if let (Some(a), Some(b)) = (ac0, ac1) {
        if a && !b {
            ev.push(("power.unplugged".into(), json!({"battery": cur["power"]["battery"]})));
        } else if !a && b {
            ev.push(("power.plugged".into(), json!({"battery": cur["power"]["battery"]})));
        }
    }
    let crossed_below = |path: &[&str], t: f64| match (f(prev, path), f(cur, path)) {
        (Some(a), Some(b)) => a >= t && b < t,
        _ => false,
    };
    let crossed_above = |path: &[&str], t: f64| match (f(prev, path), f(cur, path)) {
        (Some(a), Some(b)) => a <= t && b > t,
        _ => false,
    };
    if crossed_below(&["power", "battery"], 8.0) {
        ev.push(("battery.critical".into(), json!({"percent": cur["power"]["battery"]})));
    } else if crossed_below(&["power", "battery"], 20.0) {
        ev.push(("battery.low".into(), json!({"percent": cur["power"]["battery"]})));
    }
    if crossed_above(&["memory_pressure", "some_avg10"], 25.0) {
        ev.push(("memory.pressure".into(), json!({"some_avg10": cur["memory_pressure"]["some_avg10"],
                                                   "available_mb": cur["memory"]["available_mb"]})));
    }
    if crossed_above(&["cpu", "pressure", "some_avg10"], 80.0) {
        ev.push(("cpu.pressure".into(), json!({"some_avg10": cur["cpu"]["pressure"]["some_avg10"]})));
    }
    if crossed_above(&["temperature_max_c"], 90.0) {
        ev.push(("temperature.high".into(), json!({"celsius": cur["temperature_max_c"]})));
    }
    if let (Some(a), Some(b)) = (prev["failed_units"].as_array(), cur["failed_units"].as_array()) {
        for u in b.iter().filter(|u| !a.contains(u)) {
            ev.push(("service.failed".into(), json!({"unit": u})));
        }
        for u in a.iter().filter(|u| !b.contains(u)) {
            ev.push(("service.recovered".into(), json!({"unit": u})));
        }
    }
    ev
}

// ---------------------------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------------------------

fn s(args: &Value, k: &str) -> String {
    match args.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => v.to_string(),
    }
}

fn write_governor(gov: &str) -> Result<Value, String> {
    let mut n = 0;
    if let Ok(dir) = fs::read_dir("/sys/devices/system/cpu") {
        for e in dir.flatten() {
            let p = e.path().join("cpufreq/scaling_governor");
            if p.exists() {
                root_write(&p, gov)?;
                n += 1;
            }
        }
    }
    if n == 0 {
        return Err("no CPU frequency control on this machine".into());
    }
    Ok(json!({"ok": true, "cpus": n, "governor": gov}))
}

/// sched_ext policy → (scheduler, scx_loader mode).
fn scx_policy(policy: &str) -> Option<(&'static str, &'static str)> {
    Some(match policy {
        "balanced" | "auto" => ("scx_lavd", "auto"),
        "efficiency" | "powersave" => ("scx_lavd", "powersave"),
        "responsive" | "lowlatency" => ("scx_lavd", "lowlatency"),
        "gaming" => ("scx_lavd", "gaming"),
        "server" | "throughput" => ("scx_rusty", "server"),
        _ => return None,
    })
}

fn set(what: &str, to: &str) -> Result<Value, String> {
    match what {
        "power.profile" => {
            let via_daemon = if which("powerprofilesctl") { root_ok("powerprofilesctl", &["set", to]) } else {
                Err("power-profiles-daemon is not installed".into())
            };
            via_daemon.or_else(|e| {
                // Fall back to steering the CPU frequency governor directly.
                let gov = match to {
                    "power-saver" => "powersave",
                    "performance" => "performance",
                    _ => "schedutil",
                };
                write_governor(gov).map_err(|g| format!("{e}; fallback: {g}"))
            })
        }
        "cpu.governor" => write_governor(to),
        "cpu.policy" => {
            if !Path::new("/sys/kernel/sched_ext").exists() {
                return Err("this kernel has no sched_ext support".into());
            }
            if to == "off" || to == "default" {
                return root_ok("scxctl", &["stop"]);
            }
            let (sched, mode) = scx_policy(to).ok_or_else(|| format!("unknown cpu policy '{to}'"))?;
            let running = read("/sys/kernel/sched_ext/state").map(|s| s == "enabled").unwrap_or(false);
            let verb = if running { "switch" } else { "start" };
            root_ok("scxctl", &[verb, "--sched", sched.trim_start_matches("scx_"), "--mode", mode])
        }
        "screen.brightness" => {
            let pct: f64 = to.trim_end_matches('%').parse().map_err(|_| "brightness must be 0-100")?;
            let dir = fs::read_dir("/sys/class/backlight").map_err(|_| "no backlight")?;
            let mut n = 0;
            for e in dir.flatten() {
                let max: f64 = read(e.path().join("max_brightness")).and_then(|m| m.parse().ok()).unwrap_or(0.0);
                if max > 0.0 {
                    let v = ((pct.clamp(1.0, 100.0) / 100.0) * max).round() as u64;
                    root_write(&e.path().join("brightness"), &v.to_string())?;
                    n += 1;
                }
            }
            if n == 0 { Err("no adjustable backlight".into()) } else { Ok(json!({"ok": true, "percent": pct})) }
        }
        w if w.starts_with("sysctl.") => {
            let key = &w["sysctl.".len()..];
            if key.contains("..") || key.contains('/') {
                return Err("invalid sysctl key".into());
            }
            let path = format!("/proc/sys/{}", key.replace('.', "/"));
            root_write(Path::new(&path), to)?;
            Ok(json!({"ok": true, "sysctl": key, "value": to}))
        }
        other => Err(format!("don't know how to set '{other}'")),
    }
}

impl Organs for LinuxHost {
    fn invoke(&self, verb: &str, args: &Value) -> Result<Value, String> {
        // Rehearsal: the whole core runs (rules, plans, trust, habits, journal), but nothing touches the machine.
        // For benchmarks and for trying a plan safely. Set LINUXAI_REHEARSAL=1 on the daemon.
        if std::env::var("LINUXAI_REHEARSAL").map(|v| v == "1").unwrap_or(false) {
            return Ok(json!({"rehearsal": true, "verb": verb, "args": args}));
        }
        match verb {
            "set" => set(&s(args, "what"), &s(args, "to")),
            "limit" => {
                let unit = s(args, "unit");
                let mut props = vec!["set-property".to_string(), "--runtime".into(), unit.clone()];
                for (arg, prop) in [("cpu_weight", "CPUWeight"), ("memory_high", "MemoryHigh"),
                                    ("memory_max", "MemoryMax"), ("io_weight", "IOWeight")] {
                    let v = s(args, arg);
                    if !v.is_empty() {
                        props.push(format!("{prop}={v}"));
                    }
                }
                if props.len() == 3 {
                    return Err("limit needs at least one of cpu_weight, memory_high, memory_max, io_weight".into());
                }
                let refs: Vec<&str> = props.iter().map(String::as_str).collect();
                root_ok("systemctl", &refs)
            }
            "start" | "stop" | "restart" => root_ok("systemctl", &[verb, &s(args, "unit")]),
            "run" => {
                let program = s(args, "program");
                let list: Vec<String> = args
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect())
                    .unwrap_or_default();
                let refs: Vec<&str> = list.iter().map(String::as_str).collect();
                let timeout = args.get("timeout").and_then(Value::as_u64).unwrap_or(300);
                let (code, output) = root_cmd(&program, &refs, Duration::from_secs(timeout))?;
                Ok(json!({"code": code, "output": output}))
            }
            // A browser action the human approved (or trusts on this site): the core carries it out, naming the site,
            // and the browser checks the page is still that site (review H4).
            "browse" => {
                let mut req = args.clone();
                let op = s(args, "op");
                req["approved_origin"] = json!(s(args, "site"));
                if let Some(m) = req.as_object_mut() {
                    m.remove("site");
                    m.remove("op");
                }
                let c = crate::client::Client::new(crate::browser::default_socket());
                let r = c.request(&op, req).map_err(|e| format!("the browser isn't reachable: {e}"))?;
                if r["ok"] == false {
                    return Err(r["error"].as_str().unwrap_or("the browser refused").to_string());
                }
                Ok(r)
            }
            other => Err(format!("the host has no verb '{other}'")),
        }
    }
}
