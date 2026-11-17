use crate::core::Core;
use crate::engine;
use crate::host;
use crate::memory::Memory;
use crate::verbs::Organs;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Organs for Fake {
    fn invoke(&self, verb: &str, args: &Value) -> Result<Value, String> {
        self.calls.lock().unwrap().push((verb.to_string(), args.clone()));
        if args["what"] == "explode" {
            return Err("boom".into());
        }
        Ok(json!({"ok": true}))
    }
}

fn core() -> (Core, Fake) {
    let fake = Fake::default();
    (Core::new(Memory::in_memory(), Box::new(fake.clone())), fake)
}

fn calls(f: &Fake) -> Vec<String> {
    f.calls.lock().unwrap().iter().map(|(v, a)| format!("{v} {} {}", a["what"].as_str().unwrap_or(""), a["to"].as_str().unwrap_or(""))).collect()
}

#[test]
fn check_catches_mistakes() {
    let mem = Memory::in_memory();
    let plan = json!({"plan": "bad", "steps": [
        {"do": "fly", "to": "moon"},
        {"do": "set", "what": "power.profile"},
        {"tell": "hi {$nobody}"},
        {"do": "set", "what": "cpu.policy", "to": "x", "colour": "red"}
    ]});
    let r = engine::check(&plan, &mem, "ai", 0.8);
    assert!(!r.ok);
    let all = r.errors.join("\n");
    assert!(all.contains("unknown verb 'fly'"), "{all}");
    assert!(all.contains("needs 'to'"), "{all}");
    assert!(all.contains("$nobody is used before"), "{all}");
    assert!(all.contains("no argument 'colour'"), "{all}");
}

#[test]
fn trust_is_learned_from_conversation() {
    let (c, fake) = core();
    let plan = json!({"plan": "quiet-mode", "why": "you asked for quiet", "conf": 0.95,
                      "steps": [{"do": "set", "what": "power.profile", "to": "power-saver"}]});
    // Trust starts at zero: the AI has to ask.
    let r = c.submit(&plan, "ai");
    assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
    assert!(r["result"]["wait"]["question"].as_str().unwrap().contains("set power.profile to power-saver"));
    assert!(calls(&fake).is_empty());
    let id = r["result"]["plan_id"].as_str().unwrap().to_string();
    let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": "yes, always"}));
    assert_eq!(r["result"]["status"], "done", "{r}");
    assert_eq!(calls(&fake), vec!["set power.profile power-saver"]);
    // Now it's trusted for that kind of action.
    let r = c.submit(&plan, "ai");
    assert_eq!(r["result"]["status"], "done", "{r}");
    // ...but not for a different one.
    let other = json!({"plan": "x", "conf": 0.95, "steps": [{"do": "restart", "unit": "nginx.service"}]});
    assert_eq!(c.submit(&other, "ai")["result"]["status"], "waiting:answer");
}

#[test]
fn no_means_no() {
    let (c, fake) = core();
    let plan = json!({"plan": "p", "conf": 0.9, "steps": [{"do": "restart", "unit": "x.service"}]});
    let id = c.submit(&plan, "ai")["result"]["plan_id"].as_str().unwrap().to_string();
    let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": "no, not now"}));
    assert_eq!(r["result"]["status"], "stopped");
    assert!(calls(&fake).is_empty());
}

#[test]
fn bill_anomaly_plan_asks_then_resumes_durably() {
    let (c, _) = core();
    {
        let mem = c.mem.lock().unwrap();
        for amt in [80.0, 84.0, 82.0] {
            mem.put_object("bill", None, &json!({"from": "@verizon", "amount": amt, "status": "paid"}));
        }
        mem.put_object("bill", None, &json!({"from": "@verizon", "amount": 112.40, "status": "unpaid"}));
    }
    let plan = json!({"plan": "review-verizon", "conf": 0.9, "steps": [
        {"let": "bill", "find": {"type": "bill", "from": "@verizon", "where": {"status": "unpaid"}, "pick": "newest"}},
        {"let": "usual", "calc": ["avg", "amount", {"find": {"type": "bill", "from": "@verizon", "where": {"status": "paid"}}}]},
        {"if": [">", "$bill.amount", ["*", "$usual", 1.2]],
         "then": [{"ask": "Verizon is ${$bill.amount}, usual is ${$usual}. Pay it?", "opts": ["pay", "hold"], "as": "ans"},
                  {"expect": ["=", "$ans", "pay"], "else": "stop"}]},
        {"do": "remember", "text": "Verizon bill {$bill.amount} approved"}
    ]});
    let r = c.submit(&plan, "ai");
    assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
    assert_eq!(r["result"]["wait"]["question"], "Verizon is $112.40, usual is $82. Pay it?");
    // The plan lives in the database, not in memory: resume purely from the stored state.
    let id = r["result"]["plan_id"].as_str().unwrap().to_string();
    let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": "pay"}));
    assert_eq!(r["result"]["status"], "done", "{r}");
    let notes = c.mem.lock().unwrap().objects_of("note", 10);
    assert_eq!(notes[0]["text"], "Verizon bill 112.40 approved");
}

#[test]
fn rules_run_the_machine_without_the_model() {
    let (c, fake) = core();
    c.emit("power.unplugged", json!({"battery": 64}));
    assert_eq!(calls(&fake), vec!["set power.profile power-saver", "set cpu.policy efficiency"]);
    c.emit("power.plugged", json!({"battery": 70}));
    assert_eq!(calls(&fake)[2..], ["set power.profile balanced".to_string(), "set cpu.policy balanced".to_string()]);
}

#[test]
fn rule_failures_are_contained_by_try() {
    let (c, _) = core();
    let r = c.handle(&json!({"op": "add_rule", "rule": {"rule": "t", "when": "test.go", "steps": [
        {"try": [{"do": "set", "what": "explode", "to": "1"}], "else": [{"tell": "failed: {$error}"}]}]}}));
    assert_eq!(r["ok"], true, "{r}");
    c.emit("test.go", json!({}));
    let ev = c.handle(&json!({"op": "events", "since": 0}));
    let tells: Vec<String> = ev["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "ai.tell")
        .map(|e| e["data"]["text"].as_str().unwrap().to_string()).collect();
    assert!(tells.iter().any(|t| t.contains("boom")), "{tells:?}");
}

#[test]
fn wait_for_event_then_continue() {
    let (c, fake) = core();
    let plan = json!({"plan": "after-plug", "steps": [
        {"wait": "power.plugged", "max": "1h"},
        {"do": "set", "what": "cpu.policy", "to": "responsive"}]});
    let r = c.submit(&plan, "human");
    assert_eq!(r["result"]["status"], "waiting:event");
    c.emit("power.plugged", json!({}));
    assert!(calls(&fake).contains(&"set cpu.policy responsive".to_string()), "{:?}", calls(&fake));
}

#[test]
fn think_hands_back_to_the_ai() {
    let (c, fake) = core();
    c.emit("service.failed", json!({"unit": "nginx.service"}));
    let w = c.handle(&json!({"op": "waiting"}));
    let p = &w["plans"][0];
    assert_eq!(p["status"], "needs_thought");
    assert!(p["wait"]["note"].as_str().unwrap().contains("nginx.service"));
    let r = c.handle(&json!({"op": "resolve", "plan_id": p["plan_id"], "steps": [{"do": "restart", "unit": "nginx.service"}]}));
    assert_eq!(r["result"]["status"], "done", "{r}");
    assert_eq!(fake.calls.lock().unwrap()[0].0, "restart");
}

#[test]
fn each_and_calc() {
    let (c, _) = core();
    let plan = json!({"plan": "count", "steps": [
        {"let": "total", "value": 0},
        {"each": [1, 2, 3, 4], "as": "n", "steps": [{"let": "total", "calc": ["+", "$total", "$n"]}]},
        {"expect": ["=", "$total", 10], "else": "stop"}]});
    let r = c.submit(&plan, "human");
    assert_eq!(r["result"]["status"], "done", "{r}");
    assert_eq!(r["vars"]["total"], 10);
}

#[test]
fn senses_turn_changes_into_events() {
    let a = json!({"power": {"ac": true, "battery": 25}, "failed_units": [], "memory_pressure": {"some_avg10": 1.0},
                   "cpu": {"pressure": {"some_avg10": 0.0}}, "temperature_max_c": 50.0});
    let b = json!({"power": {"ac": false, "battery": 19}, "failed_units": ["nginx.service"], "memory_pressure": {"some_avg10": 40.0},
                   "cpu": {"pressure": {"some_avg10": 0.0}}, "temperature_max_c": 50.0});
    let kinds: Vec<String> = host::derive_events(&a, &b).into_iter().map(|(k, _)| k).collect();
    assert_eq!(kinds, ["power.unplugged", "battery.low", "memory.pressure", "service.failed"]);
    assert_eq!(host::derive_events(&Value::Null, &a)[0].0, "system.started");
}

#[test]
fn real_host_snapshot_does_not_panic() {
    let s = host::snapshot(true, &Value::Null);
    assert!(s["memory"]["total_mb"].as_u64().unwrap_or(0) > 0);
}
