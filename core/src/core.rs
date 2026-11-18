//! The running core: shared state, event dispatch to rules and waiting plans, and the API operations.

use crate::engine::{self, Bus, Ctx, Exec};
use crate::host;
use crate::memory::{now, Memory};
use crate::verbs::{self, Organs};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

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
    pub mem: Memory,
    pub organs: Box<dyn Organs>,
    pub bus: Bus,
    pub state: Mutex<Value>,
    pub subscribers: Mutex<Vec<Sender<Value>>>,
    pub threshold: f64,
    /// Run event- and timer-triggered work on its own threads (the daemon) instead of inline (tests).
    pub background: bool,
    /// Plans being advanced right now. A plan is only ever advanced by one thread at a time.
    active: Mutex<HashSet<String>>,
    /// Recent firings per rule, to stop a rule that keeps triggering itself.
    rule_fires: Mutex<HashMap<String, Vec<f64>>>,
    /// Where the playbooks (one markdown file per task category) live.
    pub playbook_dir: std::path::PathBuf,
    /// Group that owns playbook files, so the human can edit them directly.
    pub file_gid: Option<u32>,
    pub retention: crate::memory::Retention,
    /// Where the prompt library lives (markdown files the human reads and edits).
    pub prompt_dir: std::path::PathBuf,
}

const RULE_MAX_PER_MINUTE: usize = 10;

impl Core {
    pub fn new(mem: Memory, organs: Box<dyn Organs>) -> Self {
        for r in default_rules() {
            mem.insert_rule_if_missing(r["rule"].as_str().unwrap(), &r, "linuxai");
        }
        Core {
            mem,
            organs,
            bus: Bus::default(),
            state: Mutex::new(Value::Null),
            subscribers: Mutex::new(Vec::new()),
            threshold: 0.8,
            background: false,
            active: Mutex::new(HashSet::new()),
            rule_fires: Mutex::new(HashMap::new()),
            playbook_dir: std::env::var("LINUXAI_PLAYBOOKS").map(Into::into).unwrap_or_else(|_| "playbooks".into()),
            file_gid: None,
            retention: crate::memory::Retention::from_env(),
            prompt_dir: std::env::var("LINUXAI_PROMPTS").map(Into::into).unwrap_or_else(|_| "prompts".into()),
        }
    }

    fn with_ctx<T>(&self, f: impl FnOnce(&Ctx) -> T) -> T {
        let state = self.state.lock().unwrap().clone();
        let ctx = Ctx { mem: &self.mem, organs: self.organs.as_ref(), bus: &self.bus, state: &state };
        f(&ctx)
    }

    fn claim(&self, id: &str) -> bool {
        self.active.lock().unwrap().insert(id.to_string())
    }

    fn release(&self, id: &str) {
        self.active.lock().unwrap().remove(id);
    }

    /// Load a stored plan and advance it, holding its claim. None if it's busy or doesn't exist.
    fn advance(&self, id: &str, f: impl FnOnce(&mut Exec, &Ctx)) -> Result<Exec, &'static str> {
        if !self.claim(id) {
            return Err("that plan is busy right now; try again in a moment");
        }
        let out = self.with_ctx(|ctx| {
            let mut ex = ctx.mem.load_plan(id).and_then(|p| serde_json::from_value::<Exec>(p).ok()).ok_or("no such plan")?;
            f(&mut ex, ctx);
            Ok(ex)
        });
        self.release(id);
        out
    }

    /// Run a brand-new plan (claimed for the duration).
    fn run_new(&self, mut ex: Exec, f: impl FnOnce(&mut Exec, &Ctx)) -> Exec {
        self.claim(&ex.id);
        self.with_ctx(|ctx| f(&mut ex, ctx));
        self.release(&ex.id);
        ex
    }

    /// Do a piece of work now (inline) or on its own thread (background), then settle its events.
    fn dispatch(self: &Arc<Self>, job: impl FnOnce(&Arc<Core>) + Send + 'static) {
        if self.background {
            let c = self.clone();
            std::thread::spawn(move || {
                job(&c);
                c.settle();
            });
        } else {
            job(self);
        }
    }

    /// Broadcast events produced while running, then let rules and waiting plans react to them.
    pub(crate) fn settle(self: &Arc<Self>) {
        for _ in 0..200 {
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
    pub fn emit(self: &Arc<Self>, kind: &str, data: Value) {
        self.bus.emit(&self.mem, kind, data);
        self.settle();
    }

    /// False (and a one-time warning event) when a rule has fired too often in the last minute.
    fn rule_may_fire(&self, name: &str) -> bool {
        let t = now();
        let mut fires = self.rule_fires.lock().unwrap();
        let list = fires.entry(name.to_string()).or_default();
        list.retain(|x| t - x < 60.0);
        if list.len() >= RULE_MAX_PER_MINUTE {
            if list.len() == RULE_MAX_PER_MINUTE {
                list.push(t); // mark as warned
                self.bus.emit(&self.mem, "rule.throttled", json!({"rule": name, "per_minute": RULE_MAX_PER_MINUTE}));
            }
            return false;
        }
        list.push(t);
        true
    }

    fn react(self: &Arc<Self>, kind: &str, ev: &Value) {
        if matches!(kind, "plan.finished" | "plan.failed" | "plan.stopped") && ev["data"]["task"].is_string() {
            self.task_on_plan_end(kind, &ev["data"]);
        }
        if matches!(kind, "plan.finished" | "plan.failed" | "plan.stopped") && ev["data"]["proposal"].is_string() {
            self.playbook_on_plan_end(kind, &ev["data"]);
        }
        if kind.starts_with("plan.") || kind.starts_with("action.") || kind == "rule.throttled" {
            return; // bookkeeping events don't trigger rules
        }
        for (rule, enabled, _) in self.mem.rules() {
            if !enabled || !engine::event_matches(rule["when"].as_str().unwrap_or(""), kind) {
                continue;
            }
            let name = rule["rule"].as_str().unwrap_or("").to_string();
            if !self.rule_may_fire(&name) {
                continue;
            }
            let ev = ev.clone();
            self.dispatch(move |c| {
                let mut plan = rule.clone();
                plan["plan"] = json!(name);
                let mut ex = engine::new_exec(&plan, &format!("rule:{name}"));
                ex.vars.insert("event".into(), ev);
                let go = match rule.get("where") {
                    Some(cond) => c.with_ctx(|ctx| engine::eval(cond, &ex.vars, ctx).map(|v| engine::truthy(&v)).unwrap_or(false)),
                    None => true,
                };
                if go {
                    c.run_new(ex, |ex, ctx| {
                        ctx.bus.emit(ctx.mem, "plan.started", ex.summary());
                        engine::run(ex, ctx);
                    });
                }
            });
        }
        for st in self.mem.plans_with_status("waiting:event") {
            if !engine::event_matches(st["wait"]["kind"].as_str().unwrap_or(""), kind) {
                continue;
            }
            let (id, kind, ev) = (st["id"].as_str().unwrap_or("").to_string(), kind.to_string(), ev.clone());
            self.dispatch(move |c| {
                let _ = c.advance(&id, |ex, ctx| engine::on_event(ex, &kind, &ev, ctx));
            });
        }
    }

    /// After a restart: carry on with every plan that was mid-way. Steps already carried out are not
    /// repeated (each action's idempotency key is in the journal).
    pub fn recover(self: &Arc<Self>) {
        for st in self.mem.plans_with_status("running") {
            let id = st["id"].as_str().unwrap_or("").to_string();
            self.dispatch(move |c| {
                let _ = c.advance(&id, |ex, ctx| {
                    ctx.bus.emit(ctx.mem, "plan.resumed", ex.summary());
                    engine::run(ex, ctx);
                });
            });
        }
        self.settle();
        self.task_recover();
        self.settle();
    }

    pub fn tick(self: &Arc<Self>) {
        let t = now();
        for st in self.mem.plans_with_status("waiting:") {
            let w = &st["wait"];
            let reminder_due = w["reminders"].as_array().map(|a| a.iter().filter_map(Value::as_f64).any(|r| r <= t)).unwrap_or(false);
            let deadline_due = w["until"].as_f64().map(|u| u <= t).unwrap_or(false);
            if !(reminder_due || deadline_due) {
                continue;
            }
            let id = st["id"].as_str().unwrap_or("").to_string();
            self.dispatch(move |c| {
                let _ = c.advance(&id, |ex, ctx| engine::on_tick(ex, t, ctx));
            });
        }
        self.settle();
    }

    /// One pass of the senses.
    pub fn sense(self: &Arc<Self>, full: bool) {
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

    pub fn handle(self: &Arc<Self>, req: &Value) -> Value {
        let op = req["op"].as_str().unwrap_or("");
        match op {
            "ping" => json!({"ok": true, "version": env!("CARGO_PKG_VERSION")}),
            "state" => json!({"ok": true, "state": *self.state.lock().unwrap()}),
            "verbs" => json!({"ok": true, "verbs": verbs::registry_json()}),
            "check" => {
                let source = req["source"].as_str().unwrap_or("ai");
                let nov = if source == "ai" { self.novelty(&req["plan"]) } else { None };
                let r = engine::check_with(&req["plan"], &self.mem, source, self.threshold, nov.as_deref());
                json!({"ok": r.ok, "report": r})
            }
            "submit" => self.submit(&req["plan"], req["source"].as_str().unwrap_or("ai")),
            "answer" => {
                let text = req["text"].as_str().unwrap_or("").to_string();
                self.resume(req, |ex, ctx| engine::answer(ex, &text, ctx))
            }
            "resolve" => self.resolve(req),
            "plan" => match self.mem.load_plan(req["plan_id"].as_str().unwrap_or("")) {
                Some(p) => json!({"ok": true, "plan": p}),
                None => json!({"ok": false, "error": "no such plan"}),
            },
            "waiting" => {
                let mut list: Vec<Value> = self.mem.plans_with_status("waiting:").into_iter()
                    .chain(self.mem.plans_with_status("needs_thought"))
                    .map(|p| json!({"plan_id": p["id"], "plan": p["name"], "status": p["status"], "wait": p["wait"],
                                    "source": p["source"]}))
                    .collect();
                list.sort_by(|a, b| a["plan_id"].as_str().cmp(&b["plan_id"].as_str()));
                json!({"ok": true, "plans": list})
            }
            "journal" => json!({"ok": true, "journal": self.mem.journal(req["limit"].as_i64().unwrap_or(30))}),
            "events" => json!({"ok": true,
                "events": self.mem.events_since(req["since"].as_i64().unwrap_or(0), req["limit"].as_i64().unwrap_or(100)),
                "last": self.mem.last_event_id()}),
            "rules" => {
                let rules: Vec<Value> = self.mem.rules().into_iter()
                    .map(|(r, enabled, source)| json!({"rule": r, "enabled": enabled, "source": source}))
                    .collect();
                json!({"ok": true, "rules": rules})
            }
            "add_rule" => self.add_rule(&req["rule"], req["source"].as_str().unwrap_or("ai")),
            "enable_rule" => {
                // Turning a rule OFF is always allowed; turning one on from here is the human's call
                // (the AI's way to turn one on is a plan, which asks for approval).
                let on = req["enabled"].as_bool().unwrap_or(true);
                if on && req["source"].as_str().unwrap_or("ai") == "ai" {
                    json!({"ok": false, "error": "the AI turns rules on through a plan (enable_rule), which asks the human"})
                } else {
                    json!({"ok": self.mem.set_rule_enabled(req["name"].as_str().unwrap_or(""), on)})
                }
            }
            "mandates" => json!({"ok": true, "mandates": self.mem.mandates()
                                   .into_iter().map(|(v, t)| json!({"verb": v, "target": t})).collect::<Vec<_>>()}),
            "save_plan" => {
                let name = req["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    json!({"ok": false, "error": "name required"})
                } else {
                    self.mem.save_named(name, &req["plan"]);
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
            "prune" => {
                let r = self.mem.prune(&self.retention, req["dry_run"].as_bool().unwrap_or(false));
                if !req["dry_run"].as_bool().unwrap_or(false) {
                    self.emit("memory.pruned", r.clone());
                }
                json!({"ok": true, "pruned": r})
            }
            "stats" => json!({"ok": true, "stats": self.mem.stats(), "active_plans": self.active.lock().unwrap().len(),
                              "subscribers": self.subscribers.lock().unwrap().len()}),
            "sense" => {
                self.sense(true);
                json!({"ok": true, "state": *self.state.lock().unwrap()})
            }
            op if op.starts_with("task") => self.task_op(op, req),
            op if op.starts_with("playbook") => self.playbook_op(op, req),
            op if op.starts_with("prompt") => self.prompt_op(op, req),
            "object_put" | "find" => self.object_op(op, req),
            "object_patch" => json!({"ok": self.mem.patch_object(req["id"].as_str().unwrap_or(""), &req["fields"])}),
            "object" => match self.mem.object(req["id"].as_str().unwrap_or("")) {
                Some(o) => json!({"ok": true, "object": o}),
                None => json!({"ok": false, "error": "no such object"}),
            },
            _ => json!({"ok": false, "error": format!("unknown op '{op}'")}),
        }
    }

    /// A plan for a task that came from outside information carries the outside label, whatever the AI says.
    pub fn with_inherited_taint(&self, plan: &Value) -> Value {
        let task_outside = plan["task"].as_str().and_then(|t| self.mem.load_task(t)).map(|t| t["trust"] == "outside").unwrap_or(false);
        if task_outside && plan["taint"] != "outside" {
            let mut p = plan.clone();
            p["taint"] = json!("outside");
            return p;
        }
        plan.clone()
    }

    /// Some(reason) when a plan is a first-time situation: its category (or its task's) has no playbook and
    /// no past cases, in that category or a broader or narrower one.
    pub fn novelty(&self, plan: &Value) -> Option<String> {
        let cat = plan["category"].as_str().map(String::from)
            .or_else(|| plan["task"].as_str().and_then(|t| self.mem.load_task(t)).and_then(|t| t["category"].as_str().map(String::from)))?;
        if cat == "general" {
            return None;
        }
        if self.playbook_chain(&cat).as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            return None;
        }
        let related = |c: &str| c == cat || cat.starts_with(&format!("{c}/")) || c.starts_with(&format!("{cat}/"));
        if self.mem.objects_of("case", 5000).iter().any(|o| o["category"].as_str().map(related).unwrap_or(false)) {
            return None;
        }
        Some(format!("this is the first time I'm handling {cat}"))
    }

    pub fn submit(self: &Arc<Self>, plan: &Value, source: &str) -> Value {
        let plan = &self.with_inherited_taint(plan);
        let nov = if source == "ai" { self.novelty(plan) } else { None };
        let report = engine::check_with(plan, &self.mem, source, self.threshold, nov.as_deref());
        if !report.ok {
            return json!({"ok": false, "report": report});
        }
        let ex = self.run_new(engine::new_exec(plan, source), |ex, ctx| engine::start(ex, &report, ctx));
        self.settle();
        json!({"ok": true, "result": ex.summary(), "vars": ex.vars, "explain": report.explain})
    }

    fn resume(self: &Arc<Self>, req: &Value, f: impl FnOnce(&mut Exec, &Ctx)) -> Value {
        let r = match self.advance(req["plan_id"].as_str().unwrap_or(""), f) {
            Ok(ex) => json!({"ok": true, "result": ex.summary(), "vars": ex.vars}),
            Err(e) => json!({"ok": false, "error": e}),
        };
        self.settle();
        r
    }

    /// The AI's judgment for a plan stopped at `think`. New steps are checked like any plan, and
    /// actions the AI isn't trusted with yet go to the human first.
    fn resolve(self: &Arc<Self>, req: &Value) -> Value {
        let id = req["plan_id"].as_str().unwrap_or("");
        let steps = req.get("steps").and_then(Value::as_array).cloned().filter(|s| !s.is_empty());
        let report = match &steps {
            Some(s) => {
                let stored = self.mem.load_plan(id).unwrap_or(json!({}));
                let res = json!({"plan": "resolution", "with": stored["vars"], "steps": s, "assess": req["assess"],
                                 "reasoning": req["reasoning"], "category": stored["plan"]["category"], "task": stored["plan"]["task"],
                                 "taint": if stored["plan"]["taint"] == "outside" || req["taint"] == "outside" { json!("outside") } else { Value::Null }});
                let nov = self.novelty(&res);
                let r = engine::check_with(&res, &self.mem, "ai", self.threshold, nov.as_deref());
                if !r.ok {
                    return json!({"ok": false, "report": r});
                }
                Some(r)
            }
            None => None,
        };
        self.resume(req, |ex, ctx| engine::resolve_thought(ex, steps, report.as_ref(), ctx))
    }

    fn add_rule(self: &Arc<Self>, rule: &Value, source: &str) -> Value {
        let name = rule["rule"].as_str().unwrap_or("");
        if name.is_empty() || rule["when"].as_str().unwrap_or("").is_empty() {
            return json!({"ok": false, "error": "a rule needs \"rule\" (name), \"when\" (event) and \"steps\""});
        }
        let mut as_plan = rule.clone();
        as_plan["plan"] = json!(name);
        let report = engine::check(&as_plan, &self.mem, "rule", self.threshold);
        if !report.ok {
            return json!({"ok": false, "report": report});
        }
        // Rules run unattended with full authority, so one the AI writes stays off until the human agrees.
        let from_ai = source == "ai";
        self.mem.put_rule(name, rule, source, !from_ai);
        self.emit("rule.added", json!({"rule": name, "source": source, "enabled": !from_ai, "explain": report.explain}));
        if from_ai {
            let what = format!("when {}: {}", rule["when"].as_str().unwrap_or(""), report.explain.join("; "));
            let ask = json!({"plan": format!("turn on the rule \"{name}\""), "why": what,
                             "assess": {"confidence": 0.5,
                                        "consequences": [format!("from now on this runs by itself, with no model and no asking: {what}")],
                                        "outcomes": [{"if": "it fires when it shouldn't", "then": "turn the rule off; nothing else depends on it"}]},
                             "steps": [{"do": "enable_rule", "name": name}]});
            let r = self.submit(&ask, "ai");
            return json!({"ok": true, "enabled": false, "approval": r["result"], "explain": report.explain});
        }
        json!({"ok": true, "enabled": true, "explain": report.explain})
    }
}
