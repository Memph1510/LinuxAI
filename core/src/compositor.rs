//! The compositor organ: the AI's hands and eyes on the screen, through Hyprland.
//!
//! `linuxai-core compositor` runs inside the Hyprland session. It talks to Hyprland's control socket (the same
//! one `hyprctl` uses) and listens to its event socket. It gives the AI:
//!   - what's on screen: windows, which one you're in, where your pointer is ("what you're looking at", for every
//!     app, not only the browser);
//!   - control: focus, move to a workspace, fullscreen, float, close a window;
//!   - power: turns animations and blur off on battery and back on when plugged in (it follows the core's events).
//! Hyprland 0.55+ takes Lua over its control socket (`dispatch hl.dsp…`, `eval …`); the info queries are JSON.
//! It reports focus changes and windows opening and closing to the core as events.
//!
//! Requests: one JSON object per line on `$XDG_RUNTIME_DIR/linuxai-compositor.sock`:
//!   {"op":"windows"}  {"op":"active"}  {"op":"monitors"}
//!   {"op":"focus","window":ADDR|"class":...}  {"op":"workspace","window":ADDR,"to":N}  {"op":"fullscreen","window":ADDR}
//!   {"op":"float","window":ADDR}  {"op":"close","window":ADDR}  {"op":"effects","on":bool}
//!   {"op":"shot","window"?:ADDR|"app"?:...}  → a screenshot to look at (grim), with its origin on screen
//!   {"op":"point","x","y","w","h","note"?,"magnify"?,"window"?|"app"?} → outline that area for the human, over any app
//!     (magnify: also show it enlarged in a lens, for small print)

use crate::client::Client;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Hyprland's sockets for the running instance.
pub struct Hypr {
    pub dir: PathBuf,
}

impl Hypr {
    pub fn from_env() -> Option<Hypr> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        let run = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"));
        let dir = run.join("hypr").join(&sig);
        let dir = if dir.exists() { dir } else { PathBuf::from("/tmp/hypr").join(sig) };
        Some(Hypr { dir })
    }

    /// One request to Hyprland's control socket; `j/` asks for JSON.
    pub fn ask(&self, req: &str) -> std::io::Result<String> {
        let mut s = UnixStream::connect(self.dir.join(".socket.sock"))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.write_all(req.as_bytes())?;
        let mut out = String::new();
        s.read_to_string(&mut out)?;
        Ok(out)
    }

    pub fn json(&self, what: &str) -> std::io::Result<Value> {
        let raw = self.ask(&format!("j/{what}"))?;
        serde_json::from_str(&raw).map_err(|e| std::io::Error::other(format!("{what}: {e}")))
    }

    /// Hyprland 0.55+ takes Lua: `dispatch <hl.dsp…>` runs a dispatcher, `eval <lua>` anything else.
    fn lua(&self, verb: &str, code: &str) -> Result<(), String> {
        let r = self.ask(&format!("{verb} {code}")).map_err(|e| e.to_string())?;
        if r.trim() == "ok" { Ok(()) } else { Err(format!("Hyprland: {}", r.trim())) }
    }

    fn dispatch(&self, dsp: &str) -> Result<(), String> {
        self.lua("dispatch", dsp)
    }
}

/// A window as the AI needs it.
fn window(c: &Value) -> Value {
    json!({"window": c["address"], "app": c["class"], "title": c["title"], "workspace": c["workspace"]["id"],
           "at": c["at"], "size": c["size"], "floating": c["floating"], "fullscreen": c["fullscreen"], "pid": c["pid"],
           "focused_order": c["focusHistoryID"]})
}

/// Window addresses look like 0x55d1c3a0b2f0; nothing else goes into a dispatch.
fn addr(v: &Value) -> Option<String> {
    let a = v.as_str()?;
    (a.starts_with("0x") && a.len() <= 20 && a[2..].chars().all(|c| c.is_ascii_hexdigit())).then(|| a.to_string())
}

pub struct Organ {
    pub hypr: Hypr,
    pub core: Option<Client>,
}

fn err(e: impl std::fmt::Display) -> Value {
    json!({"ok": false, "error": e.to_string()})
}

impl Organ {
    /// The window the request names: by address, or the most recently used window of an app.
    fn target(&self, req: &Value) -> Result<String, String> {
        if let Some(a) = addr(&req["window"]) {
            return Ok(a);
        }
        if let Some(app) = req["class"].as_str().or(req["app"].as_str()) {
            let list = self.hypr.json("clients").map_err(|e| e.to_string())?;
            let mut matches: Vec<&Value> = list.as_array().map(|a| a.iter().filter(|c| c["class"].as_str().map(|k| k.eq_ignore_ascii_case(app)).unwrap_or(false)).collect()).unwrap_or_default();
            matches.sort_by_key(|c| c["focusHistoryID"].as_i64().unwrap_or(i64::MAX));
            return matches.first().and_then(|c| addr(&c["address"])).ok_or(format!("no {app} window open"));
        }
        Err("say which window (window address from windows, or app)".into())
    }

    pub fn handle(&self, req: &Value) -> Value {
        let r: Result<Value, String> = (|| match req["op"].as_str().unwrap_or("") {
            "ping" => Ok(json!({"ok": true})),
            "windows" => {
                let list = self.hypr.json("clients").map_err(|e| e.to_string())?;
                let mut w: Vec<Value> = list.as_array().map(|a| a.iter().map(window).collect()).unwrap_or_default();
                w.sort_by_key(|x| x["focused_order"].as_i64().unwrap_or(i64::MAX));
                Ok(json!({"ok": true, "windows": w}))
            }
            "active" => {
                // What the human is looking at, anywhere on screen: the window they're in and where the pointer is.
                let w = self.hypr.json("activewindow").map_err(|e| e.to_string())?;
                let p = self.hypr.json("cursorpos").unwrap_or(json!({}));
                let inside = match (p["x"].as_f64(), p["y"].as_f64(), w["at"][0].as_f64(), w["at"][1].as_f64()) {
                    (Some(x), Some(y), Some(ax), Some(ay)) => json!({"x": x - ax, "y": y - ay}),
                    _ => Value::Null,
                };
                Ok(json!({"ok": true, "window": if w.get("address").is_some() { window(&w) } else { Value::Null },
                          "pointer": {"x": p["x"], "y": p["y"]}, "pointer_in_window": inside}))
            }
            "monitors" => Ok(json!({"ok": true, "monitors": self.hypr.json("monitors").map_err(|e| e.to_string())?})),
            "focus" => {
                let a = self.target(req)?;
                self.hypr.dispatch(&format!("hl.dsp.focus({{ window = \"address:{a}\" }})"))?;
                Ok(json!({"ok": true, "window": a}))
            }
            "workspace" => {
                let a = self.target(req)?;
                let to = req["to"].as_i64().filter(|n| (1..=99).contains(n)).ok_or("to: a workspace number 1-99")?;
                self.hypr.dispatch(&format!("hl.dsp.window.move({{ window = \"address:{a}\", workspace = \"{to}\", follow = false }})"))?;
                Ok(json!({"ok": true, "window": a, "workspace": to}))
            }
            "fullscreen" | "float" | "close" => {
                let a = self.target(req)?;
                let d = match req["op"].as_str().unwrap_or("") {
                    "fullscreen" => format!("hl.dsp.window.fullscreen({{ window = \"address:{a}\", action = \"toggle\" }})"),
                    "float" => format!("hl.dsp.window.float({{ window = \"address:{a}\", action = \"toggle\" }})"),
                    _ => format!("hl.dsp.window.close({{ window = \"address:{a}\" }})"),
                };
                self.hypr.dispatch(&d)?;
                Ok(json!({"ok": true, "window": a}))
            }
            "shot" => {
                // A picture of the screen, or of one window, for the AI to look at (grim).
                let geom = match addr(&req["window"]).or_else(|| (req["window"].is_null() && (req["app"].is_string() || req["class"].is_string())).then(|| self.target(req).ok()).flatten()) {
                    Some(a) => {
                        let list = self.hypr.json("clients").map_err(|e| e.to_string())?;
                        let c = list.as_array().and_then(|l| l.iter().find(|c| c["address"] == a.as_str()).cloned()).ok_or("no such window")?;
                        Some((c["at"][0].as_i64().unwrap_or(0), c["at"][1].as_i64().unwrap_or(0), c["size"][0].as_i64().unwrap_or(0), c["size"][1].as_i64().unwrap_or(0)))
                    }
                    None => None,
                };
                let mut cmd = std::process::Command::new("grim");
                cmd.args(["-t", "jpeg", "-q", "75"]);
                if let Some((x, y, w, h)) = geom {
                    cmd.args(["-g", &format!("{x},{y} {w}x{h}")]);
                }
                let o = cmd.arg("-").output().map_err(|e| format!("can't take a screenshot (grim): {e}"))?;
                if !o.status.success() {
                    return Err(format!("screenshot failed: {}", String::from_utf8_lossy(&o.stderr).trim()));
                }
                let (ox, oy) = geom.map(|g| (g.0, g.1)).unwrap_or((0, 0));
                Ok(json!({"ok": true, "media_type": "image/jpeg", "data": b64(&o.stdout), "origin": [ox, oy],
                          "note": "positions in this picture plus origin are screen positions, for point"}))
            }
            "point" => {
                // Show the human what the AI means, over any app: an outline and a note, drawn by the screen's
                // always-on layer (Quickshell), never blocking clicks. Screen coordinates, or relative to a window.
                let n = |k: &str| req[k].as_f64();
                let (Some(mut x), Some(mut y), Some(w), Some(h)) = (n("x"), n("y"), n("w"), n("h")) else {
                    return Err("x, y, w, h: the area to show (screen pixels, or inside the window given)".into());
                };
                if addr(&req["window"]).is_some() || req["app"].is_string() {
                    let a = self.target(req)?;
                    let list = self.hypr.json("clients").map_err(|e| e.to_string())?;
                    let c = list.as_array().and_then(|l| l.iter().find(|c| c["address"] == a.as_str()).cloned()).ok_or("no such window")?;
                    x += c["at"][0].as_f64().unwrap_or(0.0);
                    y += c["at"][1].as_f64().unwrap_or(0.0);
                }
                let mark = json!({"x": x, "y": y, "w": w, "h": h, "note": req["note"].as_str().unwrap_or(""),
                                  "magnify": req["magnify"].as_bool().unwrap_or(false),
                                  "seconds": n("seconds").unwrap_or(8.0).clamp(1.0, 60.0)});
                self.tell_core("screen.point", mark.clone());
                Ok(json!({"ok": true, "shown": mark}))
            }
            "effects" => {
                let on = req["on"].as_bool().ok_or("on: true or false")?;
                self.hypr.lua("eval", &format!("hl.config({{ animations = {{ enabled = {on} }}, decoration = {{ blur = {{ enabled = {on} }}, shadow = {{ enabled = {on} }} }} }})"))?;
                Ok(json!({"ok": true, "effects": on}))
            }
            other => Err(format!("unknown compositor op {other:?}")),
        })();
        r.unwrap_or_else(err)
    }

    fn tell_core(&self, kind: &str, data: Value) {
        if let Some(c) = &self.core {
            let _ = c.request("event", json!({"kind": kind, "data": data}));
        }
    }

    /// Hyprland's events, turned into the core's: focus changes and windows opening and closing.
    pub fn translate(line: &str) -> Option<(&'static str, Value)> {
        let (ev, data) = line.split_once(">>")?;
        let f: Vec<&str> = data.splitn(4, ',').collect();
        match ev {
            "activewindow" if f.len() >= 2 && !f[0].is_empty() => Some(("screen.focus", json!({"app": f[0], "title": data.split_once(',').map(|x| x.1).unwrap_or("")}))),
            "openwindow" if f.len() == 4 => Some(("screen.window.opened", json!({"window": format!("0x{}", f[0]), "workspace": f[1], "app": f[2], "title": f[3]}))),
            "closewindow" => Some(("screen.window.closed", json!({"window": format!("0x{data}")}))),
            _ => None,
        }
    }

    /// The Workspace is the main screen: it belongs in the layout, never floating over it (Chromium's app window
    /// can open floating, bigger than the screen).
    pub fn tidy(&self) {
        let Ok(list) = self.hypr.json("clients") else { return };
        for c in list.as_array().cloned().unwrap_or_default() {
            if c["title"] == "LinuxAI Workspace" && c["floating"] == true {
                if let Some(a) = addr(&c["address"]) {
                    let _ = self.hypr.dispatch(&format!("hl.dsp.window.float({{ window = \"address:{a}\", action = \"toggle\" }})"));
                }
            }
        }
    }

    pub fn watch_events(self: &Arc<Self>) {
        // The Workspace may open before or after this organ starts: check now and for the first minutes.
        let me = self.clone();
        std::thread::spawn(move || for _ in 0..60 {
            me.tidy();
            std::thread::sleep(Duration::from_secs(3));
        });
        let me = self.clone();
        std::thread::spawn(move || loop {
            if let Ok(s) = UnixStream::connect(me.hypr.dir.join(".socket2.sock")) {
                let mut last_focus = String::new();
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    if line.starts_with("openwindow>>") || line.starts_with("windowtitle") {
                        me.tidy();
                    }
                    if let Some((kind, data)) = Self::translate(&line) {
                        if kind == "screen.focus" {
                            let key = data.to_string();
                            if key == last_focus {
                                continue;
                            }
                            last_focus = key;
                        }
                        me.tell_core(kind, data);
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        });
    }

    /// On battery, no animations or blur; plugged in, back on.
    pub fn follow_power(self: &Arc<Self>) {
        let Some(core) = &self.core else { return };
        let core = Client::new(core.path.clone());
        let me = self.clone();
        std::thread::spawn(move || loop {
            if let Ok(events) = core.follow() {
                for ev in events {
                    match ev["kind"].as_str() {
                        Some("power.unplugged") => { me.handle(&json!({"op": "effects", "on": false})); }
                        Some("power.plugged") => { me.handle(&json!({"op": "effects", "on": true})); }
                        _ => {}
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        });
    }
}

/// Standard base64 (for screenshots handed to the AI).
pub fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn default_socket() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir()).join("linuxai-compositor.sock")
}

fn serve(organ: Arc<Organ>, stream: UnixStream) {
    let Ok(mut w) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let resp = serde_json::from_str::<Value>(&line).map(|r| organ.handle(&r)).unwrap_or_else(|e| err(format!("bad json: {e}")));
        if writeln!(w, "{resp}").is_err() {
            return;
        }
    }
}

/// `linuxai-core compositor [--socket PATH] [--core SOCKET]`, started by the Hyprland session.
pub fn run(socket: PathBuf, core: Option<Client>) -> std::io::Result<()> {
    let hypr = Hypr::from_env().ok_or_else(|| std::io::Error::other("not inside a Hyprland session (HYPRLAND_INSTANCE_SIGNATURE unset)"))?;
    let organ = Arc::new(Organ { hypr, core });
    organ.watch_events();
    organ.follow_power();
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    eprintln!("linuxai compositor: on {}", socket.display());
    for s in listener.incoming().flatten() {
        let o = organ.clone();
        std::thread::spawn(move || serve(o, s));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A stand-in Hyprland: answers the control socket like Hyprland does and records what it was asked.
    fn fake_hyprland(dir: &std::path::Path) -> Arc<Mutex<Vec<String>>> {
        std::fs::create_dir_all(dir).unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let l = UnixListener::bind(dir.join(".socket.sock")).unwrap();
        let rec = asked.clone();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                rec.lock().unwrap().push(req.clone());
                let out = match req.as_str() {
                    "j/clients" => json!([
                        {"address": "0x5a1", "class": "chromium", "title": "Compare SUVs", "workspace": {"id": 1}, "at": [0, 0], "size": [1280, 900], "floating": false, "fullscreen": 0, "pid": 10, "focusHistoryID": 1},
                        {"address": "0x5b2", "class": "foot", "title": "LinuxAI", "workspace": {"id": 1}, "at": [1280, 0], "size": [640, 900], "floating": false, "fullscreen": 0, "pid": 11, "focusHistoryID": 0},
                        {"address": "0x5c3", "class": "Chromium", "title": "LinuxAI Workspace", "workspace": {"id": 1}, "at": [0, -100], "size": [1280, 900], "floating": true, "fullscreen": 0, "pid": 12, "focusHistoryID": 2}]).to_string(),
                    "j/activewindow" => json!({"address": "0x5a1", "class": "chromium", "title": "Compare SUVs", "workspace": {"id": 1}, "at": [0, 40], "size": [1280, 860]}).to_string(),
                    "j/cursorpos" => json!({"x": 300, "y": 240}).to_string(),
                    r if r.starts_with("dispatch hl.dsp.") || r.starts_with("eval hl.") => "ok".into(),
                    _ => "unknown request".into(),
                };
                let _ = s.write_all(out.as_bytes());
            }
        });
        asked
    }

    #[test]
    fn sees_and_steers_the_screen() {
        let dir = std::env::temp_dir().join(format!("lx-hypr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let asked = fake_hyprland(&dir);
        let o = Organ { hypr: Hypr { dir: dir.clone() }, core: None };

        let w = o.handle(&json!({"op": "windows"}));
        assert_eq!(w["windows"][0]["app"], "foot", "most recently used first: {w}");
        let a = o.handle(&json!({"op": "active"}));
        assert_eq!(a["window"]["app"], "chromium");
        assert_eq!(a["pointer_in_window"], json!({"x": 300.0, "y": 200.0}), "pointer relative to the window: {a}");

        assert_eq!(o.handle(&json!({"op": "focus", "app": "Chromium"}))["window"], "0x5a1");
        assert_eq!(o.handle(&json!({"op": "workspace", "window": "0x5b2", "to": 3}))["ok"], true);
        assert_eq!(o.handle(&json!({"op": "effects", "on": false}))["ok"], true);
        let got = asked.lock().unwrap().clone();
        assert!(got.contains(&r#"dispatch hl.dsp.focus({ window = "address:0x5a1" })"#.to_string()), "{got:?}");
        assert!(got.contains(&r#"dispatch hl.dsp.window.move({ window = "address:0x5b2", workspace = "3", follow = false })"#.to_string()));
        assert!(got.iter().any(|g| g.starts_with("eval hl.config({ animations = { enabled = false }")), "{got:?}");

        // The Workspace never floats: tidy puts it back in the layout (the stand-in lists it as floating).
        o.tidy();
        assert!(asked.lock().unwrap().iter().any(|g| g == r#"dispatch hl.dsp.window.float({ window = "address:0x5c3", action = "toggle" })"#));
        // Nothing but a real window address reaches a dispatch.
        assert_eq!(o.handle(&json!({"op": "close", "window": "0x1; exec rm -rf ~"}))["ok"], false);
        assert_eq!(o.handle(&json!({"op": "workspace", "window": "0x5b2", "to": 500}))["ok"], false);
        assert_eq!(o.handle(&json!({"op": "focus", "app": "gimp"}))["error"], "no gimp window open");
        // Pointing inside a window becomes screen coordinates (the chromium window is at 0,0; foot at 1280,0).
        let p = o.handle(&json!({"op": "point", "app": "foot", "x": 10, "y": 20, "w": 100, "h": 30, "note": "this line", "magnify": true}));
        assert_eq!(p["shown"]["x"], 1290.0, "{p}");
        assert_eq!(p["shown"]["magnify"], true);
        assert_eq!(p["shown"]["note"], "this line");
        assert_eq!(o.handle(&json!({"op": "point", "x": 1}))["ok"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn hyprland_events_become_core_events() {
        assert_eq!(Organ::translate("activewindow>>chromium,Compare SUVs, 2027").unwrap(),
                   ("screen.focus", json!({"app": "chromium", "title": "Compare SUVs, 2027"})));
        assert_eq!(Organ::translate("openwindow>>5a1,1,foot,LinuxAI").unwrap().1["window"], "0x5a1");
        assert_eq!(Organ::translate("closewindow>>5a1").unwrap(), ("screen.window.closed", json!({"window": "0x5a1"})));
        assert!(Organ::translate("activewindow>>,").is_none());
        assert!(Organ::translate("workspace>>2").is_none());
    }
}
