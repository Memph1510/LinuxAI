//! Playbook protections (from ACE, Memento-Skills and Recuris; see docs/RESEARCH_SELF_LEARNING.md).
//!
//! A playbook is a set of small **entries**, not one blob of text. The AI changes it only with small edits
//! (add / revise / retire one entry), merged by plain code, never by rewriting the whole thing, so a
//! playbook can't "collapse". The markdown file is rendered from the entries and stays editable by the
//! human: their edits are read back in. Before an AI edit is kept it must pass a **validation gate**: it
//! has to be checked against recent cases, and anything that would have made a past decision worse,
//! touches what the human wrote, or changes a large part of the playbook at once is held for the human.

use crate::core::Core;
use crate::engine::read_approval;
use crate::memory::{new_id, now};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const SECTIONS: &[(&str, &str)] = &[
    ("what_matters", "What matters"),
    ("how_i_decide", "How I decide"),
    ("exceptions", "Exceptions"),
    ("open_questions", "Open questions"),
];
const VALIDATE_CASES: usize = 8;

fn section_key(heading: &str) -> String {
    let h = heading.trim().to_lowercase();
    SECTIONS.iter().find(|(k, t)| t.to_lowercase() == h || *k == h).map(|(k, _)| k.to_string()).unwrap_or_else(|| {
        h.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>().trim_matches('_').to_string()
    })
}

fn section_title(key: &str) -> String {
    SECTIONS.iter().find(|(k, _)| *k == key).map(|(_, t)| t.to_string()).unwrap_or_else(|| {
        let s = key.replace('_', " ");
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    })
}

fn digest(text: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn words(text: &str) -> HashSet<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '$' && c != '.').filter(|w| w.len() > 1).map(String::from).collect()
}

fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / a.union(&b).count() as f64
}

/// One parsed line of a playbook file: (section, text, entry id if it carries one).
type Line = (String, String, Option<String>);

/// Read a playbook file: the preamble (title and prose before the first section) and its entries.
fn parse(text: &str) -> (String, Vec<Line>) {
    let mut preamble = Vec::new();
    let mut lines = Vec::new();
    let mut section: Option<String> = None;
    for raw in text.lines() {
        let l = raw.trim();
        if let Some(h) = l.strip_prefix("## ") {
            section = Some(section_key(h));
            continue;
        }
        let is_bullet = l.starts_with("- ") || l.starts_with("* ");
        if section.is_none() && !is_bullet {
            preamble.push(raw.trim_end().to_string());
            continue;
        }
        if l.is_empty() {
            continue;
        }
        let body = if is_bullet { &l[2..] } else { l };
        let (text, id) = match body.find("<!--") {
            Some(i) => {
                let comment = body[i + 4..].trim_end_matches("-->").trim();
                let id = comment.split(|c: char| c.is_whitespace() || c == '·').next().filter(|t| t.starts_with('e') && t.len() > 3)
                    .map(String::from);
                (body[..i].trim().to_string(), id)
            }
            None => (body.trim().to_string(), None),
        };
        if !text.is_empty() {
            lines.push((section.clone().unwrap_or_else(|| "notes".into()), text, id));
        }
    }
    while preamble.last().map(|s| s.is_empty()).unwrap_or(false) {
        preamble.pop();
    }
    (preamble.join("\n"), lines)
}

impl Core {
    fn render_playbook(&self, cat: &str) -> String {
        let preamble = self.mem.pb_meta(cat).map(|(_, p)| p).filter(|p| !p.trim().is_empty()).unwrap_or_else(|| format!("# {cat}"));
        let entries = self.mem.pb_entries(cat, true);
        let mut keys: Vec<String> = SECTIONS.iter().map(|(k, _)| k.to_string()).collect();
        for e in &entries {
            let k = e["section"].as_str().unwrap_or("notes").to_string();
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let mut out = preamble.trim_end().to_string();
        out.push('\n');
        for k in keys {
            let items: Vec<&Value> = entries.iter().filter(|e| e["section"] == json!(k)).collect();
            if items.is_empty() {
                continue;
            }
            out.push_str(&format!("\n## {}\n", section_title(&k)));
            for e in items {
                let (h, n) = (e["helped"].as_i64().unwrap_or(0), e["hurt"].as_i64().unwrap_or(0));
                let mut tag = format!("{} · helped {h} · hurt {n}", e["id"].as_str().unwrap_or(""));
                if e["by"] == "human" {
                    tag.push_str(" · yours");
                }
                if n >= 3 && n > 2 * h {
                    tag.push_str(" · often wrong");
                }
                out.push_str(&format!("- {}  <!-- {tag} -->\n", e["text"].as_str().unwrap_or("")));
            }
        }
        out
    }

    fn write_playbook(&self, cat: &str) -> std::io::Result<String> {
        let text = self.render_playbook(cat);
        let path = self.playbook_dir.join(format!("{cat}.md"));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &text)?;
        self.share_playbook(&path);
        let pre = self.mem.pb_meta(cat).map(|(_, p)| p).unwrap_or_default();
        self.mem.pb_set_meta(cat, &digest(&text), &pre);
        Ok(text)
    }

    /// If the human edited the markdown file, take their edits into the entries: changed lines update their
    /// entry, new lines become entries, deleted lines retire theirs. Their edits need no validation.
    pub fn playbook_sync(&self, cat: &str) {
        let path = self.playbook_dir.join(format!("{cat}.md"));
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let h = digest(&text);
        if self.mem.pb_meta(cat).map(|(old, _)| old == h).unwrap_or(false) {
            return;
        }
        let (preamble, lines) = parse(&text);
        let existing: HashMap<String, Value> = self.mem.pb_entries(cat, true).into_iter()
            .map(|e| (e["id"].as_str().unwrap_or("").to_string(), e)).collect();
        let mut seen = HashSet::new();
        let t = now();
        for (section, line, id) in lines {
            match id.as_ref().and_then(|i| existing.get(i)) {
                Some(e) => {
                    seen.insert(e["id"].as_str().unwrap_or("").to_string());
                    if e["text"] != json!(line) || e["section"] != json!(section) {
                        let mut e = e.clone();
                        e["text"] = json!(line);
                        e["section"] = json!(section);
                        e["by"] = json!("human");
                        e["reason"] = json!("edited by you");
                        e["updated"] = json!(t);
                        self.mem.pb_save_entry(&e);
                    }
                }
                None => {
                    let e = json!({"id": new_id("e"), "category": cat, "section": section, "text": line, "evidence": [],
                                   "helped": 0, "hurt": 0, "status": "active", "by": "human", "reason": "written by you",
                                   "created": t, "updated": t});
                    self.mem.pb_save_entry(&e);
                }
            }
        }
        for (id, e) in &existing {
            if !seen.contains(id) {
                let mut e = e.clone();
                e["status"] = json!("retired");
                e["reason"] = json!("removed by you");
                self.mem.pb_save_entry(&e);
            }
        }
        self.mem.pb_set_meta(cat, &h, &preamble);
        let had_entries = !self.mem.pb_entries(cat, true).is_empty();
        if had_entries {
            let _ = self.write_playbook(cat); // add ids to new lines so later edits map to the same entry
        }
    }

    /// Cases the AI must check a playbook change against: the most recent ones in this category family.
    pub fn validation_cases(&self, cat: &str) -> Vec<Value> {
        let related = |c: &str| c == cat || cat.starts_with(&format!("{c}/")) || c.starts_with(&format!("{cat}/"));
        self.mem.objects_of("case", 5000).into_iter()
            .filter(|o| o["category"].as_str().map(related).unwrap_or(false) && o.get("decision").is_some())
            .take(VALIDATE_CASES)
            .map(|mut o| {
                if let Value::Object(m) = &mut o {
                    for k in ["type", "created", "handle"] {
                        m.remove(k);
                    }
                }
                o
            })
            .collect()
    }

    /// Small edits to a playbook, gated. `edits`: [{op: add, section, text, evidence?} | {op: revise, id, text,
    /// evidence?} | {op: retire, id}]. `validation`: [{case, verdict: same|better|worse, note}] for every case
    /// the core asks about. `source`: "ai" (gated) or "human" (applied directly).
    pub fn playbook_edit(self: &Arc<Self>, cat: &str, edits: &[Value], validation: &[Value], reason: &str, source: &str) -> Value {
        self.playbook_sync(cat);
        if reason.trim().is_empty() {
            return json!({"ok": false, "error": "say why the playbook is changing (reason)"});
        }
        let active = self.mem.pb_entries(cat, true);
        let mut errors = Vec::new();
        let (mut n_change, mut touches_human) = (0usize, false);
        for (i, e) in edits.iter().enumerate() {
            let op = e["op"].as_str().unwrap_or("");
            let text = e["text"].as_str().unwrap_or("").trim();
            match op {
                "add" => {
                    if text.is_empty() || e["section"].as_str().unwrap_or("").is_empty() {
                        errors.push(format!("edit {i}: add needs a section and text"));
                    } else if let Some(dup) = active.iter().find(|x| similarity(x["text"].as_str().unwrap_or(""), text) >= 0.8) {
                        errors.push(format!("edit {i}: already covered by {} (\"{}\"): revise it or give feedback instead",
                                            dup["id"].as_str().unwrap_or(""), dup["text"].as_str().unwrap_or("")));
                    }
                }
                "revise" | "retire" => {
                    let id = e["id"].as_str().unwrap_or("");
                    match active.iter().find(|x| x["id"] == json!(id)) {
                        None => errors.push(format!("edit {i}: no active entry {id} in {cat}")),
                        Some(x) => {
                            n_change += 1;
                            touches_human |= x["by"] == "human";
                            if op == "revise" && text.is_empty() {
                                errors.push(format!("edit {i}: revise needs the new text"));
                            }
                        }
                    }
                }
                _ => errors.push(format!("edit {i}: op must be add, revise or retire")),
            }
        }
        if edits.is_empty() {
            errors.push("no edits".into());
        }
        if !errors.is_empty() {
            return json!({"ok": false, "errors": errors});
        }
        if source == "human" {
            return self.apply_playbook_edits(cat, edits, reason, "human");
        }
        // The validation gate.
        let cases = self.validation_cases(cat);
        let answered: HashMap<String, &Value> = validation.iter().filter_map(|v| Some((v["case"].as_str()?.to_string(), v))).collect();
        let missing: Vec<&Value> = cases.iter().filter(|c| !answered.contains_key(c["id"].as_str().unwrap_or(""))).collect();
        if !missing.is_empty() {
            return json!({"ok": false, "needs_validation": missing,
                "error": "before this is kept, check it against these past cases: for each, would the decision have come out the same, better or worse with the change? Resubmit with validation: [{case, verdict, note}]"});
        }
        let worse: Vec<String> = cases.iter().filter_map(|c| {
            let v = answered.get(c["id"].as_str().unwrap_or(""))?;
            (v["verdict"] == "worse").then(|| format!("{} ({})", c["decision"].as_str().unwrap_or(""), v["note"].as_str().unwrap_or("")))
        }).collect();
        if let Some(bad) = validation.iter().find(|v| !matches!(v["verdict"].as_str(), Some("same" | "better" | "worse"))) {
            return json!({"ok": false, "error": format!("verdict must be same, better or worse: {bad}")});
        }
        let mut hold = Vec::new();
        if !worse.is_empty() {
            hold.push(format!("it would have made {} past decision(s) worse: {}", worse.len(), worse.join("; ")));
        }
        if touches_human {
            hold.push("it changes points you wrote".to_string());
        }
        if n_change > 2 && n_change as f64 > 0.3 * active.len() as f64 {
            hold.push(format!("it changes {n_change} of {} points at once", active.len()));
        }
        if hold.is_empty() {
            return self.apply_playbook_edits(cat, edits, reason, "ai");
        }
        // Held: the human decides.
        let pid = new_id("pp");
        let summary: Vec<String> = edits.iter().map(|e| match e["op"].as_str().unwrap_or("") {
            "add" => format!("add to {}: {}", section_title(e["section"].as_str().unwrap_or("")), e["text"].as_str().unwrap_or("")),
            "revise" => format!("change {} to: {}", e["id"].as_str().unwrap_or(""), e["text"].as_str().unwrap_or("")),
            _ => format!("remove {}", e["id"].as_str().unwrap_or("")),
        }).collect();
        self.mem.pb_save_proposal(&json!({"id": pid, "category": cat, "edits": edits, "reason": reason, "status": "held",
                                          "hold": hold, "validation": validation}));
        let plan = json!({"plan": format!("change the {cat} playbook"), "proposal": pid, "category": cat,
            "steps": [{"ask": format!("I'd like to change the {cat} playbook ({reason}):\n• {}\nOK?", summary.join("\n• ")),
                       "opts": ["yes", "no"], "why": format!("I'm holding it for you because {}.", hold.join(", and ")), "as": "answer"}]});
        let r = self.submit(&plan, "ai");
        json!({"ok": true, "applied": false, "held": hold, "proposal": pid, "plan_id": r["result"]["plan_id"]})
    }

    fn apply_playbook_edits(self: &Arc<Self>, cat: &str, edits: &[Value], reason: &str, by: &str) -> Value {
        let path = self.playbook_dir.join(format!("{cat}.md"));
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let t = now();
        let mut changes = Vec::new();
        for e in edits {
            let evidence = e.get("evidence").cloned().unwrap_or(json!([]));
            match e["op"].as_str().unwrap_or("") {
                "add" => {
                    let id = new_id("e");
                    self.mem.pb_save_entry(&json!({"id": id, "category": cat, "section": section_key(e["section"].as_str().unwrap_or("notes")),
                        "text": e["text"].as_str().unwrap_or("").trim(), "evidence": evidence, "helped": 0, "hurt": 0,
                        "status": "active", "by": by, "reason": reason, "created": t, "updated": t}));
                    changes.push(json!({"op": "add", "id": id}));
                }
                op => {
                    let id = e["id"].as_str().unwrap_or("");
                    let Some(mut x) = self.mem.pb_entry(id) else { continue };
                    if op == "revise" {
                        x["previous"] = x["text"].clone();
                        x["text"] = json!(e["text"].as_str().unwrap_or("").trim());
                        let mut ev = x["evidence"].as_array().cloned().unwrap_or_default();
                        ev.extend(evidence.as_array().cloned().unwrap_or_default());
                        x["evidence"] = json!(ev);
                    } else {
                        x["status"] = json!("retired");
                    }
                    x["reason"] = json!(reason);
                    x["updated"] = json!(t);
                    self.mem.pb_save_entry(&x);
                    changes.push(json!({"op": op, "id": id}));
                }
            }
        }
        if let Err(e) = self.write_playbook(cat) {
            return json!({"ok": false, "error": e.to_string()});
        }
        if !old.is_empty() {
            self.mem.playbook_archive(cat, &old, reason);
        }
        self.emit("playbook.updated", json!({"category": cat, "reason": reason, "by": by, "changes": changes, "new": old.is_empty()}));
        json!({"ok": true, "applied": true, "changes": changes})
    }

    /// After a task: which playbook points helped or hurt. Points that keep hurting get flagged.
    pub fn playbook_feedback(self: &Arc<Self>, id: &str, helped: bool) -> Value {
        let Some(mut e) = self.mem.pb_entry(id) else { return json!({"ok": false, "error": "no such entry"}) };
        let k = if helped { "helped" } else { "hurt" };
        e[k] = json!(e[k].as_i64().unwrap_or(0) + 1);
        self.mem.pb_save_entry(&e);
        let cat = e["category"].as_str().unwrap_or("").to_string();
        let _ = self.write_playbook(&cat);
        let (h, n) = (e["helped"].as_i64().unwrap_or(0), e["hurt"].as_i64().unwrap_or(0));
        if !helped && n >= 3 && n > 2 * h {
            self.emit("playbook.entry_questioned", json!({"category": cat, "id": id, "text": e["text"], "helped": h, "hurt": n}));
        }
        json!({"ok": true, "helped": h, "hurt": n})
    }

    /// The human answered a held playbook change.
    pub fn playbook_on_plan_end(self: &Arc<Self>, kind: &str, summary: &Value) {
        let Some(pid) = summary["proposal"].as_str() else { return };
        let Some(mut p) = self.mem.pb_proposal(pid) else { return };
        if p["status"] != "held" {
            return;
        }
        let answer = self.mem.load_plan(summary["plan_id"].as_str().unwrap_or("")).map(|x| x["vars"]["answer"].clone()).unwrap_or(Value::Null);
        let yes = kind == "plan.finished" && read_approval(answer.as_str().unwrap_or("")).is_some();
        p["status"] = json!(if yes { "applied" } else { "declined" });
        self.mem.pb_save_proposal(&p);
        if p["kind"] == "prompt" {
            let name = p["name"].as_str().unwrap_or("").to_string();
            if yes {
                self.write_prompt(&name, p["text"].as_str().unwrap_or(""), &format!("{} (approved by you)", p["reason"].as_str().unwrap_or("")));
            } else {
                self.bus.emit(&self.mem, "prompt.declined", json!({"name": name, "proposal": pid}));
            }
            return;
        }
        let cat = p["category"].as_str().unwrap_or("").to_string();
        if yes {
            let edits = p["edits"].as_array().cloned().unwrap_or_default();
            let reason = format!("{} (approved by you)", p["reason"].as_str().unwrap_or(""));
            self.apply_playbook_edits(&cat, &edits, &reason, "ai");
        } else {
            self.bus.emit(&self.mem, "playbook.declined", json!({"category": cat, "proposal": pid, "answer": answer}));
        }
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn parses_sections_ids_and_free_lines() {
        let (pre, lines) = parse("# Freight\nSome intro.\n\n## How I decide\n- Floor $2.40  <!-- e12ab · helped 2 · hurt 0 -->\n- New point\n\n## Weather\nSnow slows Atlanta.\n");
        assert_eq!(pre, "# Freight\nSome intro.");
        assert_eq!(lines[0], ("how_i_decide".into(), "Floor $2.40".into(), Some("e12ab".into())));
        assert_eq!(lines[1], ("how_i_decide".into(), "New point".into(), None));
        assert_eq!(lines[2], ("weather".into(), "Snow slows Atlanta.".into(), None));
    }
}
