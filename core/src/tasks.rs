//! Tasks, micro and macro: a task tree whose leaves are either work (done by the AI through plans) or a
//! human step (action, evaluation, critique, decision) that reaches the human and continues when they
//! respond. Plus the knowledge that governs how tasks are done: playbooks (markdown, per category) and
//! cases (each real situation: its variables, what was decided, how it turned out).

use crate::core::Core;
use crate::engine;
use crate::memory::new_id;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub const HUMAN_KINDS: &[&str] = &["action", "evaluation", "critique", "decision"];
const FINISHED: &[&str] = &["done", "failed", "cancelled"];

/// What the human is asked for each kind of step, when the task doesn't say.
fn default_ask(kind: &str, title: &str) -> (String, Value) {
    match kind {
        "action" => (format!("Can you do this: {title}? Tell me when it's done."), json!(["done", "can't"])),
        "evaluation" => (format!("How does this look: {title}?"), json!(["good", "needs work"])),
        "critique" => (format!("How would you improve this: {title}?"), json!([])),
        _ => (format!("Your decision: {title}"), json!([])),
    }
}

fn valid_category(c: &str) -> bool {
    !c.is_empty() && c.len() <= 120
        && c.split('/').all(|seg| !seg.is_empty() && seg.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
}

impl Core {
    // -----------------------------------------------------------------------------------------
    // Creating
    // -----------------------------------------------------------------------------------------

    /// Create a task tree. Each node: {title, category?, kind? (work|action|evaluation|critique|decision),
    /// context?, plan? (work), ask? (human steps), key?, after? [sibling keys], subtasks? [...]}.
    pub fn task_create(self: &Arc<Self>, spec: &Value, source: &str) -> Value {
        let mut rows = Vec::new();
        let root = match self.build_task(spec, None, None, &mut rows) {
            Ok(id) => id,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        // Plans written into the task run with the authority of whoever created it. A task that came from
        // outside information keeps that label on every subtask and plan.
        let source = if source == "human" { "human" } else { "ai" };
        let outside = spec["trust"] == "outside";
        for r in &mut rows {
            r["source"] = json!(source);
            if outside {
                r["trust"] = json!("outside");
            }
            self.mem.save_task(r);
        }
        self.bus.emit(&self.mem, "task.created", json!({"task": root, "title": spec["title"], "count": rows.len()}));
        self.task_advance(&root);
        self.settle();
        json!({"ok": true, "task": root, "tree": self.task_tree(&root)})
    }

    fn build_task(&self, spec: &Value, parent: Option<&str>, inherited_cat: Option<&str>, rows: &mut Vec<Value>) -> Result<String, String> {
        let title = spec["title"].as_str().unwrap_or("").trim();
        if title.is_empty() {
            return Err("every task needs a title".into());
        }
        let kind = spec["kind"].as_str().unwrap_or("work");
        if kind != "work" && !HUMAN_KINDS.contains(&kind) {
            return Err(format!("task '{title}': kind must be work, action, evaluation, critique or decision"));
        }
        let category = spec["category"].as_str().or(inherited_cat).unwrap_or("general");
        if !valid_category(category) {
            return Err(format!("task '{title}': category '{category}' should look like freight/negotiate"));
        }
        let subs = spec["subtasks"].as_array().cloned().unwrap_or_default();
        if !subs.is_empty() && kind != "work" {
            return Err(format!("task '{title}': a {kind} step can't have subtasks"));
        }
        let id = new_id("t");
        let mut row = json!({
            "id": id, "parent": parent, "title": title, "kind": kind, "category": category,
            "context": spec.get("context").cloned().unwrap_or(json!({})),
            "plan": spec.get("plan").cloned(), "ask": spec.get("ask").cloned(),
            "key": spec.get("key").cloned(), "after_keys": spec.get("after").cloned().unwrap_or(json!([])),
            "depends": [], "status": "pending", "result": null, "error": null, "plan_id": null,
        });
        let first = rows.len();
        rows.push(Value::Null); // placeholder keeps parents before children
        let mut child_ids = Vec::new();
        for s in &subs {
            child_ids.push(self.build_task(s, Some(&id), Some(category), rows)?);
        }
        // Resolve sibling dependencies by key.
        let keyed: Vec<(String, String)> = rows.iter().filter(|r| r["parent"] == json!(id))
            .filter_map(|r| Some((r["key"].as_str()?.to_string(), r["id"].as_str()?.to_string()))).collect();
        for r in rows.iter_mut().filter(|r| r["parent"] == json!(id)) {
            let mut deps = Vec::new();
            for k in r["after_keys"].as_array().cloned().unwrap_or_default() {
                let k = k.as_str().unwrap_or("");
                match keyed.iter().find(|(key, _)| key == k) {
                    Some((_, dep)) => deps.push(json!(dep)),
                    None => return Err(format!("task '{}': no sibling with key '{k}'", r["title"].as_str().unwrap_or(""))),
                }
            }
            r["depends"] = json!(deps);
        }
        row["children"] = json!(child_ids);
        rows[first] = row;
        Ok(id)
    }

    // -----------------------------------------------------------------------------------------
    // Advancing
    // -----------------------------------------------------------------------------------------

    /// Move a task forward as far as it can go right now. Safe to call any number of times.
    pub fn task_advance(self: &Arc<Self>, id: &str) {
        let Some(t) = self.mem.load_task(id) else { return };
        let status = t["status"].as_str().unwrap_or("");
        if FINISHED.contains(&status) {
            return;
        }
        // Dependencies on siblings.
        let deps: Vec<Value> = t["depends"].as_array().cloned().unwrap_or_default().iter()
            .filter_map(|d| self.mem.load_task(d.as_str().unwrap_or(""))).collect();
        if let Some(bad) = deps.iter().find(|d| matches!(d["status"].as_str(), Some("failed") | Some("cancelled"))) {
            self.task_finish(id, &["pending"], "failed", json!({"error": format!("depends on '{}', which {}", bad["title"].as_str().unwrap_or(""), bad["status"].as_str().unwrap_or(""))}));
            return;
        }
        let ready = deps.iter().all(|d| d["status"] == "done");
        let children = self.mem.task_children(id);
        if !children.is_empty() {
            if children.iter().all(|c| c["status"] == "done") {
                let results: Map<String, Value> = children.iter()
                    .map(|c| (c["key"].as_str().or(c["title"].as_str()).unwrap_or("").to_string(), c["result"].clone())).collect();
                self.task_finish(id, &["pending", "running"], "done", json!({"result": results}));
                return;
            }
            if let Some(bad) = children.iter().find(|c| matches!(c["status"].as_str(), Some("failed"))) {
                self.task_finish(id, &["pending", "running"], "failed",
                                 json!({"error": format!("'{}' failed: {}", bad["title"].as_str().unwrap_or(""), bad["error"].as_str().unwrap_or(""))}));
                return;
            }
            if status == "pending" && ready {
                if self.mem.transition_task(id, &["pending"], "running", &json!({})).is_some() {
                    self.bus.emit(&self.mem, "task.started", json!({"task": id, "title": t["title"]}));
                }
            }
            if self.mem.load_task(id).map(|x| x["status"] == "running").unwrap_or(false) {
                for c in children {
                    self.task_advance(c["id"].as_str().unwrap_or(""));
                }
            }
            return;
        }
        if status != "pending" || !ready {
            return;
        }
        // A leaf that's ready to start.
        let kind = t["kind"].as_str().unwrap_or("work");
        let title = t["title"].as_str().unwrap_or("").to_string();
        if HUMAN_KINDS.contains(&kind) {
            let Some(t) = self.mem.transition_task(id, &["pending"], "waiting", &json!({})) else { return };
            let ask = t["ask"].clone();
            let (q, opts) = default_ask(kind, &title);
            let mut step = json!({"ask": ask["question"].as_str().map(String::from).unwrap_or(q),
                                  "opts": ask.get("opts").cloned().unwrap_or(opts), "as": "answer"});
            for k in ["why", "lean", "remind", "deadline", "fallback"] {
                if let Some(v) = ask.get(k) {
                    step[k] = v.clone();
                }
            }
            let plan = json!({"plan": title, "why": format!("{kind} needed for a task"), "task": id, "conf": 1.0, "steps": [step]});
            self.task_run_plan(id, plan);
        } else if let Some(plan) = t["plan"].as_object().cloned() {
            if self.mem.transition_task(id, &["pending"], "running", &json!({})).is_none() {
                return;
            }
            let mut plan = Value::Object(plan);
            plan["task"] = json!(id);
            if plan["plan"].as_str().unwrap_or("").is_empty() {
                plan["plan"] = json!(title);
            }
            self.task_run_plan(id, plan);
        } else if let Some(t) = self.mem.transition_task(id, &["pending"], "needs_plan", &json!({})) {
            // The AI works out how to do it: it gets the task, its category's playbooks and similar cases.
            let cat = t["category"].as_str().unwrap_or("general").to_string();
            self.bus.emit(&self.mem, "task.needs_plan", json!({"task": id, "title": title, "category": cat,
                "context": t["context"], "playbooks": self.playbook_chain(&cat)}));
        }
    }

    fn task_run_plan(self: &Arc<Self>, id: &str, plan: Value) {
        let source = self.mem.load_task(id).and_then(|t| t["source"].as_str().map(String::from)).unwrap_or_else(|| "ai".into());
        // Human steps are only questions, so they never need approval whoever created the task.
        let r = self.submit(&plan, &source);
        if r["ok"] == true && r["result"]["status"] == "waiting:answer" {
            // The plan needs the human (an approval or a question): the task is waiting on them.
            let _ = self.mem.transition_task(id, &["running"], "waiting", &json!({}));
        }
        if r["ok"] == true {
            if let Some(mut t) = self.mem.load_task(id) {
                t["plan_id"] = r["result"]["plan_id"].clone();
                // Only record the link if the plan hasn't already finished the task meanwhile.
                if !FINISHED.contains(&t["status"].as_str().unwrap_or("")) {
                    let _ = self.mem.transition_task(id, &["running", "waiting"], t["status"].as_str().unwrap_or("running"),
                                                     &json!({"plan_id": t["plan_id"]}));
                }
            }
        } else {
            let errs = r["report"]["errors"].as_array().map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default();
            // Back to the AI to plan again.
            if self.mem.transition_task(id, &["running", "waiting"], "needs_plan", &json!({"error": errs})).is_some() {
                let t = self.mem.load_task(id).unwrap_or(json!({}));
                self.bus.emit(&self.mem, "task.needs_plan", json!({"task": id, "title": t["title"], "category": t["category"],
                    "context": t["context"], "problem": errs, "playbooks": self.playbook_chain(t["category"].as_str().unwrap_or("general"))}));
            }
        }
    }

    fn task_finish(self: &Arc<Self>, id: &str, from: &[&str], to: &str, patch: Value) {
        let Some(t) = self.mem.transition_task(id, from, to, &patch) else { return };
        let kind = if to == "done" { "task.done" } else { "task.failed" };
        self.bus.emit(&self.mem, kind, json!({"task": id, "title": t["title"], "category": t["category"],
            "kind": t["kind"], "result": t["result"], "error": t["error"], "parent": t["parent"], "evidence": t["evidence"]}));
        if let Some(p) = t["parent"].as_str() {
            self.task_advance(p);
        }
    }

    /// A plan that belongs to a task ended: the task ends with it.
    pub fn task_on_plan_end(self: &Arc<Self>, kind: &str, summary: &Value) {
        let Some(id) = summary["task"].as_str() else { return };
        let vars = self.mem.load_plan(summary["plan_id"].as_str().unwrap_or("")).map(|p| p["vars"].clone()).unwrap_or(json!({}));
        let mut result = Map::new();
        if let Value::Object(v) = vars {
            for (k, val) in v {
                if !["state", "event", "result", "error", "timed_out"].contains(&k.as_str()) {
                    result.insert(k, val);
                }
            }
        }
        if kind == "plan.finished" {
            // Say what's actually known: the steps ran, plus what the plan's own steps checked (assess.verify).
            // A plan that checked nothing about its result doesn't get to look verified.
            let plan = self.mem.load_plan(summary["plan_id"].as_str().unwrap_or("")).map(|p| p["plan"].clone()).unwrap_or(Value::Null);
            let verify: Vec<Value> = plan["assess"]["verify"].as_array().cloned().unwrap_or_default().into_iter()
                .filter(|v| v.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)).collect();
            let mut checked = vec![json!("the plan ran to the end without an error")];
            let mut not_checked = vec![];
            if verify.is_empty() {
                not_checked.push(json!("whether the result is what was wanted: no step of the plan checked it"));
            } else {
                checked.extend(verify);
            }
            let evidence = json!({"checked": checked, "not_checked": not_checked, "by": "plan",
                                  "confidence": plan["assess"]["confidence"].clone()});
            self.task_finish(id, &["running", "waiting"], "done", json!({"result": result, "plan_id": summary["plan_id"], "evidence": evidence}));
        } else {
            let why = summary["error"].as_str().unwrap_or("stopped").to_string();
            self.task_finish(id, &["running", "waiting"], "failed", json!({"error": why, "plan_id": summary["plan_id"]}));
        }
    }

    /// After a restart, push every unfinished task forward again.
    pub fn task_recover(self: &Arc<Self>) {
        for st in ["pending", "running"] {
            for t in self.mem.tasks_with_status(st) {
                self.task_advance(t["id"].as_str().unwrap_or(""));
            }
        }
    }

    pub fn task_tree(&self, id: &str) -> Value {
        let Some(mut t) = self.mem.load_task(id) else { return Value::Null };
        let kids: Vec<Value> = self.mem.task_children(id).iter().map(|c| self.task_tree(c["id"].as_str().unwrap_or(""))).collect();
        t["children"] = json!(kids);
        if let Value::Object(m) = &mut t {
            m.remove("after_keys");
        }
        t
    }

    // -----------------------------------------------------------------------------------------
    // API
    // -----------------------------------------------------------------------------------------

    pub fn task_op(self: &Arc<Self>, op: &str, req: &Value) -> Value {
        let id = req["task"].as_str().unwrap_or("");
        let out = match op {
            "task_create" => return self.task_create(&req["spec"], req["source"].as_str().unwrap_or("ai")),
            "task" => match self.mem.load_task(id) {
                Some(_) => json!({"ok": true, "tree": self.task_tree(id)}),
                None => json!({"ok": false, "error": "no such task"}),
            },
            "tasks" => {
                let mut list = self.mem.root_tasks(req["limit"].as_i64().unwrap_or(30));
                for t in &mut list {
                    if let Value::Object(m) = t {
                        m.remove("after_keys");
                    }
                }
                json!({"ok": true, "tasks": list,
                       "needs_plan": self.mem.tasks_with_status("needs_plan"),
                       "waiting_on_human": self.mem.tasks_with_status("waiting")})
            }
            "task_plan" => {
                // The AI's plan for a task that needed one.
                let Some(t) = self.mem.transition_task(id, &["needs_plan"], "running", &json!({"error": null})) else {
                    return json!({"ok": false, "error": "that task isn't waiting for a plan"});
                };
                let mut plan = req["plan"].clone();
                plan["task"] = json!(id);
                if plan["plan"].as_str().unwrap_or("").is_empty() {
                    plan["plan"] = t["title"].clone();
                }
                self.task_run_plan(id, plan);
                json!({"ok": true, "tree": self.task_tree(id)})
            }
            "task_done" => {
                // The AI says how it knows the work is done: what it checked, what it didn't, how sure it is.
                let evidence = &req["evidence"];
                let list = |k: &str| evidence[k].as_array().map(|a| a.iter().filter_map(Value::as_str).filter(|s| !s.trim().is_empty()).count());
                if req["source"] != "human" && (list("checked").unwrap_or(0) == 0 || list("not_checked").is_none()
                    || evidence["confidence"].as_f64().is_none()) {
                    return json!({"ok": false, "error": "say how you know it's done: evidence {\"checked\": [what you verified,                         and how], \"not_checked\": [what you didn't verify, or []], \"confidence\": 0-1}"});
                }
                let ev = if evidence.is_object() { json!({"checked": evidence["checked"], "not_checked": evidence["not_checked"],
                    "confidence": evidence["confidence"], "by": req["source"].as_str().unwrap_or("ai")}) } else { Value::Null };
                self.task_finish(id, &["needs_plan", "running", "waiting", "pending"], "done", json!({"result": req["result"], "evidence": ev}));
                json!({"ok": self.mem.load_task(id).map(|t| t["status"] == "done").unwrap_or(false)})
            }
            "task_fail" => {
                self.task_finish(id, &["needs_plan", "running", "waiting", "pending"], "failed", json!({"error": req["error"]}));
                json!({"ok": true})
            }
            "task_cancel" => {
                self.task_cancel(id);
                json!({"ok": true})
            }
            _ => json!({"ok": false, "error": "unknown task op"}),
        };
        self.settle();
        out
    }

    fn task_cancel(self: &Arc<Self>, id: &str) {
        for c in self.mem.task_children(id) {
            self.task_cancel(c["id"].as_str().unwrap_or(""));
        }
        if let Some(t) = self.mem.transition_task(id, &["pending", "running", "waiting", "needs_plan"], "cancelled", &json!({})) {
            self.bus.emit(&self.mem, "task.cancelled", json!({"task": id, "title": t["title"]}));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Playbooks: one markdown file per category; broader categories apply to narrower ones
    // -----------------------------------------------------------------------------------------

    fn playbook_path(&self, category: &str) -> PathBuf {
        self.playbook_dir.join(format!("{category}.md"))
    }

    /// The playbooks that govern a category, broadest first: freight, freight/negotiate, freight/negotiate/hollis.
    pub fn playbook_chain(&self, category: &str) -> Value {
        if !valid_category(category) {
            return json!([]);
        }
        let segs: Vec<&str> = category.split('/').collect();
        Value::Array((1..=segs.len()).filter_map(|n| {
            let cat = segs[..n].join("/");
            self.playbook_sync(&cat);
            std::fs::read_to_string(self.playbook_path(&cat)).ok().map(|text| json!({"category": cat, "text": text}))
        }).collect())
    }

    pub fn playbook_op(self: &Arc<Self>, op: &str, req: &Value) -> Value {
        let cat = req["category"].as_str().unwrap_or("");
        match op {
            "playbook" => {
                if !valid_category(cat) {
                    return json!({"ok": false, "error": "category should look like freight/negotiate"});
                }
                json!({"ok": true, "chain": self.playbook_chain(cat),
                       "own": std::fs::read_to_string(self.playbook_path(cat)).ok()})
            }
            "playbook_put" => {
                if !valid_category(cat) {
                    return json!({"ok": false, "error": "category should look like freight/negotiate"});
                }
                let text = req["text"].as_str().unwrap_or("");
                let reason = req["reason"].as_str().unwrap_or("").trim();
                if text.trim().is_empty() || reason.is_empty() {
                    return json!({"ok": false, "error": "playbook_put needs the full new text and a reason"});
                }
                let source = req["source"].as_str().unwrap_or("human");
                if source == "ai" && !self.mem.pb_entries(cat, true).is_empty() {
                    return json!({"ok": false, "error": "the AI doesn't rewrite whole playbooks: use small edits (playbook_edit) so nothing gets lost"});
                }
                let path = self.playbook_path(cat);
                let old = std::fs::read_to_string(&path).unwrap_or_default();
                if old == text {
                    return json!({"ok": true, "changed": false});
                }
                if let Some(dir) = path.parent() {
                    if let Err(e) = std::fs::create_dir_all(dir) {
                        return json!({"ok": false, "error": e.to_string()});
                    }
                }
                if let Err(e) = std::fs::write(&path, text) {
                    return json!({"ok": false, "error": e.to_string()});
                }
                self.share_with_human(&path);
                self.playbook_sync(cat);
                self.mem.playbook_archive(cat, &old, reason);
                self.emit("playbook.updated", json!({"category": cat, "reason": reason, "new": old.is_empty()}));
                json!({"ok": true, "changed": true, "path": path.to_string_lossy()})
            }
            "playbooks" => {
                let mut cats = Vec::new();
                collect_playbooks(&self.playbook_dir, "", &mut cats);
                cats.sort();
                json!({"ok": true, "categories": cats})
            }
            "playbook_edit" => {
                if !valid_category(cat) {
                    return json!({"ok": false, "error": "category should look like freight/negotiate"});
                }
                let edits = req["edits"].as_array().cloned().unwrap_or_default();
                let validation = req["validation"].as_array().cloned().unwrap_or_default();
                self.playbook_edit(cat, &edits, &validation, req["reason"].as_str().unwrap_or(""), req["source"].as_str().unwrap_or("ai"))
            }
            "playbook_feedback" => self.playbook_feedback(req["id"].as_str().unwrap_or(""), req["helped"].as_bool().unwrap_or(true)),
            "playbook_entries" => {
                if !valid_category(cat) {
                    return json!({"ok": false, "error": "category should look like freight/negotiate"});
                }
                let segs: Vec<&str> = cat.split('/').collect();
                let mut all = Vec::new();
                for n in 1..=segs.len() {
                    let c = segs[..n].join("/");
                    self.playbook_sync(&c);
                    all.extend(self.mem.pb_entries(&c, true));
                }
                json!({"ok": true, "entries": all, "validation_cases": self.validation_cases(cat)})
            }
            "playbook_history" => json!({"ok": true, "history": self.mem.playbook_history(cat, req["limit"].as_i64().unwrap_or(20))}),
            _ => json!({"ok": false, "error": "unknown playbook op"}),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Cases: each real situation, for learning by repetition
    // -----------------------------------------------------------------------------------------

    pub fn object_op(self: &Arc<Self>, op: &str, req: &Value) -> Value {
        match op {
            "object_put" => {
                let typ = req["type"].as_str().unwrap_or("");
                if typ.is_empty() || !req["fields"].is_object() {
                    return json!({"ok": false, "error": "object_put needs a type and a fields object"});
                }
                let id = self.mem.put_object(typ, req["handle"].as_str(), &req["fields"]);
                self.emit("memory.stored", json!({"id": id, "type": typ}));
                json!({"ok": true, "id": id})
            }
            "find" => {
                let state = self.state.lock().unwrap().clone();
                let ctx = engine::Ctx { mem: &self.mem, organs: &self.organs, bus: &self.bus, state: &state };
                match engine::eval(&json!({"find": req["spec"]}), &Map::new(), &ctx) {
                    Ok(v) => json!({"ok": true, "found": v}),
                    Err(e) => json!({"ok": false, "error": e}),
                }
            }
            _ => json!({"ok": false, "error": "unknown object op"}),
        }
    }
}

impl Core {
    /// Make a playbook file (and its folders) editable by the human's group.
    pub(crate) fn share_playbook(&self, path: &std::path::Path) {
        self.share_with_human(path)
    }

    fn share_with_human(&self, path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let Some(gid) = self.file_gid else { return };
        let mut p = Some(path);
        while let Some(cur) = p {
            if !cur.starts_with(&self.playbook_dir) {
                break;
            }
            let _ = std::os::unix::fs::chown(cur, None, Some(gid));
            let mode = if cur.is_dir() { 0o2775 } else { 0o664 };
            let _ = std::fs::set_permissions(cur, std::fs::Permissions::from_mode(mode));
            p = cur.parent();
        }
    }
}

fn collect_playbooks(dir: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let p = e.path();
        if p.is_dir() {
            collect_playbooks(&p, &format!("{prefix}{name}/"), out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            out.push(format!("{prefix}{stem}"));
        }
    }
}
