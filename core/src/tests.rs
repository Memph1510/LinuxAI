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
    let plan = json!({"plan": "quiet-mode", "why": "you asked for quiet", "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]},
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
    let other = json!({"plan": "x", "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}, "steps": [{"do": "restart", "unit": "nginx.service"}]});
    assert_eq!(c.submit(&other, "ai")["result"]["status"], "waiting:answer");
}

#[test]
fn no_means_no() {
    let (c, fake) = core();
    let plan = json!({"plan": "p", "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}, "steps": [{"do": "restart", "unit": "x.service"}]});
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
    let r = c.handle(&json!({"op": "resolve", "plan_id": p["plan_id"], "steps": [{"do": "restart", "unit": "nginx.service"}], "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}}));
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
    let r = c.submit(&json!({"plan": "p", "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}, "steps": [{"do": "restart", "unit": "x.service"}]}), "ai");
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
        let r = c.submit(&json!({"plan": "p", "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}, "steps": [{"do": "restart", "unit": "x.service"}]}), "ai");
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
    let r = c.handle(&json!({"op": "resolve", "plan_id": id, "steps": [{"do": "stop", "unit": "caddy.service"}], "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}}));
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

// ---- tasks, playbooks, cases -------------------------------------------------------------------

fn core_with_playbooks() -> (Arc<Core>, Fake, std::path::PathBuf) {
    let fake = Fake::default();
    let dir = std::env::temp_dir().join(format!("linuxai-pb-{}-{}", std::process::id(), crate::memory::new_id("")));
    let mut c = Core::new(Memory::in_memory(), Box::new(fake.clone()));
    c.playbook_dir = dir.clone();
    (Arc::new(c), fake, dir)
}

fn waiting_task(c: &Arc<Core>, title: &str) -> Value {
    c.handle(&json!({"op": "tasks"}))["waiting_on_human"].as_array().unwrap().iter()
        .find(|t| t["title"] == title).cloned().unwrap_or_else(|| panic!("'{title}' is not waiting on the human"))
}

fn status_of(tree: &Value, title: &str) -> String {
    if tree["title"] == title { return tree["status"].as_str().unwrap_or("").to_string(); }
    tree["children"].as_array().into_iter().flatten().map(|c| status_of(c, title)).find(|s| !s.is_empty()).unwrap_or_default()
}

#[test]
fn a_macro_task_runs_its_subtasks_in_order_and_coordinates_the_human() {
    let (c, fake, _) = core_with_playbooks();
    let r = c.handle(&json!({"op": "task_create", "source": "human", "spec": {
        "title": "Get the truck a load home from Dallas", "category": "freight/negotiate",
        "context": {"origin": "Dallas", "home": "Atlanta", "empty_at": "Fri 2pm"},
        "subtasks": [
            {"key": "find", "title": "Find loads", "plan": {"steps": [{"do": "set", "what": "power.profile", "to": "searching"},
                                                                      {"let": "best", "value": "Hollis $1,780"}]}},
            {"key": "pick", "title": "Which load?", "kind": "decision", "after": ["find"],
             "ask": {"opts": ["Hollis $1,780", "keep looking"], "why": "Under your floor but gets the truck home", "lean": "Hollis $1,780"}},
            {"key": "sign", "title": "Sign the rate confirmation", "kind": "action", "after": ["pick"]},
            {"key": "book", "title": "Put the pickup on the calendar", "after": ["sign"],
             "plan": {"steps": [{"do": "set", "what": "power.profile", "to": "booked"}]}}]}}));
    assert_eq!(r["ok"], true, "{r}");
    let root = r["task"].as_str().unwrap().to_string();
    let tree = c.handle(&json!({"op": "task", "task": root}))["tree"].clone();
    assert_eq!(status_of(&tree, "Find loads"), "done");
    assert_eq!(status_of(&tree, "Which load?"), "waiting");
    assert_eq!(status_of(&tree, "Sign the rate confirmation"), "pending", "must wait for the decision");
    // The decision reaches the human with its reason; their answer moves the task on.
    let pick = waiting_task(&c, "Which load?");
    let w = c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().iter().find(|p| p["plan_id"] == pick["plan_id"]).cloned().unwrap();
    assert_eq!(w["wait"]["why"], "Under your floor but gets the truck home");
    c.handle(&json!({"op": "answer", "plan_id": pick["plan_id"], "text": "Hollis $1,780"}));
    let sign = waiting_task(&c, "Sign the rate confirmation");
    assert!(c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().iter()
        .any(|p| p["plan_id"] == sign["plan_id"] && p["wait"]["question"].as_str().unwrap().contains("Tell me when it's done")));
    c.handle(&json!({"op": "answer", "plan_id": sign["plan_id"], "text": "done"}));
    let tree = c.handle(&json!({"op": "task", "task": root}))["tree"].clone();
    assert_eq!(tree["status"], "done", "{tree:#}");
    assert_eq!(tree["result"]["pick"]["answer"], "Hollis $1,780");
    assert_eq!(calls(&fake), vec!["set power.profile searching", "set power.profile booked"]);
}

#[test]
fn a_task_without_a_plan_goes_to_the_ai_with_its_playbooks() {
    let (c, _, _) = core_with_playbooks();
    c.handle(&json!({"op": "playbook_put", "category": "freight", "reason": "starting out",
                     "text": "# Freight\n- Floor: $2.40/mi loaded.\n"}));
    c.handle(&json!({"op": "playbook_put", "category": "freight/negotiate", "reason": "learned from the Memphis load",
                     "text": "# Negotiating\n- Under the floor is OK if it gets the truck home for the weekend.\n"}));
    let r = c.handle(&json!({"op": "task_create", "spec": {"title": "Negotiate with Hollis", "category": "freight/negotiate/hollis",
                                                          "context": {"broker": "@hollis", "offer": 1640}}}));
    let id = r["task"].as_str().unwrap().to_string();
    assert_eq!(r["tree"]["status"], "needs_plan");
    let ev = c.handle(&json!({"op": "events", "since": 0, "limit": 1000}));
    let needs = ev["events"].as_array().unwrap().iter().find(|e| e["kind"] == "task.needs_plan").unwrap();
    let cats: Vec<&str> = needs["data"]["playbooks"].as_array().unwrap().iter().map(|p| p["category"].as_str().unwrap()).collect();
    assert_eq!(cats, ["freight", "freight/negotiate"], "broadest first; hollis has no playbook yet");
    assert_eq!(needs["data"]["context"]["offer"], 1640);
    // A plan that doesn't check out goes back to the AI with the problem.
    let bad = c.handle(&json!({"op": "task_plan", "task": id, "plan": {"steps": [{"do": "haggle"}]}}));
    assert_eq!(bad["tree"]["status"], "needs_plan", "{bad}");
    assert!(bad["tree"]["error"].as_str().unwrap().contains("unknown verb"));
    let ok = c.handle(&json!({"op": "task_plan", "task": id, "plan": {"steps": [{"tell": "Countered at $1,950"}]}}));
    assert_eq!(ok["tree"]["status"], "done", "{ok}");
}

#[test]
fn a_failure_stops_what_depends_on_it() {
    let (c, _, _) = core_with_playbooks();
    let r = c.handle(&json!({"op": "task_create", "spec": {"title": "Trip", "subtasks": [
        {"key": "a", "title": "Check the budget", "plan": {"steps": [{"expect": ["<", 900, 718], "else": "stop"}]}},
        {"key": "b", "title": "Book it", "after": ["a"], "plan": {"steps": [{"tell": "booked"}]}}]}}));
    let tree = c.handle(&json!({"op": "task", "task": r["task"]}))["tree"].clone();
    assert_eq!(status_of(&tree, "Check the budget"), "failed");
    assert_eq!(status_of(&tree, "Book it"), "failed");
    assert_eq!(tree["status"], "failed");
    let evs = c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().clone();
    assert!(!evs.iter().any(|e| e["kind"] == "ai.tell" && e["data"]["text"] == "booked"));
}

#[test]
fn playbooks_are_markdown_files_with_history() {
    let (c, _, dir) = core_with_playbooks();
    assert_eq!(c.handle(&json!({"op": "playbook_put", "category": "../etc/passwd", "text": "x", "reason": "r"}))["ok"], false);
    assert_eq!(c.handle(&json!({"op": "playbook_put", "category": "freight", "text": "v1", "reason": ""}))["ok"], false, "a reason is required");
    c.handle(&json!({"op": "playbook_put", "category": "freight/negotiate", "text": "v1\n", "reason": "first"}));
    c.handle(&json!({"op": "playbook_put", "category": "freight/negotiate", "text": "v2\n", "reason": "weather matters in winter"}));
    assert_eq!(std::fs::read_to_string(dir.join("freight/negotiate.md")).unwrap(), "v2\n");
    let h = c.handle(&json!({"op": "playbook_history", "category": "freight/negotiate"}));
    assert_eq!(h["history"][0]["previous_text"], "v1\n");
    assert_eq!(h["history"][0]["replaced_because"], "weather matters in winter");
    assert_eq!(c.handle(&json!({"op": "playbooks"}))["categories"], json!(["freight/negotiate"]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cases_are_recorded_and_found_by_their_variables() {
    let (c, _, _) = core_with_playbooks();
    for (broker, weather, rate, outcome) in [("@hollis", "clear", 2.28, "accepted"), ("@hollis", "snow", 2.55, "accepted"), ("@acme", "clear", 2.10, "declined")] {
        c.handle(&json!({"op": "object_put", "type": "case", "fields": {"category": "freight/negotiate", "broker": broker,
            "weather": weather, "rate": rate, "outcome": outcome}}));
    }
    let found = c.handle(&json!({"op": "find", "spec": {"type": "case", "where": {"broker": "@hollis", "rate": ["<", 2.4]}}}));
    let list = found["found"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["weather"], "clear");
}

#[test]
fn unfinished_tasks_pick_up_again_after_a_restart() {
    let dir = std::env::temp_dir().join(format!("linuxai-taskrecover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("core.db");
    {
        // A task tree saved, then the machine went down before anything started.
        let mem = Memory::open(&db).unwrap();
        mem.save_task(&json!({"id": "tA", "parent": null, "title": "Root", "kind": "work", "category": "general", "status": "pending",
                              "children": ["tB"], "depends": [], "context": {}}));
        mem.save_task(&json!({"id": "tB", "parent": "tA", "title": "Leaf", "kind": "work", "category": "general", "status": "pending",
                              "children": [], "depends": [], "context": {}, "plan": {"steps": [{"tell": "leaf ran"}]}}));
    }
    let c = Arc::new(Core::new(Memory::open(&db).unwrap(), Box::new(Fake::default())));
    c.recover();
    assert_eq!(c.handle(&json!({"op": "task", "task": "tA"}))["tree"]["status"], "done");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_ai_task_with_an_untrusted_action_waits_on_the_human() {
    let (c, fake, _) = core_with_playbooks();
    let r = c.handle(&json!({"op": "task_create", "spec": {"title": "Quiet the fans",
        "plan": {"assess": {"confidence": 0.95, "consequences": ["changes a machine setting"], "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}, "steps": [{"do": "set", "what": "power.profile", "to": "power-saver"}]}}}));
    assert_eq!(r["tree"]["status"], "waiting", "{r}");
    assert!(calls(&fake).is_empty());
    let t = waiting_task(&c, "Quiet the fans");
    c.handle(&json!({"op": "answer", "plan_id": t["plan_id"], "text": "yes"}));
    assert_eq!(c.handle(&json!({"op": "task", "task": r["task"]}))["tree"]["status"], "done");
}

#[test]
fn find_compares_against_variables() {
    let (c, _) = core();
    c.mem.put_object("case", None, &json!({"rate": 2.28}));
    c.mem.put_object("case", None, &json!({"rate": 2.55}));
    let r = c.submit(&json!({"plan": "q", "steps": [{"let": "floor", "value": 2.4},
        {"let": "under", "find": {"type": "case", "where": {"rate": ["<", "$floor"]}}},
        {"expect": ["=", ["count", "$under"], 1], "else": "stop"}]}), "human");
    assert_eq!(r["result"]["status"], "done", "{r}");
}

// ---- second look: assess before acting -----------------------------------------------------------

fn ai_plan(extra: Value) -> Value {
    let mut p = json!({"plan": "tune", "steps": [{"do": "set", "what": "power.profile", "to": "balanced"}]});
    for (k, v) in extra.as_object().unwrap() { p[k] = v.clone(); }
    p
}

fn trust_set(c: &Arc<Core>) { c.mem.add_mandate("set", "*", "test"); }

#[test]
fn the_ai_must_assess_before_acting() {
    let (c, fake) = core();
    trust_set(&c);
    let r = c.submit(&ai_plan(json!({})), "ai");
    assert_eq!(r["ok"], false);
    assert!(r["report"]["errors"][0].as_str().unwrap().contains("assess"), "{r}");
    let r = c.submit(&ai_plan(json!({"assess": {"confidence": 0.9, "consequences": ["x"]}})), "ai");
    assert_eq!(r["ok"], false, "outcomes are required: {r}");
    // Trusted, confident, understood, familiar: it just acts.
    let r = c.submit(&ai_plan(json!({"assess": {"confidence": 0.9, "consequences": ["fans get louder"],
        "outcomes": [{"if": "too loud", "then": "back to power-saver"}]}})), "ai");
    assert_eq!(r["result"]["status"], "done", "{r}");
    assert_eq!(calls(&fake).len(), 1);
    // The human's own plans and the system's rules don't need an assessment.
    assert_eq!(c.submit(&ai_plan(json!({})), "human")["result"]["status"], "done");
}

#[test]
fn unknowns_or_low_confidence_get_a_second_look_even_when_trusted() {
    for assess in [json!({"confidence": 0.9, "consequences": ["x"], "outcomes": ["y"], "unknowns": ["whether you're on a call"]}),
                   json!({"confidence": 0.5, "consequences": ["x"], "outcomes": ["y"]})] {
        let (c, fake) = core();
        trust_set(&c);
        let r = c.submit(&ai_plan(json!({"assess": assess})), "ai");
        assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
        let w = &r["result"]["wait"];
        assert!(w["why"].as_str().unwrap().contains("second look"), "{w}");
        assert!(w["question"].as_str().unwrap().contains("What I expect: x"), "{w}");
        assert!(calls(&fake).is_empty());
        // "always" here does not make unknowns go away next time: it only trusts actions, not situations.
        let id = r["result"]["plan_id"].as_str().unwrap().to_string();
        assert_eq!(c.handle(&json!({"op": "answer", "plan_id": id, "text": "yes"}))["result"]["status"], "done");
    }
}

#[test]
fn a_first_time_situation_needs_research_and_a_conversation() {
    let (c, _, _) = core_with_playbooks();
    trust_set(&c);
    let base = json!({"confidence": 0.9, "consequences": ["counter the broker"], "outcomes": [{"if": "they refuse", "then": "hold"}]});
    let mut plan = ai_plan(json!({"category": "freight/negotiate", "assess": base.clone()}));
    let r = c.submit(&plan, "ai");
    assert_eq!(r["ok"], false);
    assert!(r["report"]["errors"][0].as_str().unwrap().contains("first time I'm handling freight/negotiate"), "{r}");
    plan["assess"]["research"] = json!(["lane averages $2.21/mi this month", "no past Hollis loads"]);
    let r = c.submit(&plan, "ai");
    assert_eq!(r["result"]["status"], "waiting:answer", "first time: talk it over even with research: {r}");
    assert!(r["result"]["wait"]["question"].as_str().unwrap().contains("What I looked into: lane averages"));
    // Once there's experience (a case, or a playbook), it's no longer a first time.
    c.handle(&json!({"op": "object_put", "type": "case", "fields": {"category": "freight/negotiate", "decision": "countered"}}));
    plan["assess"] = base;
    assert_eq!(c.submit(&plan, "ai")["result"]["status"], "done");
    // A narrower category counts as related experience too.
    let narrower = ai_plan(json!({"category": "freight/negotiate/hollis", "assess": plan["assess"].clone()}));
    assert_eq!(c.submit(&narrower, "ai")["result"]["status"], "done");
}

#[test]
fn resolving_a_blocker_needs_an_assessment_too() {
    let (c, _) = core();
    c.emit("service.failed", json!({"unit": "nginx.service"}));
    let id = c.handle(&json!({"op": "waiting"}))["plans"][0]["plan_id"].as_str().unwrap().to_string();
    let r = c.handle(&json!({"op": "resolve", "plan_id": id, "steps": [{"do": "restart", "unit": "nginx.service"}]}));
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["report"]["errors"][0].as_str().unwrap().contains("assess"));
}

// ---- pruning ------------------------------------------------------------------------------------

#[test]
fn old_history_becomes_daily_digests_and_nothing_in_progress_is_lost() {
    let (c, _) = core();
    let day = 86400.0;
    let old = crate::memory::now() - 40.0 * day;
    for i in 0..50 { c.mem.add_event_at(old + i as f64, "power.unplugged", &json!({})); }
    c.mem.add_event_at(old + 100.0, "ai.tell", &json!({"text": "Battery is low"}));
    c.mem.add_event_at(old + 200.0, "task.failed", &json!({"title": "Book hotel", "error": "sold out"}));
    // A plan that's still waiting (old) and one that finished long ago.
    let w = c.submit(&json!({"plan": "still-waiting", "steps": [{"ask": "ok?"}]}), "human");
    let f = c.submit(&json!({"plan": "long-done", "steps": [{"tell": "hi"}]}), "human");
    let fin_id = f["result"]["plan_id"].as_str().unwrap().to_string();
    let (w_id, mut fin) = (w["result"]["plan_id"].as_str().unwrap().to_string(), c.mem.load_plan(&fin_id).unwrap());
    fin["status"] = json!("done");
    c.mem.save_plan(&fin_id, "long-done", "human", "done", &fin);
    let dry = c.handle(&json!({"op": "prune", "dry_run": true}))["pruned"].clone();
    assert_eq!(dry["events"], 52);
    assert_eq!(c.handle(&json!({"op": "stats"}))["stats"]["events"].as_i64().unwrap() >= 52, true, "dry run must not delete");
    let r = c.handle(&json!({"op": "prune"}))["pruned"].clone();
    assert_eq!(r["events"], 52);
    assert_eq!(r["digests"], 1);
    let digests = c.mem.objects_of("digest", 10);
    assert_eq!(digests[0]["counts"]["power.unplugged"], 50);
    let notable: Vec<String> = digests[0]["notable"].as_array().unwrap().iter().map(|n| n["kind"].as_str().unwrap().to_string()).collect();
    assert!(notable.contains(&"ai.tell".to_string()) && notable.contains(&"task.failed".to_string()));
    assert!(!notable.contains(&"power.unplugged".to_string()), "routine events are counted, not listed");
    // Recent events stay; the waiting plan is untouched.
    assert!(c.mem.load_plan(&w_id).is_some());
    assert_eq!(c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().len(), 1);
}

#[test]
fn finished_plans_and_old_playbook_versions_are_pruned() {
    let (c, _, _) = core_with_playbooks();
    let mut ret = c.retention.clone();
    ret.plans_days = 0.0;
    ret.playbook_versions = 3;
    for i in 0..6 {
        c.handle(&json!({"op": "playbook_put", "category": "freight", "text": format!("v{i}"), "reason": "r"}));
    }
    let done = c.submit(&json!({"plan": "d", "steps": [{"tell": "x"}]}), "human");
    let waiting = c.submit(&json!({"plan": "w", "steps": [{"ask": "?"}]}), "human");
    std::thread::sleep(std::time::Duration::from_millis(20));
    let r = c.mem.prune(&ret, false);
    assert_eq!(r["plans"], 1, "{r}");
    assert!(c.mem.load_plan(done["result"]["plan_id"].as_str().unwrap()).is_none());
    assert!(c.mem.load_plan(waiting["result"]["plan_id"].as_str().unwrap()).is_some());
    assert_eq!(c.handle(&json!({"op": "playbook_history", "category": "freight"}))["history"].as_array().unwrap().len(), 3);
    assert_eq!(c.handle(&json!({"op": "playbook", "category": "freight"}))["own"], "v5");
}

#[test]
fn repeated_prunes_merge_into_one_digest_per_day() {
    let (c, _) = core();
    let mut ret = c.retention.clone();
    ret.events_days = 0.0;
    c.mem.add_event_at(crate::memory::now() - 10.0, "test.a", &json!({}));
    c.mem.prune(&ret, false);
    c.mem.add_event_at(crate::memory::now() - 5.0, "test.a", &json!({}));
    c.mem.prune(&ret, false);
    let d = c.mem.objects_of("digest", 10);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0]["counts"]["test.a"], 2);
}

#[test]
fn finished_task_trees_expire_unfinished_ones_never_do() {
    let (c, _, _) = core_with_playbooks();
    let done = c.handle(&json!({"op": "task_create", "source": "human", "spec": {"title": "Old job", "subtasks": [
        {"title": "a", "plan": {"steps": [{"tell": "a"}]}}, {"title": "b", "plan": {"steps": [{"tell": "b"}]}}]}}));
    let open = c.handle(&json!({"op": "task_create", "source": "human", "spec": {"title": "Open job", "subtasks": [
        {"title": "decide", "kind": "decision"}]}}));
    assert_eq!(done["tree"]["status"], "done");
    let mut ret = c.retention.clone();
    ret.tasks_days = 0.0;
    std::thread::sleep(std::time::Duration::from_millis(20));
    let r = c.mem.prune(&ret, false);
    assert_eq!(r["tasks"], 3, "{r}");
    assert!(c.mem.load_task(done["task"].as_str().unwrap()).is_none());
    assert_eq!(c.handle(&json!({"op": "task", "task": open["task"]}))["tree"]["children"].as_array().unwrap().len(), 1);
}

// ---- playbook protections ------------------------------------------------------------------------

fn pb_edit(c: &Arc<Core>, edits: Value, validation: Value, source: &str) -> Value {
    c.handle(&json!({"op": "playbook_edit", "category": "freight/negotiate", "edits": edits, "validation": validation,
                     "reason": "learned from a load", "source": source}))
}

fn pb_file(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("freight/negotiate.md")).unwrap_or_default()
}

#[test]
fn playbooks_change_by_small_edits_and_render_to_markdown() {
    let (c, _, dir) = core_with_playbooks();
    let r = pb_edit(&c, json!([
        {"op": "add", "section": "How I decide", "text": "Floor is $2.40/mi loaded."},
        {"op": "add", "section": "Exceptions", "text": "Under the floor is OK if it gets the truck home for the weekend.", "evidence": ["Memphis load"]}]),
        json!([]), "ai");
    assert_eq!(r["applied"], true, "{r}");
    let md = pb_file(&dir);
    assert!(md.contains("## How I decide\n- Floor is $2.40/mi loaded."), "{md}");
    assert!(md.contains("## Exceptions\n- Under the floor"), "{md}");
    let entries = c.handle(&json!({"op": "playbook_entries", "category": "freight/negotiate"}))["entries"].clone();
    let floor = entries[0]["id"].as_str().unwrap().to_string();
    // Revise one point: the rest stays exactly as it was.
    let r = pb_edit(&c, json!([{"op": "revise", "id": floor, "text": "Floor is $2.45/mi loaded (fuel went up)."}]), json!([]), "ai");
    assert_eq!(r["applied"], true, "{r}");
    let md2 = pb_file(&dir);
    assert!(md2.contains("$2.45") && md2.contains("Under the floor is OK"), "{md2}");
    // A duplicate is refused; feedback counts show up in the file.
    let dup = pb_edit(&c, json!([{"op": "add", "section": "How I decide", "text": "The floor is $2.45/mi loaded (fuel went up)."}]), json!([]), "ai");
    assert!(dup["errors"][0].as_str().unwrap().contains("already covered"), "{dup}");
    c.handle(&json!({"op": "playbook_feedback", "id": floor, "helped": true}));
    assert!(pb_file(&dir).contains(&format!("{floor} · helped 1 · hurt 0")));
    // History keeps the previous file.
    assert!(c.handle(&json!({"op": "playbook_history", "category": "freight/negotiate"}))["history"][0]["previous_text"].as_str().unwrap().contains("$2.40"));
}

#[test]
fn the_ai_cannot_rewrite_a_whole_playbook() {
    let (c, _, _) = core_with_playbooks();
    pb_edit(&c, json!([{"op": "add", "section": "How I decide", "text": "Floor is $2.40/mi."}]), json!([]), "ai");
    let r = c.handle(&json!({"op": "playbook_put", "category": "freight/negotiate", "text": "# tiny\n", "reason": "simplify", "source": "ai"}));
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("small edits"));
}

#[test]
fn ai_edits_must_be_checked_against_past_cases() {
    let (c, _, _) = core_with_playbooks();
    for (b, rate, d) in [("@hollis", 2.28, "accepted under the floor to get home"), ("@acme", 2.10, "declined")] {
        c.handle(&json!({"op": "object_put", "type": "case", "fields": {"category": "freight/negotiate", "broker": b, "rate": rate,
                                                                        "decision": d, "outcome": "fine"}}));
    }
    let edit = json!([{"op": "add", "section": "How I decide", "text": "Never go under the floor."}]);
    let r = pb_edit(&c, edit.clone(), json!([]), "ai");
    assert_eq!(r["ok"], false);
    let ids: Vec<String> = r["needs_validation"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids.len(), 2, "{r}");
    // It would have made the Hollis decision worse: held for the human, not saved.
    let v = json!([{"case": ids[0], "verdict": "same", "note": ""}, {"case": ids[1], "verdict": "same", "note": ""}]);
    let hollis = if r["needs_validation"][0]["broker"] == "@hollis" { 0 } else { 1 };
    let mut v_worse = v.clone();
    v_worse[hollis]["verdict"] = json!("worse");
    v_worse[hollis]["note"] = json!("would have driven home empty");
    let r = pb_edit(&c, edit.clone(), v_worse, "ai");
    assert_eq!(r["applied"], false, "{r}");
    assert!(r["held"][0].as_str().unwrap().contains("worse"));
    assert!(c.handle(&json!({"op": "playbook_entries", "category": "freight/negotiate"}))["entries"].as_array().unwrap().is_empty());
    // The human says no: nothing changes. A clean validation goes straight in.
    c.handle(&json!({"op": "answer", "plan_id": r["plan_id"], "text": "no"}));
    assert!(c.handle(&json!({"op": "playbook_entries", "category": "freight/negotiate"}))["entries"].as_array().unwrap().is_empty());
    let r = pb_edit(&c, json!([{"op": "add", "section": "What matters", "text": "Weekend home time for the driver."}]), v, "ai");
    assert_eq!(r["applied"], true, "{r}");
}

#[test]
fn held_changes_apply_when_the_human_agrees() {
    let (c, _, dir) = core_with_playbooks();
    // The human's own point.
    pb_edit(&c, json!([{"op": "add", "section": "How I decide", "text": "Never haul for Acme again."}]), json!([]), "human");
    let id = c.handle(&json!({"op": "playbook_entries", "category": "freight/negotiate"}))["entries"][0]["id"].as_str().unwrap().to_string();
    let r = pb_edit(&c, json!([{"op": "retire", "id": id}]), json!([]), "ai");
    assert_eq!(r["applied"], false, "{r}");
    assert!(r["held"][0].as_str().unwrap().contains("points you wrote"));
    assert!(pb_file(&dir).contains("Never haul for Acme"));
    c.handle(&json!({"op": "answer", "plan_id": r["plan_id"], "text": "yes"}));
    assert!(!pb_file(&dir).contains("Never haul for Acme"), "{}", pb_file(&dir));
}

#[test]
fn changing_much_of_a_playbook_at_once_is_held() {
    let (c, _, _) = core_with_playbooks();
    let adds: Vec<Value> = (0..6).map(|i| json!({"op": "add", "section": "How I decide", "text": format!("Rule number {i} about lane {i}")})).collect();
    pb_edit(&c, json!(adds), json!([]), "ai");
    let ids: Vec<String> = c.handle(&json!({"op": "playbook_entries", "category": "freight/negotiate"}))["entries"].as_array().unwrap()
        .iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
    let retire: Vec<Value> = ids[..3].iter().map(|i| json!({"op": "retire", "id": i})).collect();
    let r = pb_edit(&c, json!(retire), json!([]), "ai");
    assert_eq!(r["applied"], false, "{r}");
    assert!(r["held"][0].as_str().unwrap().contains("3 of 6"));
}

#[test]
fn the_humans_edits_to_the_file_are_read_back_in() {
    let (c, _, dir) = core_with_playbooks();
    pb_edit(&c, json!([{"op": "add", "section": "How I decide", "text": "Floor is $2.40/mi."},
                      {"op": "add", "section": "Exceptions", "text": "OK under the floor to get home."}]), json!([]), "ai");
    let md = pb_file(&dir);
    // The human changes a line, deletes one, and adds a new section and line in their editor.
    let edited = md.replace("Floor is $2.40/mi.", "Floor is $2.50/mi.")
        .lines().filter(|l| !l.contains("OK under the floor")).collect::<Vec<_>>().join("\n")
        + "\n\n## Weather\n- Snow in Atlanta adds a day.\n";
    std::fs::write(dir.join("freight/negotiate.md"), edited).unwrap();
    let entries = c.handle(&json!({"op": "playbook_entries", "category": "freight/negotiate"}))["entries"].as_array().unwrap().clone();
    let texts: Vec<&str> = entries.iter().map(|e| e["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["Floor is $2.50/mi.", "Snow in Atlanta adds a day."], "{entries:?}");
    assert!(entries.iter().all(|e| e["by"] == "human"));
    // New lines got ids, so the next edit maps to the same entry.
    assert!(pb_file(&dir).contains("Snow in Atlanta adds a day.  <!-- e"));
    // And the AI now needs the human's OK to change the point they edited.
    let r = pb_edit(&c, json!([{"op": "revise", "id": entries[0]["id"], "text": "Floor is $2.30/mi."}]), json!([]), "ai");
    assert_eq!(r["applied"], false);
}

#[test]
fn points_that_keep_hurting_get_flagged() {
    let (c, _, dir) = core_with_playbooks();
    pb_edit(&c, json!([{"op": "add", "section": "How I decide", "text": "Always counter at $2.60."}]), json!([]), "ai");
    let id = c.handle(&json!({"op": "playbook_entries", "category": "freight/negotiate"}))["entries"][0]["id"].as_str().unwrap().to_string();
    for _ in 0..3 { c.handle(&json!({"op": "playbook_feedback", "id": id, "helped": false})); }
    assert!(pb_file(&dir).contains("often wrong"));
    let evs = c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().clone();
    assert!(evs.iter().any(|e| e["kind"] == "playbook.entry_questioned"));
}

// ---- outside information: facts, never commands ------------------------------------------------

fn offer(c: &Arc<Core>, authenticated: bool) -> String {
    let r = c.handle(&json!({"op": "object_put", "type": "load_offer", "fields": {
        "trust": "outside", "origin": "Dallas, TX", "destination": "Atlanta, GA", "rate_cents": 178000, "miles": 781,
        "source": {"kind": "email", "from": "dana@hollis-logistics.com", "authenticated": authenticated, "message": "obj:message/x"}}}));
    r["id"].as_str().unwrap().to_string()
}

fn outside_plan(facts: Value) -> Value {
    json!({"plan": "accept the Hollis load", "taint": "outside", "category": "general",
           "assess": {"confidence": 0.9, "consequences": ["the load is booked"], "outcomes": [{"if": "rate con differs", "then": "stop"}]},
           "reasoning": {"facts": facts},
           "steps": [{"do": "set", "what": "power.profile", "to": "accept-hollis"}]})
}

#[test]
fn outside_information_must_be_cited() {
    let (c, _) = core();
    let r = c.submit(&outside_plan(json!([])), "ai");
    assert_eq!(r["ok"], false);
    assert!(r["report"]["errors"][0].as_str().unwrap().contains("cite the stored facts"), "{r}");
    let r = c.submit(&outside_plan(json!(["obj:load_offer/made-up"])), "ai");
    assert!(r["report"]["errors"][0].as_str().unwrap().contains("no stored fact"), "{r}");
}

#[test]
fn ordinary_trust_does_not_cover_acting_on_outside_information() {
    let (c, fake) = core();
    trust_set(&c); // the AI may change this setting on its own...
    let id = offer(&c, false);
    let r = c.submit(&outside_plan(json!([id])), "ai");
    // ...but not because of what an email said.
    assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
    let q = r["result"]["wait"]["question"].as_str().unwrap();
    assert!(q.contains("Based on outside information") && q.contains("dana@hollis-logistics.com") && q.contains("sender NOT verified"), "{q}");
    assert!(q.contains("rate_cents: 178000"), "the human sees the core's record, not the AI's retelling: {q}");
    assert!(r["result"]["wait"]["why"].as_str().unwrap().contains("information from outside"));
    assert!(calls(&fake).is_empty());
    // "always" gives standing permission for this kind of action based on outside information.
    c.handle(&json!({"op": "answer", "plan_id": r["result"]["plan_id"], "text": "always"}));
    assert_eq!(calls(&fake).len(), 1);
    let id2 = offer(&c, true);
    assert_eq!(c.submit(&outside_plan(json!([id2])), "ai")["result"]["status"], "done");
    // That permission doesn't leak into ordinary trust for a different action.
    let mut other = outside_plan(json!([id2]));
    other["steps"] = json!([{"do": "restart", "unit": "x.service"}]);
    assert_eq!(c.submit(&other, "ai")["result"]["status"], "waiting:answer");
}

#[test]
fn a_task_from_outside_information_labels_its_plans_whatever_the_ai_says() {
    let (c, fake, _) = core_with_playbooks();
    trust_set(&c);
    let r = c.handle(&json!({"op": "task_create", "spec": {"title": "Answer the Hollis offer", "trust": "outside"}}));
    let task = r["task"].as_str().unwrap().to_string();
    // The AI's plan doesn't mention the label and cites nothing: refused.
    let plan = json!({"assess": {"confidence": 0.9, "consequences": ["x"], "outcomes": ["y"]},
                      "steps": [{"do": "set", "what": "power.profile", "to": "x"}]});
    let r = c.handle(&json!({"op": "task_plan", "task": task, "plan": plan}));
    assert_eq!(r["tree"]["status"], "needs_plan", "{r}");
    assert!(r["tree"]["error"].as_str().unwrap().contains("outside information"), "{r}");
    assert!(calls(&fake).is_empty());
}

#[test]
fn the_human_sees_the_reasoning_modules() {
    let (c, _) = core();
    let id = offer(&c, true);
    let mut p = outside_plan(json!([id]));
    p["reasoning"]["policies"] = json!(["e123: under the floor is OK to get home for the weekend"]);
    p["reasoning"]["chain"] = json!(["offer is $2.28/mi", "below the $2.40 floor", "but it gets the truck home"]);
    p["reasoning"]["decision"] = json!("accept $1,780");
    p["reasoning"]["downstream"] = json!(["pickup Friday 4pm goes on the calendar"]);
    let q = c.submit(&p, "ai")["result"]["wait"]["question"].as_str().unwrap().to_string();
    for part in ["Rules applied: e123", "Reasoning: offer is $2.28/mi → below", "Decision: accept $1,780",
                 "If this runs: pickup Friday", "Based on outside information"] {
        assert!(q.contains(part), "missing {part:?} in {q}");
    }
}

#[test]
fn claims_show_their_checks_and_a_corrected_fact_cannot_be_cited() {
    let (c, _) = core();
    let old = offer(&c, true);
    let new = offer(&c, true);
    c.handle(&json!({"op": "object_patch", "id": new, "fields": {"corrects": old, "checks": ["rate works out to $2.28/mi, the email says $2.38/mi"],
        "reliability": {"track_record": "12 messages, 2 corrections, 1 contradicted"}, "source": {"from": "forged"}}}));
    c.handle(&json!({"op": "object_patch", "id": old, "fields": {"superseded_by": new}}));
    let r = c.submit(&outside_plan(json!([old])), "ai");
    assert!(r["report"]["errors"][0].as_str().unwrap().contains("was corrected by"), "{r}");
    let q = c.submit(&outside_plan(json!([new])), "ai")["result"]["wait"]["question"].as_str().unwrap().to_string();
    assert!(q.contains("⚠ rate works out to $2.28/mi") && q.contains("sender's record: 12 messages") && q.contains("corrects"), "{q}");
    assert!(q.contains("dana@hollis-logistics.com"), "provenance can't be patched: {q}");
}

// ---- prompt library --------------------------------------------------------------------------------

#[test]
fn the_prompt_library_lists_fills_counts_and_holds_ai_changes() {
    let (c0, _, dir) = core_with_playbooks();
    let mut c = Arc::try_unwrap(c0).ok().unwrap();
    c.prompt_dir = dir.join("prompts");
    let c = Arc::new(c);
    let counter = "---\ntitle: Counter a broker's offer\nuse: freight/negotiate\nmodel: any\ntags: email, negotiation\n---\nReply to {{broker}}: we can do {{lane}} at {{counter}}.";
    assert_eq!(c.handle(&json!({"op": "prompt_put", "name": "freight/counter-offer", "text": counter}))["ok"], true);
    c.handle(&json!({"op": "prompt_put", "name": "email/decline", "text": "---\ntitle: Decline politely\nmodel: local\n---\nDecline {{what}} kindly."}));
    // Search and filter.
    let l = c.handle(&json!({"op": "prompts", "query": "broker"}))["prompts"].clone();
    assert_eq!(l.as_array().unwrap().len(), 1);
    assert_eq!(l[0]["slots"], json!(["broker", "lane", "counter"]));
    assert_eq!(c.handle(&json!({"op": "prompts", "model": "claude"}))["prompts"].as_array().unwrap().len(), 1, "local-only prompts are filtered out");
    // Fill in; missing slots are refused.
    let r = c.handle(&json!({"op": "prompt_render", "name": "freight/counter-offer", "vars": {"broker": "Dana", "lane": "DAL-ATL"}}));
    assert_eq!(r["missing"], json!(["counter"]));
    let r = c.handle(&json!({"op": "prompt_render", "name": "freight/counter-offer", "vars": {"broker": "Dana", "lane": "DAL-ATL", "counter": "$1,950"}}));
    assert_eq!(r["text"], "Reply to Dana: we can do DAL-ATL at $1,950.");
    // Feedback ranks prompts.
    c.handle(&json!({"op": "prompt_feedback", "name": "email/decline", "helped": true}));
    c.handle(&json!({"op": "prompt_feedback", "name": "email/decline", "helped": true}));
    assert_eq!(c.handle(&json!({"op": "prompts"}))["prompts"][0]["name"], "email/decline");
    // The AI's suggested change is held for the human; "yes" writes it.
    let better = counter.replace("Reply to", "Thank {{broker}} and reply to");
    let r = c.handle(&json!({"op": "prompt_suggest", "name": "freight/counter-offer", "text": better, "reason": "brokers respond better to thanks"}));
    assert_eq!(r["held"], true, "{r}");
    assert!(!std::fs::read_to_string(dir.join("prompts/freight/counter-offer.md")).unwrap().contains("Thank"));
    c.handle(&json!({"op": "answer", "plan_id": r["plan_id"], "text": "yes"}));
    assert!(std::fs::read_to_string(dir.join("prompts/freight/counter-offer.md")).unwrap().contains("Thank"));
    assert_eq!(c.handle(&json!({"op": "prompt_put", "name": "../etc/x", "text": "---\ntitle: x\n---\ny"}))["ok"], false);
    let _ = std::fs::remove_dir_all(&dir);
}
