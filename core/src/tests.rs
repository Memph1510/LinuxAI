use crate::core::Core;
use crate::engine;
use crate::host;
use crate::memory::Memory;
use crate::verbs::Organs;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use crate::crash::Locked;

#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Organs for Fake {
    fn invoke(&self, verb: &str, args: &Value) -> Result<Value, String> {
        self.calls.lk().push((verb.to_string(), args.clone()));
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

fn core_holding(hold: f64) -> (Arc<Core>, Fake) {
    let fake = Fake::default();
    let mut c = Core::new(Memory::in_memory(), Box::new(fake.clone()));
    c.habit_hold = hold;
    (Arc::new(c), fake)
}

fn calls(f: &Fake) -> Vec<String> {
    f.calls.lk().iter().map(|(v, a)| format!("{v} {} {}", a["what"].as_str().unwrap_or(""), a["to"].as_str().unwrap_or(""))).collect()
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
    let r = c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": "yes, always"}));
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
    let r = c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": "no, not now"}));
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
    let r = c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": "pay"}));
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
    let r = c.handle(&json!({"op": "answer", "source": "human", "plan_id": p["plan_id"], "text": "yes"}));
    assert_eq!(r["result"]["status"], "done", "{r}");
    assert_eq!(fake.calls.lk()[0].0, "restart");
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
        self.calls.lk().push((verb.to_string(), args.clone()));
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
        let r = c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": reply}));
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": "yes"}));
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": ask["plan_id"], "text": "yes"}));
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
                let r = c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": "go"}));
                assert_eq!(r["result"]["status"], "done", "{r}");
            }
        })
    }).collect();
    for t in threads { t.join().unwrap(); }
    let sets = fake.calls.lk().len();
    assert!(sets >= 200, "only {sets} actions ran");
    assert!(c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().is_empty());
}

#[test]
fn a_plan_is_saved_before_its_first_step_runs() {
    #[derive(Clone)]
    struct Peek(Arc<Mutex<Option<Value>>>, Arc<Memory>);
    impl Organs for Peek {
        fn invoke(&self, _: &str, _: &Value) -> Result<Value, String> {
            *self.0.lk() = Some(json!(self.1.plans_with_status("running").len()));
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
    assert_eq!(*seen.lk(), Some(json!(1)), "plan was not saved before its first action");
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": pick["plan_id"], "text": "Hollis $1,780"}));
    let sign = waiting_task(&c, "Sign the rate confirmation");
    assert!(c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().iter()
        .any(|p| p["plan_id"] == sign["plan_id"] && p["wait"]["question"].as_str().unwrap().contains("Tell me when it's done")));
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": sign["plan_id"], "text": "done"}));
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": t["plan_id"], "text": "yes"}));
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
        assert_eq!(c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": "yes"}))["result"]["status"], "done");
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": r["plan_id"], "text": "no"}));
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": r["plan_id"], "text": "yes"}));
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": r["result"]["plan_id"], "text": "always"}));
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
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": r["plan_id"], "text": "yes"}));
    assert!(std::fs::read_to_string(dir.join("prompts/freight/counter-offer.md")).unwrap().contains("Thank"));
    assert_eq!(c.handle(&json!({"op": "prompt_put", "name": "../etc/x", "text": "---\ntitle: x\n---\ny"}))["ok"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_full_disk_is_reported_not_hidden() {
    let (core, _fake) = core();
    assert_eq!(core.handle(&json!({"op": "ping"}))["memory"]["healthy"], true);
    core.mem.refuse_writes();
    let r = core.handle(&json!({"op": "task_create", "source": "human", "spec": {"title": "Boot test", "subtasks": [{"title": "Pick one", "kind": "decision"}]}}));
    assert_eq!(r["ok"], false, "a task that couldn't be saved isn't reported as created: {r}");
    assert!(r["error"].as_str().unwrap().contains("couldn't save"), "{r}");
    let p = core.handle(&json!({"op": "ping"}));
    assert_eq!(p["ok"], true, "the core still answers");
    assert_eq!(p["memory"]["healthy"], false, "{p}");
    assert!(p["memory"]["failed_writes"].as_u64().unwrap() > 0);
}

#[test]
fn finished_work_says_what_was_checked_and_what_wasnt() {
    let (c, _, _) = core_with_playbooks();
    // The AI finishing a task itself has to say how it knows.
    let r = c.handle(&json!({"op": "task_create", "spec": {"title": "Find the cheapest diesel near Memphis"}}));
    let id = r["task"].as_str().unwrap().to_string();
    let r = c.handle(&json!({"op": "task_done", "task": id, "result": "Pilot on I-40, $3.41"}));
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"].as_str().unwrap().contains("checked"));
    let r = c.handle(&json!({"op": "task_done", "task": id, "result": "Pilot on I-40, $3.41", "evidence": {
        "checked": ["prices on GasBuddy for 6 stations within 10 miles, 20 minutes ago"],
        "not_checked": ["whether the price holds until you get there"], "confidence": 0.8}}));
    assert_eq!(r["ok"], true, "{r}");
    let t = c.handle(&json!({"op": "task", "task": id}))["tree"].clone();
    assert_eq!(t["evidence"]["not_checked"][0], "whether the price holds until you get there");
    assert_eq!(t["evidence"]["by"], "ai");

    // Done by a plan: what ran, plus what its own steps checked; a plan that checked nothing doesn't look verified.
    let r = c.handle(&json!({"op": "task_create", "source": "human", "spec": {"title": "Quiet the fans",
        "plan": {"steps": [{"do": "set", "what": "power.profile", "to": "quiet"}]}}}));
    let t = c.handle(&json!({"op": "task", "task": r["task"]}))["tree"].clone();
    assert_eq!(t["status"], "done", "{t}");
    assert_eq!(t["evidence"]["by"], "plan");
    assert!(t["evidence"]["not_checked"][0].as_str().unwrap().contains("no step of the plan checked it"), "{t}");
    let r = c.handle(&json!({"op": "task_create", "source": "human", "spec": {"title": "Quiet the fans, checked",
        "plan": {"assess": {"confidence": 0.9, "consequences": ["quieter"], "outcomes": ["fine"], "verify": ["the profile reads back as quiet"]},
                 "steps": [{"do": "set", "what": "power.profile", "to": "quiet"}]}}}));
    let t = c.handle(&json!({"op": "task", "task": r["task"]}))["tree"].clone();
    assert_eq!(t["evidence"]["checked"][1], "the profile reads back as quiet", "{t}");
    assert_eq!(t["evidence"]["not_checked"], json!([]));
}

#[test]
fn a_solved_situation_becomes_a_habit_replayed_without_the_model_until_it_fails() {
    let (c, fake) = core_holding(0.0); // the situation staying away is tested on its own below
    trust_set(&c);
    let thinking = |c: &Arc<Core>| -> Vec<Value> {
        c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().iter().filter(|p| p["status"] == "needs_thought").cloned().collect()
    };
    let kinds = |c: &Arc<Core>| -> Vec<String> {
        c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
            .map(|e| e["kind"].as_str().unwrap_or("").to_string()).collect()
    };
    let assess = json!({"confidence": 0.9, "consequences": ["the cache is dropped"], "outcomes": ["memory frees up"]});
    let fix = json!([{"do": "set", "what": "{$event.data.target}", "to": "dropped"}]);

    // First time: the AI is woken and works it out.
    c.emit("memory.pressure", json!({"available_mb": 120, "target": "page-cache"}));
    let w = thinking(&c);
    assert_eq!(w.len(), 1, "the AI is asked: {w:?}");
    assert_eq!(w[0]["wait"]["habit"], "memory.pressure");
    let r = c.handle(&json!({"op": "resolve", "plan_id": w[0]["plan_id"], "steps": fix, "assess": assess}));
    assert_eq!(r["ok"], true, "{r}");
    assert!(!kinds(&c).contains(&"habit.learned".to_string()), "finishing isn't working: it counts once the situation stays away");

    // Next time: the same situation is handled with no model at all, through the usual checks.
    c.emit("memory.pressure", json!({"available_mb": 110, "target": "page-cache"}));
    assert!(kinds(&c).contains(&"habit.learned".to_string()), "it stayed away long enough, so it's remembered");
    assert!(thinking(&c).is_empty(), "the AI isn't woken");
    assert!(kinds(&c).contains(&"habit.used".to_string()));
    assert_eq!(calls(&fake).iter().filter(|x| x.contains("page-cache dropped")).count(), 2);

    // When the replay fails, the habit is retired and the AI thinks fresh next time.
    c.emit("memory.pressure", json!({"available_mb": 90, "target": "explode"}));
    assert!(kinds(&c).contains(&"habit.retired".to_string()));
    c.emit("memory.pressure", json!({"available_mb": 80, "target": "page-cache"}));
    assert_eq!(thinking(&c).len(), 1, "back to thinking");

    // A situation it hasn't solved (another service) still wakes the AI; the human can see and forget habits.
    c.emit("service.failed", json!({"unit": "cups"}));
    assert_eq!(thinking(&c).iter().filter(|p| p["wait"]["habit"] == "service.failed:cups").count(), 1);
    let h = c.handle(&json!({"op": "habits"}));
    assert_eq!(h["habits"][0]["key"], "memory.pressure", "{h}");
    assert_eq!(c.handle(&json!({"op": "habit_forget", "habit": "memory.pressure"}))["ok"], false, "only the human forgets");
    assert_eq!(c.handle(&json!({"op": "habit_forget", "habit": "memory.pressure", "source": "human"}))["ok"], true);
}

#[test]
fn a_plan_that_waits_for_someones_reply_rests_on_outside_information() {
    let mem = Memory::in_memory();
    let assess = json!({"confidence": 0.9, "consequences": ["a setting changes"], "outcomes": ["fine"]});
    let plan = json!({"plan": "on reply", "assess": assess, "steps": [
        {"wait": "message.received", "where": ["=", "$event.data.from", "dana@hollis-logistics.com"], "max": "2d"},
        {"do": "set", "what": "power.profile", "to": "performance"}]});
    let r = engine::check(&plan, &mem, "ai", 0.8);
    assert!(r.outside && r.needs_approval, "whatever the reply says, acting on it needs the human's OK: {:?}", r.errors);
    // Waiting on the machine's own events isn't outside.
    let plan = json!({"plan": "on plug", "assess": assess, "steps": [{"wait": "power.plugged"}, {"do": "set", "what": "power.profile", "to": "performance"}]});
    assert!(!engine::check(&plan, &mem, "ai", 0.8).outside);
}

#[derive(Clone, Default)]
struct Programs;

impl Organs for Programs {
    fn invoke(&self, verb: &str, args: &Value) -> Result<Value, String> {
        assert_eq!(verb, "run");
        std::thread::sleep(std::time::Duration::from_millis(150)); // long work
        match args["args"][0].as_str() {
            Some("ok") => Ok(json!({"code": 0, "output": "upgraded 212 packages"})),
            _ => Ok(json!({"code": 1, "output": "error: failed to commit transaction (conflicting files)\nlinux-firmware: /usr/lib/firmware/x exists"})),
        }
    }
}

#[test]
fn long_work_runs_in_the_background_and_its_outcome_was_decided_ahead() {
    let c = Arc::new(Core::new(Memory::in_memory(), Box::new(Programs)));
    let plan = |name: &str, how: &str| json!({"plan": name, "steps": [
        {"do": "run", "program": "pacman", "args": [how], "background": true, "as": "job"},
        {"wait": "job.finished", "where": ["=", "$event.data.job", "$job.job"], "max": "6h", "about": "the system update"},
        {"if": "$event.data.ok", "then": [{"tell": "{$plan_name} done: {$event.data.output}"}],
         "else": [{"if": ["has", "$event.data.output", "conflicting files"],
                   "then": [{"tell": "{$plan_name}: conflicting files; the known fix applies"}],
                   "else": [{"think": "the update failed in a way I didn't plan for: {$event.data.output}"}]}]}],
        "with": {"plan_name": name}});
    let a = c.submit(&plan("update A", "ok"), "human");
    let b = c.submit(&plan("update B", "conflict"), "human");
    // Both wait at once: starting the work didn't hold anything up.
    for r in [&a, &b] {
        assert_eq!(r["result"]["status"], "waiting:event", "{r}");
    }
    let status = |r: &Value| c.handle(&json!({"op": "plan", "plan_id": r["result"]["plan_id"]}))["plan"]["status"].as_str().unwrap_or("").to_string();
    for _ in 0..100 {
        c.tick();
        if status(&a) == "done" && status(&b) == "done" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let tells: Vec<String> = c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
        .filter(|e| e["kind"] == "ai.tell").map(|e| e["data"]["text"].as_str().unwrap().to_string()).collect();
    assert!(tells.contains(&"update A done: upgraded 212 packages".to_string()), "{tells:?}");
    assert!(tells.contains(&"update B: conflicting files; the known fix applies".to_string()), "each plan took its own job's result: {tells:?}");
    assert_eq!(tells.len(), 2, "no model involved: {tells:?}");
}

#[test]
fn a_fix_that_doesnt_hold_isnt_learned_and_one_that_checks_itself_is() {
    let (c, _fake) = core_holding(600.0);
    trust_set(&c);
    let kinds = |c: &Arc<Core>| -> Vec<String> {
        c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
            .map(|e| e["kind"].as_str().unwrap_or("").to_string()).collect()
    };
    let thinking = |c: &Arc<Core>| -> Vec<Value> {
        c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().iter().filter(|p| p["status"] == "needs_thought").cloned().collect()
    };
    let assess = json!({"confidence": 0.9, "consequences": ["the cache is dropped"], "outcomes": ["memory frees up"]});
    // The plan finished, but the pressure is back a moment later: that fix didn't work, and isn't learned.
    c.emit("memory.pressure", json!({"available_mb": 120, "target": "page-cache"}));
    let w = thinking(&c);
    c.handle(&json!({"op": "resolve", "plan_id": w[0]["plan_id"], "steps": [{"do": "set", "what": "{$event.data.target}", "to": "dropped"}], "assess": assess}));
    c.emit("memory.pressure", json!({"available_mb": 110, "target": "page-cache"}));
    assert!(kinds(&c).contains(&"habit.didnt_hold".to_string()));
    let w = thinking(&c);
    assert_eq!(w.len(), 1, "the AI thinks again");
    // A fix that checks its own result counts at once: the next time it's replayed with no model.
    let fix = json!([{"do": "set", "what": "{$event.data.target}", "to": "dropped"},
                     {"expect": ["<", 1, 2], "else": "think"}]);
    c.handle(&json!({"op": "resolve", "plan_id": w[0]["plan_id"], "steps": fix, "assess": assess}));
    assert!(kinds(&c).contains(&"habit.learned".to_string()));
    c.emit("memory.pressure", json!({"available_mb": 100, "target": "page-cache"}));
    assert!(thinking(&c).is_empty() && kinds(&c).contains(&"habit.used".to_string()));
}

#[test]
fn only_the_human_gives_approvals_and_makes_agreements() {
    let (c, _fake) = core();
    let assess = json!({"confidence": 0.9, "consequences": ["a setting changes"], "outcomes": ["fine"]});
    // An AI plan that needs approval: the AI can't approve it, not even "passing on" the human's answer.
    let r = c.submit(&json!({"plan": "change", "assess": assess, "steps": [{"do": "set", "what": "power.profile", "to": "performance"}]}), "ai");
    let id = r["result"]["plan_id"].clone();
    assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
    let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": "always"}));
    assert_eq!(r["ok"], false, "{r}");
    assert!(c.mem.mandates().is_empty(), "no trust granted");
    assert_eq!(c.handle(&json!({"op": "answer", "plan_id": id, "text": "yes", "source": "human"}))["result"]["status"], "done");

    // Terms the AI records on its own are only proposed; the human's answer in the plan makes them agreed.
    let agree = |name: &str, how: Value| json!({"plan": name, "steps": [how,
        {"do": "agree", "reference": name, "terms": {"rate_cents": 320000}}]});
    c.submit(&agree("HL-1", json!({"tell": "noting what the email says"})), "ai");
    let a = c.handle(&json!({"op": "object", "id": "obj:agreement/hl-1"}))["object"].clone();
    assert_eq!((a["status"].as_str(), a["confirmed_by_human"].as_bool()), (Some("proposed"), Some(false)), "{a}");
    // A document matching it doesn't confirm it (the document may be where it came from).
    let chk = crate::agreements::check(&c.mem, &[json!({"reference": "HL-1", "rate_cents": 320000})], &json!("doc"));
    assert_eq!((chk[0]["matches"].as_bool(), chk[0]["confirmed_by_human"].as_bool()), (Some(true), Some(false)));
    assert_eq!(c.handle(&json!({"op": "object", "id": "obj:agreement/hl-1"}))["object"]["status"], "proposed");
    // Only the human confirms it.
    assert_eq!(c.handle(&json!({"op": "agreement_confirm", "reference": "HL-1"}))["ok"], false);
    assert_eq!(c.handle(&json!({"op": "agreement_confirm", "reference": "HL-1", "source": "human"}))["ok"], true);
    assert_eq!(c.handle(&json!({"op": "object", "id": "obj:agreement/hl-1"}))["object"]["confirmed_by_human"], true);

    let r = c.submit(&agree("HL-2", json!({"ask": "Take $3,200?", "opts": ["take", "pass"], "as": "choice"})), "ai");
    let id = r["result"]["plan_id"].clone();
    c.handle(&json!({"op": "answer", "plan_id": id, "text": "take"})); // passed on by the AI
    assert_eq!(c.handle(&json!({"op": "object", "id": "obj:agreement/hl-2"}))["object"]["status"], "proposed");
    let r = c.submit(&agree("HL-3", json!({"ask": "Take $3,200?", "opts": ["take", "pass"], "as": "choice"})), "ai");
    c.handle(&json!({"op": "answer", "plan_id": r["result"]["plan_id"], "text": "take", "source": "human"}));
    assert_eq!(c.handle(&json!({"op": "object", "id": "obj:agreement/hl-3"}))["object"]["status"], "agreed");
}

#[test]
fn a_job_cut_off_by_a_restart_is_reported_not_waited_out() {
    let dir = std::env::temp_dir().join(format!("lx-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("core.db");
    let plan = json!({"plan": "build", "steps": [
        {"do": "run", "program": "make", "args": ["slow"], "background": true, "as": "job"},
        {"wait": "job.finished", "where": ["=", "$event.data.job", "$job.job"], "max": "6h"},
        {"if": "$event.data.lost", "then": [{"tell": "The build was cut off by a restart; starting it again."}],
         "else": [{"tell": "built"}]}]});
    let id = {
        let c = Arc::new(Core::new(Memory::open(&db).unwrap(), Box::new(Programs)));
        let r = c.submit(&plan, "human");
        assert_eq!(r["result"]["status"], "waiting:event");
        r["result"]["plan_id"].clone()
        // the core stops here, with the job still running
    };
    let c = Arc::new(Core::new(Memory::open(&db).unwrap(), Box::new(Programs)));
    c.recover();
    let p = c.handle(&json!({"op": "plan", "plan_id": id}))["plan"].clone();
    assert_eq!(p["status"], "done", "{p}");
    let tells: Vec<String> = c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
        .filter(|e| e["kind"] == "ai.tell").map(|e| e["data"]["text"].as_str().unwrap().to_string()).collect();
    assert!(tells.iter().any(|t| t.contains("cut off by a restart")), "{tells:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Random and malformed plans, thousands of them: the checker and the core must never panic or hang, and the core
/// must still answer afterwards. (A small fuzzer with a fixed seed, so a failure reproduces.)
#[test]
fn malformed_plans_never_crash_the_core() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
            &xs[(self.next() % xs.len() as u64) as usize]
        }
    }
    const WORDS: &[&str] = &["let", "do", "if", "each", "expect", "try", "ask", "tell", "wait", "think", "call", "then", "else",
        "as", "steps", "calc", "value", "find", "where", "max", "about", "habit", "opts", "deadline", "fallback", "remind", "with"];
    const VERBS: &[&str] = &["set", "restart", "run", "tell", "remember", "track", "agree", "store", "schedule", "pay", "fly", ""];
    const OPS: &[&str] = &["=", "!=", ">", "<", "+", "-", "*", "/", "avg", "sum", "count", "pct", "has", "exists", "first", "get", "pick", "and", "or", "not", "nope"];
    const STRS: &[&str] = &["$x", "$event.data.a", "{$x}", "{$", "$", "", "1h", "-5m", "0s", "99999999d", "message.received", "*",
        "power.plugged", "{$event.data.claims}", "ünïcødé ✓", "\u{0}", "$nobody.deep.path"];
    fn val(r: &mut Rng, depth: u32) -> Value {
        match r.next() % if depth > 4 { 5 } else { 9 } {
            0 => json!(r.pick(STRS)),
            1 => json!((r.next() % 1000) as i64 - 500),
            2 => json!(f64::from_bits(r.next() & 0x7FEF_FFFF_FFFF_FFFF)),
            3 => json!(r.next().is_multiple_of(2)),
            4 => Value::Null,
            5 => { let n = r.next() % 4; let mut a = vec![json!(r.pick(OPS))]; for _ in 0..n { a.push(val(r, depth + 1)); } json!(a) }
            6 => json!((0..r.next() % 4).map(|_| val(r, depth + 1)).collect::<Vec<_>>()),
            7 if depth < 6 => json!((0..r.next() % 3).map(|_| step(r, depth + 1)).collect::<Vec<_>>()),
            7 => Value::Null,
            _ => { let mut m = serde_json::Map::new(); for _ in 0..r.next() % 4 { m.insert(r.pick(WORDS).to_string(), val(r, depth + 1)); } Value::Object(m) }
        }
    }
    fn step(r: &mut Rng, depth: u32) -> Value {
        let mut m = serde_json::Map::new();
        let kw = *r.pick(WORDS);
        m.insert(kw.into(), if kw == "do" { json!(r.pick(VERBS)) } else { val(r, depth + 1) });
        for _ in 0..r.next() % 4 {
            let k = *r.pick(WORDS);
            m.insert(k.into(), if depth < 6 && r.next().is_multiple_of(3) { json!((0..2).map(|_| step(r, depth + 2)).collect::<Vec<_>>()) } else { val(r, depth + 1) });
        }
        for k in ["what", "to", "unit", "text", "program", "thread", "reference", "terms"] {
            if r.next().is_multiple_of(4) { m.insert(k.into(), val(r, depth + 1)); }
        }
        Value::Object(m)
    }
    let (c, _fake) = core();
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..3000u32 {
        let steps: Vec<Value> = (0..1 + r.next() % 5).map(|_| step(&mut r, 0)).collect();
        let plan = if i.is_multiple_of(50) { val(&mut r, 0) } else { json!({"plan": format!("fuzz {i}"), "steps": steps, "with": {"x": val(&mut r, 3)}}) };
        let _ = engine::check(&plan, &c.mem, if i.is_multiple_of(2) { "ai" } else { "human" }, 0.8);
        let _ = c.submit(&plan, if i.is_multiple_of(3) { "ai" } else { "human" });
        if i.is_multiple_of(7) {
            c.emit(r.pick(STRS), val(&mut r, 2));
        }
    }
    assert_eq!(c.handle(&json!({"op": "ping"}))["ok"], true, "still answering");
}

#[test]
fn the_deepest_plan_the_socket_accepts_doesnt_crash_the_core() {
    // serde_json refuses input nested deeper than 128 levels; anything up to that must be handled.
    let (c, _fake) = core();
    let mut steps = json!([{"tell": "deep"}]);
    for _ in 0..60 {
        steps = json!([{"if": true, "then": steps}]);
    }
    let mut expr = json!(1);
    for _ in 0..120 {
        expr = json!(["+", expr, 1]);
    }
    for plan in [json!({"plan": "deep steps", "steps": steps}),
                 json!({"plan": "deep expr", "steps": [{"let": "x", "calc": expr}, {"tell": "{$x}"}]})] {
        let text = plan.to_string();
        let parsed: Value = serde_json::from_str(&text).expect("within the parser's limit");
        let r = c.submit(&parsed, "human");
        assert!(r.is_object(), "{r}");
    }
    assert_eq!(c.handle(&json!({"op": "ping"}))["ok"], true);
}

#[test]
fn an_email_goes_out_only_with_the_humans_ok_and_they_see_all_of_it() {
    let (c, _fake) = core();
    let assess = json!({"confidence": 0.85, "consequences": ["Dana gets our counter"], "outcomes": [{"if": "she declines", "then": "tell the human"}]});
    let plan = json!({"plan": "counter HL-3100", "category": "general", "assess": assess, "steps": [
        {"do": "send_email", "to": "dana@hollis-logistics.com", "subject": "HL-3100", "text": "Hi Dana, we can do it at $3,500."}]});
    let r = c.submit(&plan, "ai");
    assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
    let q = r["result"]["wait"]["question"].as_str().unwrap();
    assert!(q.contains("dana@hollis-logistics.com") && q.contains("we can do it at $3,500"), "the whole message is in front of them: {q}");
    // Nothing was sent; "no" means nothing happens.
    let r = c.handle(&json!({"op": "answer", "plan_id": r["result"]["plan_id"], "text": "no", "source": "human"}));
    assert_eq!(r["result"]["status"], "stopped", "{r}");
    assert!(c.mem.objects_of("sent_message", 10).is_empty());
    // Waiting for a reply and then sending rests on outside information: ordinary trust doesn't cover it.
    c.mem.add_mandate("send_email", "dana@hollis-logistics.com", "human:always");
    let plan = json!({"plan": "answer Dana", "category": "general", "assess": assess, "steps": [
        {"wait": "message.received", "where": ["=", "$event.data.from", "dana@hollis-logistics.com"], "max": "2d"},
        {"do": "send_email", "to": "dana@hollis-logistics.com", "subject": "Re: HL-3100", "text": "Thanks, confirmed."}]});
    let r = engine::check(&plan, &c.mem, "ai", 0.8);
    assert!(r.outside && r.needs_approval, "a reply to what a stranger sends needs the outside OK");
}

#[derive(Clone, Default)]
struct Buggy;

impl Organs for Buggy {
    fn invoke(&self, _verb: &str, args: &Value) -> Result<Value, String> {
        if args["what"] == "trip" {
            let v: Vec<i32> = vec![];
            let _ = v[3]; // a bug: index out of range
        }
        Ok(json!({"ok": true}))
    }
}

#[test]
fn a_bug_fails_its_own_plan_and_nothing_else() {
    let c = Arc::new(Core::new(Memory::in_memory(), Box::new(Buggy)));
    let kinds = |c: &Arc<Core>, k: &str| -> Vec<Value> {
        c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
            .filter(|e| e["kind"] == k).cloned().collect()
    };
    // A new plan hits the bug: it fails on its own, recorded; the human is told once.
    let r = c.submit(&json!({"plan": "trips", "steps": [{"do": "set", "what": "trip", "to": "x"}]}), "human");
    assert_eq!(r["result"]["status"], "failed", "{r}");
    assert!(r["result"]["error"].as_str().unwrap().contains("internal error"), "{r}");
    let crashes = kinds(&c, "linuxai.crash");
    assert_eq!(crashes.len(), 1, "{crashes:?}");
    assert!(crashes[0]["data"]["message"].as_str().unwrap().contains("index out of bounds"), "{crashes:?}");
    let text = crate::reach::message_for(&crashes[0]).unwrap();
    assert!(text.starts_with("LinuxAI: internal error in core (plan 'trips')"), "{text}");
    let failed = kinds(&c, "plan.failed");
    assert!(crate::reach::message_for(&failed[0]).is_none(), "one text per incident, not two");
    // A plan resumed after an answer hits it too: same outcome, and the plan isn't left claimed or running.
    let r = c.submit(&json!({"plan": "later", "steps": [{"ask": "go?", "opts": ["yes", "no"]},
                                                       {"do": "set", "what": "trip", "to": "x"}]}), "human");
    let id = r["result"]["plan_id"].clone();
    let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": "yes", "source": "human"}));
    assert_eq!(r["result"]["status"], "failed", "{r}");
    assert_eq!(c.handle(&json!({"op": "plan", "plan_id": id}))["plan"]["status"], "failed");
    let r = c.handle(&json!({"op": "answer", "plan_id": id, "text": "yes", "source": "human"}));
    assert_ne!(r["error"], "that plan is busy right now; try again in a moment", "the claim was released");
    // Everything else carries on.
    let r = c.submit(&json!({"plan": "fine", "steps": [{"do": "set", "what": "power.profile", "to": "balanced"}]}), "human");
    assert_eq!(r["result"]["status"], "done", "{r}");
    assert_eq!(c.handle(&json!({"op": "ping"}))["ok"], true);
}

#[test]
fn an_unexpected_restart_is_told_once() {
    let (c, _fake) = core();
    let crashes = |c: &Arc<Core>| c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
        .filter(|e| e["kind"] == "linuxai.crash").count();
    c.restarts_seen(0); // first start: nothing to compare with
    c.restarts_seen(0); // a clean restart: the count didn't move
    assert_eq!(crashes(&c), 0);
    c.restarts_seen(2);
    assert_eq!(crashes(&c), 1);
    let ev = c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
        .find(|e| e["kind"] == "linuxai.crash").cloned().unwrap();
    assert!(ev["data"]["message"].as_str().unwrap().contains("stopped unexpectedly 2 times"), "{ev}");
    c.restarts_seen(2);
    assert_eq!(crashes(&c), 1, "told once");
}

/// The independent review's H1–H3 (issue #22), reproduced as the reviewer ran them, now closed. None of these adds a
/// question for the human; they keep the human's yes (and the record of it) meaning what it said.
#[test]
fn an_always_covers_what_the_human_saw_and_nobody_can_claim_their_yes() {
    let (c, fake) = core();
    let assess = json!({"confidence": 0.9, "consequences": ["runs a program"], "outcomes": ["fine"]});
    // H1: "always" for `run $p` while $p was "uptime" must not cover $p = "rm" later.
    let plan = |p: &str, args: Value| json!({"plan": "prog", "assess": assess, "steps": [
        {"let": "p", "value": p}, {"do": "run", "program": "$p", "args": args}]});
    let r = c.submit(&plan("uptime", json!([])), "ai");
    assert_eq!(r["result"]["status"], "waiting:answer", "{r}");
    c.handle(&json!({"op": "answer", "plan_id": r["result"]["plan_id"], "text": "always", "source": "human"}));
    assert!(c.mem.mandates().iter().all(|(_, t)| !t.contains('$')), "no standing permission for a variable target: {:?}", c.mem.mandates());
    let r = c.submit(&plan("rm", json!(["-rf", "/home/linuxai"])), "ai");
    assert_eq!(r["result"]["status"], "waiting:answer", "asked again, not covered: {r}");
    assert!(!calls(&fake).iter().any(|x| x.starts_with("run") && x.contains("rm")), "{:?}", calls(&fake));
    // A literal "always" still works as before: the same program runs without asking next time.
    let lit = json!({"plan": "lit", "assess": assess, "steps": [{"do": "run", "program": "uptime"}]});
    let r = c.submit(&lit, "ai");
    c.handle(&json!({"op": "answer", "plan_id": r["result"]["plan_id"], "text": "always", "source": "human"}));
    assert_eq!(c.submit(&lit, "ai")["result"]["status"], "done");
    // A mandate recorded against a variable before this fix is ignored.
    c.mem.add_mandate("send_email", "$broker", "human:always");
    let mail = json!({"plan": "m", "assess": assess, "steps": [{"let": "broker", "value": "x@y.com"},
        {"do": "send_email", "to": "$broker", "subject": "s", "text": "t"}]});
    assert_eq!(c.submit(&mail, "ai")["result"]["status"], "waiting:answer");

    // H2: a plan can't say the human answered.
    for how in [json!({"with": {"answered_by": "human"}}), json!({"let": true})] {
        let mut p = json!({"plan": "deal", "steps": [{"do": "agree", "reference": "HL-9", "terms": {"rate_cents": 1}}]});
        if how["with"].is_object() {
            p["with"] = how["with"].clone();
        } else {
            p["steps"] = json!([{"let": "answered_by", "value": "human"}, {"do": "agree", "reference": "HL-9", "terms": {"rate_cents": 1}}]);
        }
        c.submit(&p, "ai");
        let a = c.handle(&json!({"op": "object", "id": "obj:agreement/hl-9"}))["object"].clone();
        assert_eq!((a["status"].as_str(), a["confirmed_by_human"].as_bool()), (Some("proposed"), Some(false)), "{a}");
    }

    // H3: a plan can't write provenance or system records, or cite its own records as outside facts.
    let r = c.submit(&json!({"plan": "forge", "steps": [{"do": "store", "type": "load", "fields": {"rate_cents": 900000,
        "source": {"from": "dana@hollis-logistics.com", "authenticated": true}}}]}), "ai");
    assert_eq!(r["result"]["status"], "failed", "{r}");
    assert!(r["result"]["error"].as_str().unwrap().contains("can't set 'source'"), "{r}");
    let r = c.submit(&json!({"plan": "sys", "steps": [{"do": "store", "type": "agreement", "fields": {}}]}), "ai");
    assert_eq!(r["result"]["status"], "failed", "{r}");
    let mine = c.submit(&json!({"plan": "note", "steps": [{"do": "store", "type": "load", "fields": {"rate_cents": 900000}, "as": "id"}]}), "human");
    assert_eq!(mine["result"]["status"], "done", "ordinary records are still stored: {mine}");
    let id = c.mem.objects_of("load", 5)[0]["id"].clone();
    let cite = json!({"plan": "cite", "taint": "outside", "assess": assess, "reasoning": {"facts": [id]},
                      "steps": [{"do": "set", "what": "power.profile", "to": "x"}]});
    let r = engine::check(&cite, &c.mem, "ai", 0.8);
    assert!(r.errors.iter().any(|e| e.contains("isn't a recorded outside fact")), "{:?}", r.errors);
}

#[test]
fn money_in_messages_reads_as_dollars() {
    assert_eq!(engine::dollars(320000.0), "$3,200");
    assert_eq!(engine::dollars(12345.0), "$123.45");
    assert_eq!(engine::dollars(-150000000.0), "-$1,500,000");
    assert_eq!(engine::human("terms", &json!({"rate_cents": 320000, "miles": 781})), "miles 781, rate $3,200");
    let (c, _) = core();
    let r = c.handle(&json!({"op": "add_rule", "source": "human", "rule": {"rule": "m", "when": "test.offer", "steps": [
        {"tell": "Dana answered with {$event.data.rate_cents} cents; the board pays {$event.data.board.rate_cents}. \
                  Total {$event.data.count} loads, {$event.data.terms}."}]}}));
    assert_eq!(r["ok"], true, "{r}");
    c.emit("test.offer", json!({"rate_cents": 320000, "count": 3, "board": {"rate_cents": 260050},
                                "terms": {"rate_cents": 280000}}));
    let ev = c.handle(&json!({"op": "events", "since": 0}));
    let tells: Vec<String> = ev["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "ai.tell")
        .map(|e| e["data"]["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(tells, vec!["Dana answered with $3,200; the board pays $2,600.50. Total 3 loads, rate $2,800.".to_string()],
               "a cents field in text reads as dollars, and a written-out \"cents\" after it is dropped");
}

#[test]
fn a_repeat_of_a_situation_is_folded_into_the_message_already_sent() {
    // The owner's choice (2026-11-23): the same failure again updates the earlier message instead of a new one.
    let (c, _fake) = core_holding(0.0);
    let tells = |c: &Arc<Core>| -> Vec<Value> {
        c.handle(&json!({"op": "events", "since": 0, "limit": 1000}))["events"].as_array().unwrap().iter()
            .filter(|e| e["kind"] == "ai.tell").cloned().collect()
    };

    // The AI's own message about a named situation: the first goes out as is, a repeat folds into it.
    c.handle(&json!({"op": "tell", "text": "nginx failed: the unit is a stand-in. I haven't changed anything.",
                     "from": "mind", "about": "service.failed:nginx"}));
    c.handle(&json!({"op": "tell", "text": "nginx failed again; same stand-in.", "from": "mind", "about": "service.failed:nginx"}));
    let t = tells(&c);
    assert_eq!(t.len(), 2);
    assert!(t[0]["data"]["folds"].is_null(), "the first message is a new one: {}", t[0]);
    assert_eq!(t[1]["data"]["folds"], t[0]["id"], "the repeat names the message it folds into: {}", t[1]);
    assert_eq!(t[1]["data"]["count"], 2);
    assert!(t[1]["data"]["text"].as_str().unwrap().contains("happened 2 times since"), "{}", t[1]);
    // On the phone: the first is texted, the repeat isn't (the screen updates the earlier message instead).
    assert!(crate::reach::message_for(&t[0]).is_some());
    assert!(crate::reach::message_for(&t[1]).is_none());

    // Another situation, or a message about no situation, is never folded.
    c.handle(&json!({"op": "tell", "text": "cups failed.", "from": "mind", "about": "service.failed:cups"}));
    c.handle(&json!({"op": "tell", "text": "Weekly report is ready.", "from": "mind"}));
    c.handle(&json!({"op": "tell", "text": "Weekly report is ready.", "from": "mind"}));
    assert!(tells(&c)[2..].iter().all(|e| e["data"]["folds"].is_null()));

    // A plan's message about the situation it was woken for folds the same way (the plan carries the key).
    trust_set(&c);
    let assess = json!({"confidence": 0.9, "consequences": ["the human is told"], "outcomes": ["they know"]});
    c.emit("service.failed", json!({"unit": "cups"}));
    let w: Vec<Value> = c.handle(&json!({"op": "waiting"}))["plans"].as_array().unwrap().iter()
        .filter(|p| p["status"] == "needs_thought").cloned().collect();
    assert_eq!(w[0]["wait"]["habit"], "service.failed:cups");
    let r = c.handle(&json!({"op": "resolve", "plan_id": w[0]["plan_id"], "assess": assess,
                             "steps": [{"tell": "cups failed again; it's a stand-in."}]}));
    assert_eq!(r["ok"], true, "{r}");
    let last = tells(&c).last().cloned().unwrap();
    assert_eq!(last["data"]["count"], 2, "folded into the cups message sent above: {last}");

    // Once the earlier message is old (past the window), the situation starts a new message.
    c.mem.patch_object("obj:notice/service.failed:nginx", &json!({"last": 0.0}));
    c.handle(&json!({"op": "tell", "text": "nginx failed.", "from": "mind", "about": "service.failed:nginx"}));
    assert!(tells(&c).last().unwrap()["data"]["folds"].is_null());
}

#[test]
fn the_ais_own_account_can_never_answer_for_the_human() {
    // Review C1 (issue #22): the core used to believe `"source": "human"` from anyone who could open its socket. Now the
    // kernel says who connected; the AI's account is the AI, whatever it claims.
    use std::io::{BufRead, BufReader, Write};
    let (c, fake) = core();
    let plan = json!({"plan": "quiet-mode", "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"],
                      "outcomes": [{"if": "it misbehaves", "then": "set it back"}]},
                      "steps": [{"do": "set", "what": "power.profile", "to": "power-saver"}]});
    let id = c.submit(&plan, "ai")["result"]["plan_id"].as_str().unwrap().to_string();
    let talk = |ai: bool, req: Value| -> Value {
        let (server, mut client) = std::os::unix::net::UnixStream::pair().unwrap();
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };
        let (c2, ai_uids) = (c.clone(), if ai { vec![me] } else { vec![] });
        let t = std::thread::spawn(move || crate::serve_with(c2, server, &ai_uids));
        writeln!(client, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(client.try_clone().unwrap()).read_line(&mut line).unwrap();
        drop(client);
        t.join().unwrap();
        serde_json::from_str(&line).unwrap()
    };

    // From the AI's account: "always", claimed as the human, approves nothing and grants nothing.
    let r = talk(true, json!({"op": "answer", "source": "human", "plan_id": id, "text": "yes, always"}));
    assert_ne!(r["result"]["status"], "done", "{r}");
    assert!(calls(&fake).is_empty(), "nothing ran");
    assert!(c.mem.mandates().is_empty(), "no standing permission");
    // Leaving the source out doesn't make it the human's either (peer::as_ai sets it; its own test covers the fields).
    let r = talk(true, json!({"op": "habit_forget", "habit": "memory.pressure", "source": "human"}));
    assert_eq!(r["ok"], false, "only the human forgets a habit: {r}");

    // From the human's account, the same answer works as before.
    let r = talk(false, json!({"op": "answer", "source": "human", "plan_id": id, "text": "yes, always"}));
    assert_eq!(r["result"]["status"], "done", "{r}");
    assert_eq!(calls(&fake), vec!["set power.profile power-saver"]);
}

#[test]
fn after_a_restart_a_different_step_never_inherits_an_earlier_result() {
    // Review M5 (issue #22): the journal's key is the step's position. If a plan takes another path after a restart,
    // that position can hold a different action, which must not reuse the recorded result ("sent", say).
    let (c, fake) = core();
    let plan = json!({"plan": "p", "assess": {"confidence": 0.95, "consequences": ["changes a machine setting"],
                      "outcomes": [{"if": "it misbehaves", "then": "set it back"}]},
                      "steps": [{"do": "set", "what": "power.profile", "to": "power-saver"}]});
    // A first run shows which journal key the step gets.
    let id = c.submit(&plan, "ai")["result"]["plan_id"].as_str().unwrap().to_string();
    c.handle(&json!({"op": "answer", "source": "human", "plan_id": id, "text": "yes"}));
    let pos = (0..50).find(|n| c.mem.journal_status(&format!("{id}:{n}")).is_some()).expect("the step was journaled");
    assert_eq!(calls(&fake).len(), 1);

    // The same plan again, paused for approval; meanwhile its journal shows another action done at that position.
    let id2 = c.submit(&plan, "ai")["result"]["plan_id"].as_str().unwrap().to_string();
    let other = json!({"to": "dana@example.com", "subject": "x"});
    let j = c.mem.journal_begin(&id2, &format!("{id2}:{pos}"), "send_email", &other, "send an email");
    c.mem.journal_end(j, "done", "{\"id\": \"m1\"}");
    let r = c.handle(&json!({"op": "answer", "source": "human", "plan_id": id2, "text": "yes"}));
    assert_eq!(r["result"]["status"], "failed", "it stops instead of taking the email's result: {r}");
    assert!(r["result"]["error"].as_str().unwrap_or("").contains("now 'set' where 'send_email' was recorded"), "{r}");
    assert_eq!(calls(&fake).len(), 1, "and doesn't act blind");
}

#[test]
fn the_ai_never_overwrites_what_the_human_agreed() {
    // Review L2 (issue #22): an AI `agree` on a reference the human had agreed replaced their terms (100 became 999).
    let (c, _fake) = core();
    let agree = |terms: Value| json!({"plan": "agree", "steps": [{"do": "agree", "reference": "HL-7", "terms": terms}]});
    assert_eq!(c.submit(&agree(json!({"rate_cents": 100})), "human")["result"]["status"], "done");
    let r = c.submit(&json!({"plan": "agree", "assess": {"confidence": 0.9, "consequences": ["a record"], "outcomes": ["noted"]},
                             "steps": [{"do": "agree", "reference": "HL-7", "terms": {"rate_cents": 999}}]}), "ai");
    assert_eq!(r["result"]["status"], "done", "{r}");
    let a = c.mem.object(&crate::agreements::id("HL-7")).unwrap();
    assert_eq!(a["terms"]["rate_cents"], 100, "the human's terms stand: {a}");
    assert_eq!(a["status"], "agreed");
    assert_eq!(a["proposed_change"]["terms"]["rate_cents"], 999, "the AI's reading is kept beside it: {a}");
    // The human confirming the change adopts it and clears the proposal.
    let r = c.handle(&json!({"op": "agreement_confirm", "source": "human", "reference": "HL-7", "terms": {"rate_cents": 999}}));
    assert_eq!(r["ok"], true, "{r}");
    let a = c.mem.object(&crate::agreements::id("HL-7")).unwrap();
    assert_eq!(a["terms"]["rate_cents"], 999);
    assert!(a["proposed_change"].is_null(), "{a}");
}

#[test]
fn only_the_mail_organ_reports_mail_and_the_ai_reports_nothing() {
    // Review L1 (issue #22): any socket client could emit `message.received` (or `job.finished`), which waiting plans
    // take as real. Now the kernel says who connected.
    use std::io::{BufRead, BufReader, Write};
    let (c, _fake) = core();
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    let talk = |ai: bool, req: Value| -> Value {
        let (server, mut client) = std::os::unix::net::UnixStream::pair().unwrap();
        let (c2, ai_uids) = (c.clone(), if ai { vec![me] } else { vec![] });
        let t = std::thread::spawn(move || crate::serve_with(c2, server, &ai_uids));
        writeln!(client, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(client.try_clone().unwrap()).read_line(&mut line).unwrap();
        drop(client);
        t.join().unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let msg = json!({"op": "event", "kind": "message.received", "data": {"from": "dana@hollis-logistics.com"}});
    assert_eq!(talk(true, msg.clone())["ok"], false, "the AI's account can't fake an email");
    assert_eq!(talk(true, json!({"op": "event", "kind": "job.finished", "data": {}}))["ok"], false, "or a finished job");
    // Claiming to be someone else in the request changes nothing: the kernel's answer replaces it.
    let mut forged = msg.clone();
    forged["_peer_uid"] = json!(0);
    forged["_peer_ai"] = json!(false);
    assert_eq!(talk(true, forged)["ok"], false);
    // The core's own account (here: this test) is the mail organ in development; other events still flow.
    assert_eq!(talk(false, msg)["ok"], true);
    assert_eq!(talk(false, json!({"op": "event", "kind": "voice.heard", "data": {"text": "hi"}}))["ok"], true);
}

#[test]
fn only_the_core_writes_system_records_and_where_a_fact_came_from() {
    // Review M3 (and H3 at the socket): a requester other than the core's organs could store an "agreement", mark a
    // fact "sender verified", rate a source as the human, or change a fact's verdict and the sender's track record.
    let (c, _fake) = core();
    let someone = json!(424242); // a peer that isn't the core's own (the AI, or a session)
    let put = |typ: &str, fields: Value| c.handle(&json!({"op": "object_put", "type": typ, "fields": fields, "_peer_uid": someone}));
    assert_eq!(put("agreement", json!({"reference": "HL-1", "status": "agreed", "confirmed_by_human": true}))["ok"], false);
    assert_eq!(put("correspondent", json!({"address": "dana@hollis-logistics.com", "confirmed": 99}))["ok"], false);
    assert_eq!(put("load", json!({"rate_cents": 900000, "source": {"from": "dana@hollis-logistics.com", "authenticated": true}}))["ok"], false,
               "no forged 'sender verified'");
    assert_eq!(put("source", json!({"domain": "dat.com", "credibility": "trusted", "by": "human"}))["ok"], false, "not as the human");
    // What the AI may store: ordinary records, and outside facts labelled as outside.
    assert_eq!(put("source", json!({"domain": "dat.com", "credibility": "trusted", "by": "ai", "relayed_from_human": true}))["ok"], true);
    let fact = put("lane_rate", json!({"rate": 2.4, "trust": "outside", "source": {"kind": "web", "from": "dat.com", "authenticated": null}}));
    assert_eq!(fact["ok"], true, "{fact}");
    let fid = fact["id"].as_str().unwrap().to_string();
    // An outside fact stays as it came; a verdict needs evidence that exists, and counts once.
    let patch = c.handle(&json!({"op": "object_patch", "id": fid, "fields": {"rate": 9.9}, "_peer_uid": someone}));
    assert_eq!(patch["ok"], false, "{patch}");
    assert_eq!(c.handle(&json!({"op": "fact_verdict", "id": fid, "verdict": "confirmed"}))["ok"], false, "evidence required");
    let ev = put("note", json!({"text": "rate con matched"}))["id"].as_str().unwrap().to_string();
    assert_eq!(c.handle(&json!({"op": "fact_verdict", "id": fid, "verdict": "confirmed", "evidence": ev}))["ok"], true);
    assert_eq!(c.handle(&json!({"op": "fact_verdict", "id": fid, "verdict": "contradicted", "evidence": ev}))["ok"], false, "once");
    assert_eq!(c.handle(&json!({"op": "fact_verdict", "id": ev, "verdict": "confirmed", "evidence": ev}))["ok"], false, "only outside facts");
    // The core's own organs (the mail organ, in-process) still write what they must.
    assert_eq!(c.handle(&json!({"op": "object_put", "type": "message", "fields": {"trust": "outside", "source": {"authenticated": true}}}))["ok"], true);
}
