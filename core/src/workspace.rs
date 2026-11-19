//! The Workspace bridge: the main screen is a page in the shared browser, with a private channel to the OS.
//!
//! The browser organ opens the Workspace page in its own window and gives that one page a binding,
//! `window.linuxai(json)`. Nothing else gets it: there is no network server, and a call is honored only while the
//! tab is still showing the Workspace page itself. Through it the page:
//!   - asks the core for what to show (tasks, a task's steps, waiting questions, recent events) and answers
//!     questions as the human (it is the human's screen);
//!   - shows a web page inside a task: the organ streams a live picture of that tab (Chromium's screencast) into
//!     the Workspace, and passes the human's clicks, scrolling and typing back to the real tab. The page lives in
//!     the book, not in a separate browser;
//!   - gets the core's events as they happen.
//!
//! Page → organ: `linuxai(JSON.stringify({id, op, ...}))`. Organ → page: `linuxaiReply(id, result)`,
//! `linuxaiEvent(event)`, `linuxaiFrame(tab, jpegBase64, {width, height, scroll})`.

use crate::browser::{press, Cdp, Organ};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the Workspace page may ask the core: reading what to show, and answering as the human.
const CORE_OPS: [&str; 9] = ["tasks", "task", "waiting", "answer", "events", "state", "calendar", "invites", "invite_answer"];

pub fn default_path() -> String {
    std::env::var("LINUXAI_WORKSPACE").unwrap_or_else(|_| "/usr/share/linuxai/workspace/index.html".into())
}

pub fn default_url() -> String {
    format!("file://{}", default_path())
}

pub struct Workspace {
    cdp: Arc<Cdp>,
    core: Option<crate::client::Client>,
    pub tab: String,
    session: String,
    url: String,
    /// The tab being shown inside the Workspace, and its screencast session.
    watched: Mutex<Option<(String, String)>>,
}

impl Workspace {
    /// Open the Workspace (or bring it back if it's already open).
    pub fn open(organ: &Organ, b: &Arc<Cdp>, url: &str) -> std::io::Result<String> {
        if let Some(w) = organ.workspace.lock().unwrap().as_ref().filter(|w| b.owner(&w.tab).is_some() && Arc::ptr_eq(&w.cdp, b)) {
            let _ = b.call("Target.activateTarget", json!({"targetId": w.tab}), None);
            return Ok(w.tab.clone());
        }
        // On screen, Chromium started with the Workspace as its app window: take that one over. Otherwise open it in
        // its own window. Either way the channel goes in before the page (re)loads, so it has it from the start.
        let existing = b.tabs().into_iter().find(|t| t["url"] == url).and_then(|t| t["tab"].as_str().map(String::from));
        let tab = match existing {
            Some(t) => t,
            None => {
                let t = b.call("Target.createTarget", json!({"url": "about:blank", "newWindow": true}), None)?["targetId"]
                    .as_str().unwrap_or("").to_string();
                let start = Instant::now();
                while b.owner(&t).is_none() && start.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
                t
            }
        };
        b.set_owner(&tab, "workspace");
        let session = b.session(&tab)?;
        b.call("Runtime.enable", json!({}), Some(&session))?;
        b.call("Runtime.addBinding", json!({"name": "linuxai"}), Some(&session))?;
        b.call("Page.enable", json!({}), Some(&session))?;
        b.call("Page.navigate", json!({"url": url}), Some(&session))?;
        b.settle(&tab);
        let w = Arc::new(Workspace { cdp: b.clone(), core: organ.core.as_ref().map(|c| crate::client::Client::new(c.path.clone())),
                                     tab: tab.clone(), session, url: url.to_string(), watched: Mutex::new(None) });
        w.clone().listen();
        w.clone().forward_core_events();
        *organ.workspace.lock().unwrap() = Some(w);
        Ok(tab)
    }

    fn listen(self: Arc<Self>) {
        let rx = self.cdp.listen();
        std::thread::spawn(move || {
            for ev in rx {
                let sid = ev["sessionId"].as_str().unwrap_or("");
                match ev["method"].as_str().unwrap_or("") {
                    "Runtime.bindingCalled" if sid == self.session && ev["params"]["name"] == "linuxai" => {
                        // Only the Workspace page itself: if the tab ever showed anything else, it gets nothing.
                        if self.cdp.url_of(&self.tab).as_deref() != Some(self.url.as_str()) {
                            continue;
                        }
                        let me = self.clone();
                        let payload = ev["params"]["payload"].as_str().unwrap_or("").to_string();
                        std::thread::spawn(move || {
                            let req: Value = serde_json::from_str(&payload).unwrap_or(json!({}));
                            let out = me.handle(&req);
                            me.call_page("linuxaiReply", &[req["id"].clone(), out]);
                        });
                    }
                    "Page.screencastFrame" => {
                        let watched = self.watched.lock().unwrap().clone();
                        if let Some((tab, s)) = watched.filter(|(_, s)| s == sid) {
                            let p = &ev["params"];
                            let _ = self.cdp.call("Page.screencastFrameAck", json!({"sessionId": p["sessionId"]}), Some(&s));
                            let m = &p["metadata"];
                            self.call_page("linuxaiFrame", &[json!(tab), p["data"].clone(),
                                json!({"width": m["deviceWidth"], "height": m["deviceHeight"], "scroll": [m["scrollOffsetX"], m["scrollOffsetY"]]})]);
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    /// The core's events, into the Workspace as they happen.
    fn forward_core_events(self: Arc<Self>) {
        let Some(core) = self.core.as_ref().map(|c| crate::client::Client::new(c.path.clone())) else { return };
        std::thread::spawn(move || loop {
            if let Ok(events) = core.follow() {
                for ev in events {
                    if !self.cdp.alive() || self.cdp.owner(&self.tab).is_none() {
                        return;
                    }
                    self.call_page("linuxaiEvent", &[ev]);
                }
            }
            if !self.cdp.alive() {
                return;
            }
            std::thread::sleep(Duration::from_secs(3));
        });
    }

    fn call_page(&self, f: &str, args: &[Value]) {
        let a: Vec<String> = args.iter().map(|v| v.to_string()).collect();
        let expr = format!("window.{f} && window.{f}({})", a.join(","));
        let _ = self.cdp.call("Runtime.evaluate", json!({"expression": expr}), Some(&self.session));
    }

    fn handle(&self, req: &Value) -> Value {
        let r: Result<Value, String> = (|| match req["op"].as_str().unwrap_or("") {
            "core" => {
                let op = req["request"]["op"].as_str().unwrap_or("");
                if !CORE_OPS.contains(&op) {
                    return Err(format!("the Workspace can't ask the core for {op:?}"));
                }
                let core = self.core.as_ref().ok_or("the core isn't connected")?;
                let mut fields = req["request"].clone();
                if let Value::Object(m) = &mut fields {
                    m.remove("op");
                    // The Workspace is the human's own surface: what they click there is theirs.
                    m.insert("source".into(), json!("human"));
                }
                core.request(op, fields).map_err(|e| e.to_string())
            }
            "pages" => {
                let tabs: Vec<Value> = self.cdp.tabs().into_iter().filter(|t| t["tab"] != self.tab.as_str()).collect();
                Ok(json!({"ok": true, "pages": tabs}))
            }
            "open" => {
                // The human opens a page from the Workspace: theirs, shown inside the Workspace.
                let url = req["url"].as_str().filter(|u| u.starts_with("http://") || u.starts_with("https://")).ok_or("an http(s) address")?;
                let tab = self.cdp.call("Target.createTarget", json!({"url": url, "newWindow": true, "background": true}), None)
                    .map_err(|e| e.to_string())?["targetId"].as_str().unwrap_or("").to_string();
                Ok(json!({"ok": true, "tab": tab}))
            }
            "watch" => {
                let tab = req["tab"].as_str().ok_or("tab")?.to_string();
                if tab == self.tab || self.cdp.owner(&tab).is_none() {
                    return Err("no such page".into());
                }
                self.unwatch();
                let s = self.cdp.session(&tab).map_err(|e| e.to_string())?;
                let w = req["width"].as_u64().unwrap_or(1280).clamp(320, 2560);
                let h = req["height"].as_u64().unwrap_or(800).clamp(240, 1600);
                *self.watched.lock().unwrap() = Some((tab.clone(), s.clone()));
                self.cdp.call("Page.startScreencast", json!({"format": "jpeg", "quality": 70, "maxWidth": w, "maxHeight": h}), Some(&s))
                    .map_err(|e| e.to_string())?;
                Ok(json!({"ok": true, "tab": tab}))
            }
            "unwatch" => {
                self.unwatch();
                Ok(json!({"ok": true}))
            }
            "input" => {
                // The human, acting on the page shown in the Workspace (any page: it's their screen).
                let (tab, s) = self.watched.lock().unwrap().clone().ok_or("no page is shown")?;
                let x = req["x"].as_f64().unwrap_or(0.0);
                let y = req["y"].as_f64().unwrap_or(0.0);
                let call = |m: &str, p: Value| self.cdp.call(m, p, Some(&s)).map(|_| ()).map_err(|e| e.to_string());
                match req["kind"].as_str().unwrap_or("") {
                    "click" => {
                        call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x, "y": y}))?;
                        for t in ["mousePressed", "mouseReleased"] {
                            call("Input.dispatchMouseEvent", json!({"type": t, "x": x, "y": y, "button": "left", "clickCount": 1}))?;
                        }
                    }
                    "move" => call("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": x, "y": y}))?,
                    "scroll" => call("Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": x, "y": y,
                        "deltaX": req["dx"].as_f64().unwrap_or(0.0), "deltaY": req["dy"].as_f64().unwrap_or(0.0)}))?,
                    "text" => call("Input.insertText", json!({"text": req["text"].as_str().unwrap_or("")}))?,
                    "key" => press(&self.cdp, &s, req["key"].as_str().unwrap_or("Enter")).map_err(|e| e.to_string())?,
                    other => return Err(format!("unknown input {other:?}")),
                }
                Ok(json!({"ok": true, "tab": tab}))
            }
            other => Err(format!("unknown Workspace request {other:?}")),
        })();
        r.unwrap_or_else(|e| json!({"ok": false, "error": e}))
    }

    fn unwatch(&self) {
        if let Some((_, s)) = self.watched.lock().unwrap().take() {
            let _ = self.cdp.call("Page.stopScreencast", json!({}), Some(&s));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::find_browser;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    const WS: &str = r#"<!doctype html><title>Workspace</title><script>
      window.__r = {}; window.__frames = 0; window.__events = [];
      window.linuxaiReply = (id, r) => { window.__r[id] = r; };
      window.linuxaiFrame = (tab, data, meta) => { window.__frames++; window.__meta = meta; };
      window.linuxaiEvent = (e) => { window.__events.push(e.kind); };
    </script>"#;
    const PAGE: &str = r#"<!doctype html><title>Form</title><body style="margin:0">
      <button style="position:absolute;left:10px;top:10px;width:200px;height:60px" onclick="document.title='clicked'">Go</button>
      <input id=i style="position:absolute;left:10px;top:100px" onfocus="">"#;

    /// A stand-in core: answers each request with what it was asked.
    fn fake_core(path: &std::path::Path) {
        let l = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                let _ = r.read_line(&mut line);
                let req: Value = serde_json::from_str(&line).unwrap_or(json!({}));
                let mut w = s;
                let _ = writeln!(w, "{}", json!({"ok": true, "asked": req}));
            }
        });
    }

    fn wait_for(b: &Cdp, tab: &str, expr: &str) -> Value {
        let start = Instant::now();
        loop {
            let v = b.eval(tab, expr).unwrap_or(Value::Null);
            if !v.is_null() && v != json!(false) && v != json!(0) {
                return v;
            }
            if start.elapsed() > Duration::from_secs(10) {
                return v;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn the_workspace_talks_to_the_os_and_shows_pages_inside_it() {
        let Some(bin) = find_browser().or_else(|| {
            let p = "/opt/pw-browsers/chromium-1243/chrome-linux64/chrome";
            std::path::Path::new(p).exists().then(|| p.to_string())
        }) else { return };
        let dir = std::env::temp_dir().join(format!("lx-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ws.html"), WS).unwrap();
        std::fs::write(dir.join("page.html"), PAGE).unwrap();
        fake_core(&dir.join("core.sock"));
        let organ = Organ::new(bin, dir.join("profile"), true, Some(crate::client::Client::new(dir.join("core.sock"))));

        let ws_url = format!("file://{}", dir.join("ws.html").display());
        let r = organ.handle(&json!({"op": "workspace", "url": ws_url}));
        let ws = r["tab"].as_str().expect("workspace opened").to_string();
        let b = organ.browser().unwrap();
        assert!(organ.handle(&json!({"op": "tabs"}))["tabs"].as_array().unwrap().iter().all(|t| t["tab"] != ws.as_str()), "the Workspace isn't one of the pages");
        assert_eq!(organ.handle(&json!({"op": "go", "tab": ws, "url": "about:blank"}))["ok"], false, "the AI can't drive the Workspace");

        // The page asks the core (allowed ops only) through its private channel.
        b.eval(&ws, r#"linuxai(JSON.stringify({id: 1, op: "core", request: {op: "waiting"}}))"#).unwrap();
        let got = wait_for(&b, &ws, "window.__r[1]");
        assert_eq!(got["asked"]["op"], "waiting", "{got}");
        b.eval(&ws, r#"linuxai(JSON.stringify({id: 2, op: "core", request: {op: "submit", plan: {}}}))"#).unwrap();
        assert_eq!(wait_for(&b, &ws, "window.__r[2]")["ok"], false, "the Workspace can't submit plans");

        // A page shown inside the Workspace: live frames, and the human's clicks and typing reach the real page.
        let page = organ.handle(&json!({"op": "open", "url": format!("file://{}", dir.join("page.html").display())}));
        let tab = page["tab"].as_str().unwrap().to_string();
        b.eval(&ws, &format!(r#"linuxai(JSON.stringify({{id: 3, op: "watch", tab: {}, width: 800, height: 600}}))"#, json!(tab))).unwrap();
        assert_eq!(wait_for(&b, &ws, "window.__r[3]")["ok"], true);
        assert!(wait_for(&b, &ws, "window.__frames").as_u64().unwrap_or(0) > 0, "frames arrive");
        b.eval(&ws, r#"linuxai(JSON.stringify({id: 4, op: "input", kind: "click", x: 100, y: 40}))"#).unwrap();
        wait_for(&b, &ws, "window.__r[4]");
        assert_eq!(wait_for(&b, &tab, "document.title === 'clicked' && document.title"), "clicked", "the click reached the page");
        b.eval(&tab, "document.getElementById('i').focus()").unwrap();
        b.eval(&ws, r#"linuxai(JSON.stringify({id: 5, op: "input", kind: "text", text: "Dallas"}))"#).unwrap();
        wait_for(&b, &ws, "window.__r[5]");
        assert_eq!(wait_for(&b, &tab, "document.getElementById('i').value"), "Dallas");

        // If the Workspace tab ever shows something else, that page gets nothing.
        let s = b.session(&ws).unwrap();
        b.call("Page.navigate", json!({"url": format!("file://{}", dir.join("page.html").display())}), Some(&s)).unwrap();
        b.settle(&ws);
        b.eval(&ws, r#"window.__r = {}; window.linuxaiReply = (id, r) => { window.__r[id] = r; }; linuxai(JSON.stringify({id: 6, op: "pages"}))"#).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert!(b.eval(&ws, "window.__r[6]").unwrap().is_null(), "an outside page can't use the channel");
        organ.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
