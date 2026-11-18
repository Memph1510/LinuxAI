//! The prompt library: reusable prompts as markdown files the human reads and edits (`~/prompts/`).
//!
//! A prompt file has a short header and the prompt itself, with {{slots}} to fill in:
//!
//!   ---
//!   title: Counter a broker's offer
//!   use: freight/negotiate
//!   model: any            (any | local | claude)
//!   slots: broker, lane, offer, counter
//!   tags: email, negotiation
//!   ---
//!   Write a short, firm, friendly reply to {{broker}} ...
//!
//! The core lists and searches them (best-performing first), fills them in, counts how often each helped or
//! hurt, and holds the AI's suggested changes for the human. The human edits the files directly.

use crate::core::Core;
use crate::memory::new_id;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 120
        && n.split('/').all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

/// Split a prompt file into its header fields and its body.
pub fn parse_prompt(text: &str) -> (Map<String, Value>, String) {
    let mut head = Map::new();
    let t = text.trim_start_matches('\u{feff}');
    let Some(rest) = t.strip_prefix("---") else { return (head, t.trim().to_string()) };
    let Some(end) = rest.find("\n---") else { return (head, t.trim().to_string()) };
    for line in rest[..end].lines() {
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_lowercase(), v.trim());
            let val = if matches!(k.as_str(), "slots" | "tags") {
                json!(v.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).collect::<Vec<_>>())
            } else {
                json!(v)
            };
            head.insert(k, val);
        }
    }
    let body = rest[end + 4..].trim_start_matches(['-', '\r', '\n']).trim().to_string();
    (head, body)
}

/// The {{slots}} a prompt body uses, in order of first appearance.
pub fn slots_in(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        let Some(j) = rest[i..].find("}}") else { break };
        let name = rest[i + 2..i + j].trim().to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
        rest = &rest[i + j + 2..];
    }
    out
}

/// Fill in a prompt. Every slot must be given; nothing else changes.
pub fn render(body: &str, vars: &Map<String, Value>) -> Result<String, Vec<String>> {
    let missing: Vec<String> = slots_in(body).into_iter().filter(|s| !vars.contains_key(s)).collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    let mut out = body.to_string();
    for (k, v) in vars {
        let s = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace(&format!("{{{{{k}}}}}"), &s).replace(&format!("{{{{ {k} }}}}"), &s);
    }
    Ok(out)
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.eq_ignore_ascii_case("README.md") {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            collect(&p, &format!("{prefix}{name}/"), out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            out.push(format!("{prefix}{stem}"));
        }
    }
}

impl Core {
    fn prompt_path(&self, name: &str) -> PathBuf {
        self.prompt_dir.join(format!("{name}.md"))
    }

    fn prompt_entry(&self, name: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.prompt_path(name)).ok()?;
        let (head, body) = parse_prompt(&text);
        let stats = self.mem.object(&format!("obj:prompt_stats/{name}")).unwrap_or(json!({}));
        let n = |k: &str| stats[k].as_i64().unwrap_or(0);
        let mut slots: Vec<String> = head.get("slots").and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        for s in slots_in(&body) {
            if !slots.contains(&s) {
                slots.push(s);
            }
        }
        Some(json!({"name": name, "title": head.get("title").cloned().unwrap_or(json!(name)), "use": head.get("use"),
                    "model": head.get("model").cloned().unwrap_or(json!("any")), "tags": head.get("tags").cloned().unwrap_or(json!([])),
                    "slots": slots, "uses": n("uses"), "helped": n("helped"), "hurt": n("hurt"), "body": body}))
    }

    fn bump_prompt(&self, name: &str, key: &str) {
        let id = format!("obj:prompt_stats/{name}");
        match self.mem.object(&id) {
            Some(o) => {
                self.mem.patch_object(&id, &json!({key: o[key].as_i64().unwrap_or(0) + 1}));
            }
            None => self.mem.put_object_with_id(&id, "prompt_stats", &json!({"name": name, "uses": 0, "helped": 0, "hurt": 0, key: 1})),
        }
    }

    pub fn prompt_op(self: &Arc<Self>, op: &str, req: &Value) -> Value {
        let name = req["name"].as_str().unwrap_or("");
        match op {
            "prompts" => {
                let mut names = Vec::new();
                collect(&self.prompt_dir, "", &mut names);
                let q: Vec<String> = req["query"].as_str().unwrap_or("").to_lowercase().split_whitespace().map(String::from).collect();
                let model = req["model"].as_str();
                let mut list: Vec<Value> = names.iter().filter_map(|n| self.prompt_entry(n)).filter(|p| {
                    let hay = format!("{} {} {} {}", p["name"], p["title"], p["use"], p["tags"]).to_lowercase();
                    q.iter().all(|w| hay.contains(w.as_str()))
                        && model.map(|m| p["model"] == "any" || p["model"] == m).unwrap_or(true)
                }).map(|mut p| {
                    if let Value::Object(m) = &mut p {
                        m.remove("body");
                    }
                    p
                }).collect();
                // Best first: what has helped most (net of hurt), then what's used most.
                list.sort_by_key(|p| std::cmp::Reverse((p["helped"].as_i64().unwrap_or(0) - p["hurt"].as_i64().unwrap_or(0), p["uses"].as_i64().unwrap_or(0))));
                json!({"ok": true, "prompts": list})
            }
            "prompt" => match self.prompt_entry(name) {
                Some(p) => json!({"ok": true, "prompt": p}),
                None => json!({"ok": false, "error": format!("no prompt named {name}")}),
            },
            "prompt_render" => {
                let Some(p) = self.prompt_entry(name) else { return json!({"ok": false, "error": format!("no prompt named {name}")}) };
                let vars = req["vars"].as_object().cloned().unwrap_or_default();
                match render(p["body"].as_str().unwrap_or(""), &vars) {
                    Ok(text) => {
                        self.bump_prompt(name, "uses");
                        json!({"ok": true, "text": text, "model": p["model"]})
                    }
                    Err(missing) => json!({"ok": false, "error": format!("fill in: {}", missing.join(", ")), "missing": missing}),
                }
            }
            "prompt_feedback" => {
                if self.prompt_entry(name).is_none() {
                    return json!({"ok": false, "error": "no such prompt"});
                }
                self.bump_prompt(name, if req["helped"].as_bool().unwrap_or(true) { "helped" } else { "hurt" });
                json!({"ok": true})
            }
            "prompt_put" | "prompt_suggest" => {
                if !valid_name(name) {
                    return json!({"ok": false, "error": "name should look like freight/counter-offer"});
                }
                let text = req["text"].as_str().unwrap_or("");
                let reason = req["reason"].as_str().unwrap_or("").trim();
                let (head, body) = parse_prompt(text);
                if body.is_empty() || !head.contains_key("title") {
                    return json!({"ok": false, "error": "a prompt needs a header with at least a title, and a body"});
                }
                if op == "prompt_put" {
                    // The human's own edit.
                    return self.write_prompt(name, text, if reason.is_empty() { "edited by you" } else { reason });
                }
                if reason.is_empty() {
                    return json!({"ok": false, "error": "say why (reason)"});
                }
                // The AI's suggestion waits for the human.
                let pid = new_id("pp");
                let old = std::fs::read_to_string(self.prompt_path(name)).ok();
                self.mem.pb_save_proposal(&json!({"id": pid, "kind": "prompt", "name": name, "text": text, "reason": reason,
                                                  "status": "held"}));
                let what = if old.is_some() { format!("change the prompt \"{name}\"") } else { format!("add a prompt \"{name}\"") };
                let plan = json!({"plan": what, "proposal": pid, "steps": [{"ask": format!("I'd like to {what} ({reason}):\n\n{text}\n\nOK?"),
                    "opts": ["yes", "no"], "why": "Prompts in your library change only with your OK.", "as": "answer"}]});
                let r = self.submit(&plan, "ai");
                json!({"ok": true, "held": true, "proposal": pid, "plan_id": r["result"]["plan_id"]})
            }
            _ => json!({"ok": false, "error": "unknown prompt op"}),
        }
    }

    pub(crate) fn write_prompt(self: &Arc<Self>, name: &str, text: &str, reason: &str) -> Value {
        let path = self.prompt_path(name);
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return json!({"ok": false, "error": e.to_string()});
            }
        }
        if let Err(e) = std::fs::write(&path, text) {
            return json!({"ok": false, "error": e.to_string()});
        }
        self.share_playbook(&path);
        self.emit("prompt.updated", json!({"name": name, "reason": reason}));
        json!({"ok": true, "path": path.to_string_lossy()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_starter_prompt_parses_and_fills_in() {
        let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../library/prompts");
        let mut names = Vec::new();
        collect(&lib, "", &mut names);
        assert!(names.len() >= 19, "{names:?}");
        for n in names {
            let (head, body) = parse_prompt(&std::fs::read_to_string(lib.join(format!("{n}.md"))).unwrap());
            assert!(head.get("title").is_some() && head.get("use").is_some(), "{n}: header");
            assert!(matches!(head["model"].as_str(), Some("any" | "local" | "claude")), "{n}: model");
            let slots = slots_in(&body);
            assert!(!slots.is_empty(), "{n}: no slots");
            let vars: Map<String, Value> = slots.iter().map(|s| (s.clone(), json!("X"))).collect();
            assert!(!render(&body, &vars).unwrap().contains("{{"), "{n}: unfilled slot");
        }
    }

    #[test]
    fn parses_header_slots_and_renders() {
        let (h, body) = parse_prompt("---\ntitle: Counter\nuse: freight/negotiate\nmodel: claude\nslots: broker, offer\ntags: email,x\n---\nReply to {{broker}} about {{ offer }} and {{lane}}.");
        assert_eq!(h["title"], "Counter");
        assert_eq!(h["slots"], json!(["broker", "offer"]));
        assert_eq!(slots_in(&body), ["broker", "offer", "lane"]);
        let mut vars = Map::new();
        vars.insert("broker".into(), json!("Dana"));
        vars.insert("offer".into(), json!("$1,640"));
        assert_eq!(render(&body, &vars).unwrap_err(), ["lane"]);
        vars.insert("lane".into(), json!("DAL-ATL"));
        assert_eq!(render(&body, &vars).unwrap(), "Reply to Dana about $1,640 and DAL-ATL.");
    }
}
