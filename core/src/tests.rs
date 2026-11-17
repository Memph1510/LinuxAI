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

fn core() -> (Arc<Core>, Fake) {
    let fake = Fake::default();
    (Arc::new(Core::new(Memory::in_memory(), Box::new(fake.clone()))), fake)
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
        let mem = &c.mem;
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
    let notes = c.mem.objects_of("note", 10);
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
    let r = c.handle(&json!({"op": "add_rule", "source": "human", "rule": {"rule": "t", "when": "test.go", "steps": [
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
    assert_eq!(r["result"]["status"], "waiting:answer", "restarting needs the human's ok first: {r}");
    let r = c.handle(&json!({"op": "answer", "plan_id": p["plan_id"], "text": "yes"}));
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

#[test]
fn undecided_reaches_out_follows_up_and_falls_back() {
    let (c, fake) = core();
    let plan = json!({"plan": "freight-offer", "steps": [
        {"ask": "Broker offers $1,780 ($2.28/mi), under your floor. Take it?", "opts": ["take", "hold at floor"],
         "why": "It matches your weekend-home exception, but it's further under the floor than you've accepted before.",
         "lean": "take", "remind": ["0s"], "deadline": "0s", "fallback": "hold at floor", "as": "ans"},
        {"if": ["=", "$ans", "take"], "then": [{"do": "set", "what": "power.profile", "to": "accepted"}],
         "else": [{"tell": "Holding at your floor ({$answered_by})."}]}]});
    let r = c.submit(&plan, "human");
    let w = &r["result"]["wait"];
    assert_eq!(r["result"]["status"], "waiting:answer");
    assert!(w["why"].as_str().unwrap().contains("weekend-home exception"));
    assert_eq!(w["lean"], "take");
    c.tick();   // reminder fires and the deadline passes → the fallback is taken, and the human is told
    let ev = c.handle(&json!({"op": "events", "since": 0}));
    let kinds: Vec<String> = ev["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect();
    let tells: Vec<String> = ev["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "ai.tell")
        .map(|e| e["data"]["text"].as_str().unwrap().to_string()).collect();
    assert!(tells.iter().any(|t| t.contains("went with \"hold at floor\"")), "{tells:?}");
    assert!(tells.iter().any(|t| t.contains("Holding at your floor (fallback)")), "{tells:?}");
    assert!(kinds.contains(&"plan.finished".to_string()), "{kinds:?}");
    assert!(calls(&fake).is_empty());
}

#[test]
fn approval_says_why_and_schedules_follow_ups() {
    let (c, _) = core();
    let r = c.submit(&json!({"plan": "p", "conf": 0.9, "steps": [{"do": "restart", "unit": "x.service"}]}), "ai");
    let w = &r["result"]["wait"];
    assert!(w["why"].as_str().unwrap().contains("on my own"));
    assert_eq!(w["reminders"].as_array().unwrap().len(), 2);
}

// ---- soundness ---------------------------------------------------------------------------------

/// An organ whose `run` verb takes a while, like a real long command.
#[derive(Clone, Default)]
struct Slow {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}
impl Organs for Slow {
    fn invoke(&self, verb: &str, args: &Value) -> Result<Value, String> {
        if verb == "run" {
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        self.calls.lock().unwrap().push((verb.to_string(), args.clone()));
        Ok(json!({"ok": true}))
    }
}

#[test]
fn a_long_action_does_not_freeze_the_core() {
    let c = Arc::new(Core::new(Memory::in_memory(), Box::new(Slow::default())));
    let c2 = c.clone();
    let worker = std::thread::spawn(move || {
        c2.submit(&json!({"plan": "slow", "steps": [{"do": "run", "program": "sleep"}]}), "human")
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    let t = std::time::Instant::now();
    c.handle(&json!({"op": "events", "since": 0}));
    c.handle(&json!({"op": "waiting"}));
    c.emit("power.unplugged", json!({}));   // the machine must still react while the long action runs
    assert!(t.elapsed().as_millis() < 500, "core was blocked for {:?}", t.elapsed());
    assert_eq!(worker.join().unwrap()["result"]["status"], "done");
}

#[test]
fn no_not_always_is_a_no() {
    for reply in ["no, not always", "not now", "never", "don't", "cancel that", "hmm, let me think"] {
        let (c, fake) = core();
        let r = c.submit(&json!({"plan": "p", "conf": 0.9, "steps": [{"do": "restart", "unit": "x.service"}]}), "ai");
        let id = r["result"]["plan_id"].as_str().unwrap().to_string();
        let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": reply}));
        assert_eq!(r["result"]["status"], "stopped", "reply {reply:?} was taken as a yes: {r}");
        assert!(calls(&fake).is_empty());
        assert!(c.handle(&json!({"op": "mandates"}))["mandates"].as_array().unwrap().is_empty(), "{reply:?} granted trust");
    }
}

#[test]
fn steps_added_by_the_ai_later_are_checked_and_gated() {
    let (c, fake) = core();
    c.emit("service.failed", json!({"unit": "nginx.service"}));
    let id = c.handle(&json!({"op": "waiting"}))["plans"][0]["plan_id"].as_str().unwrap().to_string();
    // Malformed steps are rejected, and the plan keeps waiting for a proper answer.
    let r = c.handle(&json!({"op": "resolve", "plan_id": id, "steps": [{"do": "launch-missiles"}]}));
    assert_eq!(r["ok"], false, "{r}");
    assert_eq!(c.handle(&json!({"op": "waiting"}))["plans"][0]["status"], "needs_thought");
    // An action the AI isn't trusted with yet becomes a question to the human, not an action.
    let r = c.handle(&json!({"op": "resolve", "plan_id": id, "steps": [{"do": "stop", "unit": "caddy.service"}]}));
    assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
    assert!(calls(&fake).is_empty(), "ran before the human approved: {:?}", calls(&fake));
    c.handle(&json!({"op": "answer", "plan_id": id, "text": "yes"}));
    assert_eq!(calls(&fake).len(), 1);
}

#[test]
fn rules_from_the_ai_need_the_humans_ok() {
    let (c, fake) = core();
    let rule = json!({"rule": "restart-web", "when": "service.failed", "steps": [{"do": "restart", "unit": "nginx.service"}]});
    let r = c.handle(&json!({"op": "add_rule", "rule": rule, "source": "ai"}));
    assert_eq!(r["ok"], true, "{r}");
    let rules = c.handle(&json!({"op": "rules"}))["rules"].as_array().unwrap().clone();
    let ours = rules.iter().find(|r| r["rule"]["rule"] == "restart-web").unwrap();
    assert_eq!(ours["enabled"], false, "an AI-made rule ran without the human's ok");
    let w = c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().clone();
    let ask = w.iter().find(|p| p["wait"]["approval"] == true).expect("the human should be asked");
    c.handle(&json!({"op": "answer", "plan_id": ask["plan_id"], "text": "yes"}));
    c.emit("service.failed", json!({"unit": "nginx.service"}));
    assert!(calls(&fake).iter().any(|c| c.starts_with("restart")), "{:?}", calls(&fake));
}

#[test]
fn a_crash_mid_plan_resumes_without_repeating_actions() {
    let dir = std::env::temp_dir().join(format!("linuxai-recover-{}", std::process::id()));
    let db = dir.join("core.db");
    let _ = std::fs::remove_dir_all(&dir);
    let plan = json!({"plan": "two-steps", "steps": [
        {"do": "set", "what": "power.profile", "to": "A"}, {"do": "set", "what": "power.profile", "to": "B"}]});
    {
        // Simulate a crash right after step 1's action completed, before its progress was saved.
        let mem = Memory::open(&db).unwrap();
        let mut ex = engine::new_exec(&plan, "human");
        ex.approved = true;
        ex.counter = 0;
        let key = format!("{}:1", ex.id);
        let j = mem.journal_begin(&ex.id, &key, "set", &json!({"what": "power.profile", "to": "A"}), "set A");
        mem.journal_end(j, "done", "{}");
        mem.save_plan(&ex.id, &ex.name, &ex.source, "running", &serde_json::to_value(&ex).unwrap());
    }
    let fake = Fake::default();
    let c = Arc::new(Core::new(Memory::open(&db).unwrap(), Box::new(fake.clone())));
    c.recover();
    assert_eq!(calls(&fake), vec!["set power.profile B"], "step 1 must not be repeated after the restart");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rule_that_triggers_itself_is_stopped() {
    let (c, _) = core();
    let r = c.handle(&json!({"op": "add_rule", "source": "human",
        "rule": {"rule": "echo", "when": "ai.tell", "steps": [{"tell": "again"}]}}));
    assert_eq!(r["ok"], true, "{r}");
    c.handle(&json!({"op": "tell", "text": "start"}));
    let ev = c.handle(&json!({"op": "events", "since": 0, "limit": 100000}));
    let evs = ev["events"].as_array().unwrap();
    let tells = evs.iter().filter(|e| e["kind"] == "ai.tell").count();
    assert!(tells <= 12, "rule looped {tells} times");
    assert!(evs.iter().any(|e| e["kind"] == "rule.throttled"));
}

#[test]
fn many_plans_at_once_all_finish() {
    let (c, fake) = core();
    let threads: Vec<_> = (0..8).map(|t| {
        let c = c.clone();
        std::thread::spawn(move || {
            for i in 0..25 {
                let r = c.submit(&json!({"plan": format!("p{t}-{i}"), "steps": [
                    {"ask": "go?", "as": "a"}, {"do": "set", "what": "power.profile", "to": format!("{t}-{i}")}]}), "human");
                let id = r["result"]["plan_id"].as_str().unwrap().to_string();
                if i % 5 == 0 { c.emit("power.unplugged", json!({})); }
                let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": "go"}));
                assert_eq!(r["result"]["status"], "done", "{r}");
            }
        })
    }).collect();
    for t in threads { t.join().unwrap(); }
    let sets = fake.calls.lock().unwrap().len();
    assert!(sets >= 200, "only {sets} actions ran");
    assert!(c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().is_empty());
}

#[test]
fn a_plan_is_saved_before_its_first_step_runs() {
    #[derive(Clone)]
    struct Peek(Arc<Mutex<Option<Value>>>, Arc<Memory>);
    impl Organs for Peek {
        fn invoke(&self, _: &str, _: &Value) -> Result<Value, String> {
            *self.0.lock().unwrap() = Some(json!(self.1.plans_with_status("running").len()));
            Ok(json!({}))
        }
    }
    // The organ looks at the database while the first action is in progress.
    let dir = std::env::temp_dir().join(format!("linuxai-first-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("core.db");
    let seen = Arc::new(Mutex::new(None));
    let c = Arc::new(Core::new(Memory::open(&db).unwrap(), Box::new(Peek(seen.clone(), Arc::new(Memory::open(&db).unwrap())))));
    c.submit(&json!({"plan": "one", "steps": [{"do": "set", "what": "power.profile", "to": "x"}]}), "human");
    assert_eq!(*seen.lock().unwrap(), Some(json!(1)), "plan was not saved before its first action");
    let _ = std::fs::remove_dir_all(&dir);
}
