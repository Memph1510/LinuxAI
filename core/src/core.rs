//! The running core: shared state, event dispatch to rules and waiting plans, and the API operations.

use crate::engine::{self, Bus, Ctx, Exec};
use crate::host;
use crate::memory::{now, Memory};
use crate::verbs::{self, Organs};
use serde_json::{json, Value};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// Rules LinuxAI ships with. They are the starting reflexes for running the machine; the AI adds
/// its own as it learns, and any of them can be switched off.
pub fn default_rules() -> Vec<Value> {
    vec![
        json!({"rule": "unplugged-save-power", "when": "power.unplugged",
               "why": "on battery: stretch it",
               "steps": [
                 {"try": [{"do": "set", "what": "power.profile", "to": "power-saver"}], "else": []},
                 {"try": [{"do": "set", "what": "cpu.policy", "to": "efficiency"}], "else": []}]}),
        json!({"rule": "plugged-in-balanced", "when": "power.plugged",
               "why": "on wall power: back to normal",
               "steps": [
                 {"try": [{"do": "set", "what": "power.profile", "to": "balanced"}], "else": []},
                 {"try": [{"do": "set", "what": "cpu.policy", "to": "balanced"}], "else": []}]}),
        json!({"rule": "battery-low-warn", "when": "battery.low",
               "steps": [{"tell": "Battery is at {$event.data.percent}%. I've switched to power saving; plug in soon."},
                         {"try": [{"do": "set", "what": "power.profile", "to": "power-saver"}], "else": []}]}),
        json!({"rule": "service-failed-investigate", "when": "service.failed",
               "steps": [{"think": "The service {$event.data.unit} just failed. Find out why and fix it or tell the human."}]}),
        json!({"rule": "memory-pressure-investigate", "when": "memory.pressure",
               "steps": [{"think": "Memory pressure is high ({$event.data.available_mb} MB available). Find what is using memory and decide what to do."}]}),
        json!({"rule": "too-hot", "when": "temperature.high",
               "steps": [{"try": [{"do": "set", "what": "power.profile", "to": "power-saver"}], "else": []},
                         {"tell": "The machine is running hot ({$event.data.celsius}°C). I've lowered power use."}]}),
    ]
}

pub struct Core {
    pub mem: Mutex<Memory>,
    pub organs: Box<dyn Organs>,
    pub bus: Bus,
    pub state: Mutex<Value>,
    pub subscribers: Mutex<Vec<Sender<Value>>>,
    pub threshold: f64,
}

impl Core {
    pub fn new(mem: Memory, organs: Box<dyn Organs>) -> Self {
        for r in default_rules() {
            mem.insert_rule_if_missing(r["rule"].as_str().unwrap(), &r, "linuxai");
        }
        Core {
            mem: Mutex::new(mem),
            organs,
            bus: Bus::default(),
            state: Mutex::new(Value::Null),
            subscribers: Mutex::new(Vec::new()),
            threshold: 0.8,
        }
    }

    fn with_ctx<T>(&self, f: impl FnOnce(&Ctx) -> T) -> T {
        let mem = self.mem.lock().unwrap();
        let state = self.state.lock().unwrap().clone();
        let ctx = Ctx { mem: &mem, organs: self.organs.as_ref(), bus: &self.bus, state: &state, threshold: self.threshold };
        let out = f(&ctx);
        drop(mem);
        out
    }

    /// Broadcast events produced while running, then let rules and waiting plans react to them.
    fn settle(&self) {
        for _ in 0..50 {
            let pending = self.bus.drain();
            if pending.is_empty() {
                return;
            }
            for (kind, ev) in pending {
                self.broadcast(&ev);
                self.react(&kind, &ev);
            }
        }
    }

    fn broadcast(&self, ev: &Value) {
        self.subscribers.lock().unwrap().retain(|tx| tx.send(ev.clone()).is_ok());
    }

    /// An event from outside (senses, organs, the human).
    pub fn emit(&self, kind: &str, data: Value) {
        {
            let mem = self.mem.lock().unwrap();
            self.bus.emit(&mem, kind, data);
        }
        self.settle();
    }

    fn react(&self, kind: &str, ev: &Value) {
        if kind.starts_with("plan.") || kind.starts_with("action.") {
            return; // bookkeeping events don't trigger rules
        }
        self.with_ctx(|ctx| {
            for (rule, enabled, _) in ctx.mem.rules() {
                if !enabled || !engine::event_matches(rule["when"].as_str().unwrap_or(""), kind) {
                    continue;
                }
                let mut plan = rule.clone();
                plan["plan"] = rule["rule"].clone();
                let mut ex = engine::new_exec(&plan, &format!("rule:{}", rule["rule"].as_str().unwrap_or("")));
                ex.vars.insert("event".into(), ev.clone());
                if let Some(cond) = rule.get("where") {
                    match engine::eval(cond, &ex.vars, ctx) {
                        Ok(v) if engine::truthy(&v) => {}
                        _ => continue,
                    }
                }
                ctx.bus.emit(ctx.mem, "plan.started", ex.summary());
                engine::run(&mut ex, ctx);
            }
            for st in ctx.mem.plans_with_status("waiting:event") {
                if let Ok(mut ex) = serde_json::from_value::<Exec>(st) {
                    engine::on_event(&mut ex, kind, ev, ctx);
                }
            }
        });
    }

    pub fn tick(&self) {
        let t = now();
        self.with_ctx(|ctx| {
            for st in ctx.mem.plans_with_status("waiting:") {
                if let Ok(mut ex) = serde_json::from_value::<Exec>(st) {
                    engine::on_tick(&mut ex, t, ctx);
                }
            }
        });
        self.settle();
    }

    /// One pass of the senses.
    pub fn sense(&self, full: bool) {
        let prev = self.state.lock().unwrap().clone();
        let cur = host::snapshot(full, &prev);
        let events = host::derive_events(&prev, &cur);
        *self.state.lock().unwrap() = cur;
        for (kind, data) in events {
            self.emit(&kind, data);
        }
    }

    // ---------------------------------------------------------------------------------------
    // API operations
    // ---------------------------------------------------------------------------------------

    pub fn handle(&self, req: &Value) -> Value {
        let op = req["op"].as_str().unwrap_or("");
        let out = match op {
            "ping" => json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}),
            "state" => json!({"ok": true, "state": *self.state.lock().unwrap()}),
            "verbs" => json!({"ok": true, "verbs": verbs::registry_json()}),
            "check" => {
                let mem = self.mem.lock().unwrap();
                let source = req["source"].as_str().unwrap_or("ai");
                let r = engine::check(&req["plan"], &mem, source, self.threshold);
                json!({"ok": r.ok, "report": r})
            }
            "submit" => self.submit(&req["plan"], req["source"].as_str().unwrap_or("ai")),
            "answer" => self.resume(req, |ex, ctx| engine::answer(ex, req["text"].as_str().unwrap_or(""), ctx)),
            "resolve" => self.resume(req, |ex, ctx| {
                let steps = req.get("steps").and_then(Value::as_array).cloned();
                engine::resolve_thought(ex, steps, ctx)
            }),
            "plan" => {
                let mem = self.mem.lock().unwrap();
                match mem.load_plan(req["plan_id"].as_str().unwrap_or("")) {
                    Some(p) => json!({"ok": true, "plan": p}),
                    None => json!({"ok": false, "error": "no such plan"}),
                }
            }
            "waiting" => {
                let mem = self.mem.lock().unwrap();
                let mut list: Vec<Value> = mem.plans_with_status("waiting:").into_iter()
                    .chain(mem.plans_with_status("needs_thought"))
                    .map(|p| json!({"plan_id": p["id"], "plan": p["name"], "status": p["status"], "wait": p["wait"],
                                    "source": p["source"]}))
                    .collect();
                list.sort_by(|a, b| a["plan_id"].as_str().cmp(&b["plan_id"].as_str()));
                json!({"ok": true, "plans": list})
            }
            "journal" => json!({"ok": true, "journal": self.mem.lock().unwrap().journal(req["limit"].as_i64().unwrap_or(30))}),
            "events" => {
                let mem = self.mem.lock().unwrap();
                json!({"ok": true, "events": mem.events_since(req["since"].as_i64().unwrap_or(0), req["limit"].as_i64().unwrap_or(100)),
                       "last": mem.last_event_id()})
            }
            "rules" => {
                let rules: Vec<Value> = self.mem.lock().unwrap().rules().into_iter()
                    .map(|(r, enabled, source)| json!({"rule": r, "enabled": enabled, "source": source}))
                    .collect();
                json!({"ok": true, "rules": rules})
            }
            "add_rule" => self.add_rule(&req["rule"], req["source"].as_str().unwrap_or("ai")),
            "enable_rule" => {
                let ok = self.mem.lock().unwrap().set_rule_enabled(req["name"].as_str().unwrap_or(""), req["enabled"].as_bool().unwrap_or(true));
                json!({"ok": ok})
            }
            "mandates" => json!({"ok": true, "mandates": self.mem.lock().unwrap().mandates()
                                   .into_iter().map(|(v, t)| json!({"verb": v, "target": t})).collect::<Vec<_>>()}),
            "save_plan" => {
                let name = req["name"].as_str().unwrap_or("");
                if name.is_empty() { json!({"ok": false, "error": "name required"}) } else {
                    self.mem.lock().unwrap().save_named(name, &req["plan"]);
                    json!({"ok": true})
                }
            }
            "event" => {
                // An organ (mail, sms, a test harness...) reporting something that happened.
                let kind = req["kind"].as_str().unwrap_or("");
                if kind.is_empty() || kind.starts_with("plan.") || kind.starts_with("action.") {
                    json!({"ok": false, "error": "event needs a kind (plan.* and action.* are reserved)"})
                } else {
                    self.emit(kind, req.get("data").cloned().unwrap_or(json!({})));
                    json!({"ok": true})
                }
            }
            "tell" => {
                // The AI (outside a plan) telling the human something.
                self.emit("ai.tell", json!({"text": req["text"], "from": req["from"]}));
                json!({"ok": true})
            }
            "say" => {
                // The human said something; an event others (rules, waiting plans) can react to.
                self.emit("human.said", json!({"text": req["text"]}));
                json!({"ok": true})
            }
            "sense" => {
                self.sense(true);
                json!({"ok": true, "state": *self.state.lock().unwrap()})
            }
            _ => json!({"ok": false, "error": format!("unknown op '{op}'")}),
        };
        out
    }

    pub fn submit(&self, plan: &Value, source: &str) -> Value {
        let result = self.with_ctx(|ctx| {
            let report = engine::check(plan, ctx.mem, source, ctx.threshold);
            if !report.ok {
                return json!({"ok": false, "report": report});
            }
            let mut ex = engine::new_exec(plan, source);
            engine::start(&mut ex, &report, ctx);
            json!({"ok": true, "result": ex.summary(), "vars": ex.vars, "explain": report.explain})
        });
        self.settle();
        result
    }

    fn resume(&self, req: &Value, f: impl FnOnce(&mut Exec, &Ctx)) -> Value {
        let id = req["plan_id"].as_str().unwrap_or("");
        let result = self.with_ctx(|ctx| match ctx.mem.load_plan(id).and_then(|p| serde_json::from_value::<Exec>(p).ok()) {
            Some(mut ex) => {
                f(&mut ex, ctx);
                json!({"ok": true, "result": ex.summary(), "vars": ex.vars})
            }
            None => json!({"ok": false, "error": "no such plan"}),
        });
        self.settle();
        result
    }

    fn add_rule(&self, rule: &Value, source: &str) -> Value {
        let name = rule["rule"].as_str().unwrap_or("");
        if name.is_empty() || rule["when"].as_str().unwrap_or("").is_empty() {
            return json!({"ok": false, "error": "a rule needs \"rule\" (name), \"when\" (event) and \"steps\""});
        }
        let mem = self.mem.lock().unwrap();
        let mut as_plan = rule.clone();
        as_plan["plan"] = json!(name);
        // Rules run unattended, so their effects are checked as if a human approved them.
        let report = engine::check(&as_plan, &mem, "rule", self.threshold);
        if !report.ok {
            return json!({"ok": false, "report": report});
        }
        mem.put_rule(name, rule, source, true);
        drop(mem);
        self.emit("rule.added", json!({"rule": name, "source": source, "explain": report.explain}));
        json!({"ok": true, "explain": report.explain})
    }
}
