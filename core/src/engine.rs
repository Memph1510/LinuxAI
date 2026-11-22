//! The action-language engine: evaluates expressions, checks plans, and runs them durably.
//!
//! A plan's execution state (variables + an explicit frame stack) is plain data, saved after every
//! step, so a plan can pause for days (ask / wait) and resume after a reboot exactly where it was.

use crate::memory::{new_id, now, Memory};
use crate::verbs::{self, Effect, Organs};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::Mutex;
use crate::crash::Locked;

// ---------------------------------------------------------------------------------------------
// Execution state
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum FrameKind {
    Block,
    Each { var: String, items: Vec<Value>, pos: usize },
    Try { otherwise: Vec<Value> },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Frame {
    pub steps: Vec<Value>,
    pub idx: usize,
    pub kind: FrameKind,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Exec {
    pub id: String,
    pub name: String,
    pub why: String,
    pub source: String,
    pub plan: Value,
    pub vars: Map<String, Value>,
    pub stack: Vec<Frame>,
    /// running | waiting:answer | waiting:event | waiting:time | needs_thought | done | stopped | failed
    pub status: String,
    pub wait: Value,
    pub counter: u64,
    pub approved: bool,
    pub error: Option<String>,
    pub created: f64,
    /// The human answered a question in this plan themselves (set only by the core, in `answer`). Plan variables,
    /// `$answered_by` included, can be set by whoever wrote the plan, so they never prove this.
    #[serde(default)]
    pub human_answered: bool,
}

impl Exec {
    pub fn summary(&self) -> Value {
        json!({"plan_id": self.id, "plan": self.name, "status": self.status, "wait": self.wait,
               "error": self.error, "source": self.source, "task": self.plan.get("task"),
               "proposal": self.plan.get("proposal")})
    }
}

/// Collects events emitted while a plan runs; the core broadcasts them and dispatches rules afterwards.
#[derive(Default)]
pub struct Bus {
    pub pending: Mutex<Vec<(String, Value)>>,
}

impl Bus {
    pub fn emit(&self, mem: &Memory, kind: &str, data: Value) {
        let id = mem.add_event(kind, &data);
        let ev = json!({"id": id, "ts": now(), "kind": kind, "data": data});
        self.pending.lk().push((kind.to_string(), ev));
    }
    pub fn drain(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.pending.lk())
    }
}

pub struct Ctx<'a> {
    pub mem: &'a Memory,
    pub organs: &'a dyn Organs,
    pub bus: &'a Bus,
    pub state: &'a Value,
}

// ---------------------------------------------------------------------------------------------
// Values and expressions
// ---------------------------------------------------------------------------------------------

const OPS: &[&str] = &[
    "=", "!=", ">", "<", ">=", "<=", "and", "or", "not", "+", "-", "*", "/", "avg", "sum", "count", "min",
    "max", "pct", "has", "exists", "first", "get", "pick",
];

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn to_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => {
            let t = s.trim().trim_start_matches(['$', '€', '£']).trim_end_matches('%').replace(',', "");
            t.parse().ok()
        }
        _ => None,
    }
}

fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        json!(f as i64)
    } else {
        json!((f * 10000.0).round() / 10000.0)
    }
}

pub fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            if f.fract() == 0.0 { format!("{}", f as i64) } else { format!("{:.2}", f) }
        }
        Value::Null => "nothing".into(),
        other => other.to_string(),
    }
}

/// Cents as dollars, the way a person writes money: 320000 -> "$3,200", 12345 -> "$123.45".
pub fn dollars(cents: f64) -> String {
    let neg = cents < 0.0;
    let c = cents.abs().round() as i64;
    let whole = (c / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let frac = if c % 100 == 0 { String::new() } else { format!(".{:02}", c % 100) };
    format!("{}${grouped}{frac}", if neg { "-" } else { "" })
}

/// A value as a person reads it in a message: a field named `..._cents` is money and reads as dollars; a record
/// reads as "rate $3,200, miles 781" rather than JSON. (Comparisons keep using `display`.)
pub fn human(key: &str, v: &Value) -> String {
    match v {
        Value::Number(n) if key.ends_with("_cents") => dollars(n.as_f64().unwrap_or(0.0)),
        Value::Object(m) => m.iter().map(|(k, x)| {
            let name = k.strip_suffix("_cents").unwrap_or(k).replace('_', " ");
            format!("{name} {}", human(k, x))
        }).collect::<Vec<_>>().join(", "),
        Value::Array(a) => a.iter().map(|x| human(key, x)).collect::<Vec<_>>().join(", "),
        other => display(other),
    }
}

fn lookup(path: &str, vars: &Map<String, Value>, state: &Value) -> Value {
    let mut parts = path.split('.');
    let root = parts.next().unwrap_or("");
    let mut cur = match root {
        "state" => state.clone(),
        "now" => json!(now()),
        r => vars.get(r).cloned().unwrap_or(Value::Null),
    };
    for p in parts {
        cur = match &cur {
            Value::Object(m) => m.get(p).cloned().unwrap_or(Value::Null),
            Value::Array(a) => p.parse::<usize>().ok().and_then(|i| a.get(i).cloned()).unwrap_or(Value::Null),
            _ => Value::Null,
        };
    }
    cur
}

fn is_var(s: &str) -> bool {
    s.len() > 1 && s.starts_with('$') && !s.contains(char::is_whitespace) && !s[1..].starts_with(|c: char| c.is_ascii_digit())
}

/// Replace `{$path}` occurrences in text with values.
fn interpolate(s: &str, vars: &Map<String, Value>, state: &Value) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("{$") {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end) => {
                let path = &rest[start + 2..start + end];
                let field = path.rsplit('.').next().unwrap_or(path);
                let value = lookup(path, vars, state);
                out.push_str(&human(field, &value));
                rest = &rest[start + end + 1..];
                // "{$x.rate_cents} cents" already reads as dollars: drop the unit written after it.
                if field.ends_with("_cents") && value.is_number() {
                    for unit in [" cents", " cent"] {
                        if rest.len() >= unit.len() && rest[..unit.len()].eq_ignore_ascii_case(unit)
                            && !rest[unit.len()..].starts_with(|c: char| c.is_alphanumeric()) {
                            rest = &rest[unit.len()..];
                            break;
                        }
                    }
                }
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn eval(v: &Value, vars: &Map<String, Value>, ctx: &Ctx) -> Result<Value, String> {
    match v {
        Value::String(s) if is_var(s) => Ok(lookup(&s[1..], vars, ctx.state)),
        Value::String(s) if s.contains("{$") => Ok(Value::String(interpolate(s, vars, ctx.state))),
        Value::Array(a) => {
            if let Some(Value::String(op)) = a.first() {
                if OPS.contains(&op.as_str()) {
                    let args = a[1..].iter().map(|x| eval(x, vars, ctx)).collect::<Result<Vec<_>, _>>()?;
                    return apply_op(op, &args);
                }
            }
            Ok(Value::Array(a.iter().map(|x| eval(x, vars, ctx)).collect::<Result<_, _>>()?))
        }
        Value::Object(m) if m.contains_key("find") => find(m.get("find").unwrap(), vars, ctx),
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, x) in m {
                out.insert(k.clone(), eval(x, vars, ctx)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn nums(args: &[Value]) -> Vec<f64> {
    // A single list argument is spread: ["sum", [1,2,3]]; ["avg", "field", list] picks a field.
    let list: Vec<Value> = match args {
        [Value::String(field), Value::Array(items)] => items.iter().map(|i| i.get(field).cloned().unwrap_or(Value::Null)).collect(),
        [Value::Array(items)] => items.clone(),
        _ => args.to_vec(),
    };
    list.iter().filter_map(to_num).collect()
}

fn cmp_vals(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (to_num(a), to_num(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y),
        _ => Some(display(a).cmp(&display(b))),
    }
}

fn apply_op(op: &str, a: &[Value]) -> Result<Value, String> {
    let need = |n: usize| if a.len() < n { Err(format!("'{op}' needs {n} argument(s)")) } else { Ok(()) };
    let n = |v: &Value| to_num(v).ok_or_else(|| format!("'{op}': {} is not a number", display(v)));
    Ok(match op {
        "=" => { need(2)?; json!(a[0] == a[1] || (to_num(&a[0]).is_some() && to_num(&a[0]) == to_num(&a[1]))
                     || (a[0].is_string() && a[1].is_string() && display(&a[0]).eq_ignore_ascii_case(&display(&a[1])))) }
        "!=" => { need(2)?; json!(!truthy(&apply_op("=", a)?)) }
        ">" => { need(2)?; json!(cmp_vals(&a[0], &a[1]) == Some(std::cmp::Ordering::Greater)) }
        "<" => { need(2)?; json!(cmp_vals(&a[0], &a[1]) == Some(std::cmp::Ordering::Less)) }
        ">=" => { need(2)?; json!(cmp_vals(&a[0], &a[1]) != Some(std::cmp::Ordering::Less)) }
        "<=" => { need(2)?; json!(cmp_vals(&a[0], &a[1]) != Some(std::cmp::Ordering::Greater)) }
        "and" => json!(a.iter().all(truthy)),
        "or" => json!(a.iter().any(truthy)),
        "not" => { need(1)?; json!(!truthy(&a[0])) }
        "+" => { need(2)?; if a[0].is_string() && to_num(&a[0]).is_none() { json!(format!("{}{}", display(&a[0]), display(&a[1]))) } else { num(n(&a[0])? + n(&a[1])?) } }
        "-" => { need(2)?; num(n(&a[0])? - n(&a[1])?) }
        "*" => { need(2)?; num(n(&a[0])? * n(&a[1])?) }
        "/" => { need(2)?; let d = n(&a[1])?; if d == 0.0 { return Err("division by zero".into()) } num(n(&a[0])? / d) }
        "sum" => num(nums(a).iter().sum()),
        "avg" => { let v = nums(a); if v.is_empty() { Value::Null } else { num(v.iter().sum::<f64>() / v.len() as f64) } }
        "min" => nums(a).into_iter().reduce(f64::min).map(num).unwrap_or(Value::Null),
        "max" => nums(a).into_iter().reduce(f64::max).map(num).unwrap_or(Value::Null),
        "count" => json!(match a { [Value::Array(x)] => x.len(), _ => a.len() }),
        "pct" => { need(2)?; let b = n(&a[1])?; if b == 0.0 { Value::Null } else { num(((n(&a[0])? - b) / b) * 100.0) } }
        "has" => { need(2)?; json!(match &a[0] { Value::Array(x) => x.contains(&a[1]),
                     Value::String(s) => s.to_lowercase().contains(&display(&a[1]).to_lowercase()),
                     Value::Object(m) => m.contains_key(&display(&a[1])), _ => false }) }
        "exists" => { need(1)?; json!(!a[0].is_null()) }
        "first" => { need(1)?; match &a[0] { Value::Array(x) => x.first().cloned().unwrap_or(Value::Null), v => v.clone() } }
        "get" => { need(2)?; a[0].get(display(&a[1])).cloned().unwrap_or(Value::Null) }
        // ["pick", list, field, value]: the first item whose field equals value (["pick", claims, "type", "load_offer"]);
        // without a value, the first item that has the field.
        "pick" => { need(2)?; let f = display(&a[1]); match &a[0] {
            Value::Array(x) => x.iter().find(|i| match (i.get(&f), a.get(2)) {
                (Some(h), Some(w)) => apply_op("=", &[h.clone(), w.clone()]).map(|r| truthy(&r)).unwrap_or(false),
                (Some(h), None) => !h.is_null(),
                _ => false,
            }).cloned().unwrap_or(Value::Null),
            _ => Value::Null } }
        _ => return Err(format!("unknown operator {op}")),
    })
}

/// Whether an object meets a `where`: {field: value | [op, value]} (the same form `find` uses).
pub fn where_matches(o: &Value, w: &Map<String, Value>) -> bool {
    w.iter().all(|(k, want)| {
        let have = o.get(k).cloned().unwrap_or(Value::Null);
        match want {
            Value::Array(x) if x.len() == 2 && x[0].as_str().map(|s| OPS.contains(&s)).unwrap_or(false) => {
                apply_op(x[0].as_str().unwrap(), &[have, x[1].clone()]).map(|r| truthy(&r)).unwrap_or(false)
            }
            _ => apply_op("=", &[have, want.clone()]).map(|r| truthy(&r)).unwrap_or(false),
        }
    })
}

/// `{"find": {"type": "bill", "from": "@verizon", "where": {"status": "unpaid"}, "pick": "newest", "last": 6}}`
fn find(spec: &Value, vars: &Map<String, Value>, ctx: &Ctx) -> Result<Value, String> {
    // Evaluate the spec, except that a where-condition [op, value] keeps its operator and only the value
    // is evaluated (["<", "$floor"] compares against $floor; it isn't itself an expression).
    let mut spec = spec.clone();
    if let Some(Value::Object(w)) = spec.get_mut("where") {
        for v in w.values_mut() {
            match v {
                Value::Array(x) if x.len() == 2 && x[0].as_str().map(|s| OPS.contains(&s)).unwrap_or(false) => {
                    x[1] = eval(&x[1], vars, ctx)?;
                }
                other => *other = eval(other, vars, ctx)?,
            }
        }
    }
    let where_ = spec.as_object_mut().and_then(|m| m.remove("where"));
    let mut spec = eval(&spec, vars, ctx)?;
    if let Some(w) = where_ {
        spec["where"] = w;
    }
    let typ = spec.get("type").and_then(Value::as_str).ok_or("find needs a type")?;
    let mut items = ctx.mem.objects_of(typ, 5000);
    if let Some(from) = spec.get("from").and_then(Value::as_str) {
        let f = from.trim_start_matches('@').to_lowercase();
        items.retain(|o| {
            ["from", "handle", "org"].iter().any(|k| {
                o.get(*k).map(|v| display(v).trim_start_matches('@').to_lowercase() == f).unwrap_or(false)
            })
        });
    }
    if let Some(Value::Object(w)) = spec.get("where") {
        items.retain(|o| where_matches(o, w));
    }
    match spec.get("pick").and_then(Value::as_str) {
        Some("newest") => return Ok(items.first().cloned().unwrap_or(Value::Null)),
        Some("oldest") => return Ok(items.last().cloned().unwrap_or(Value::Null)),
        _ => {}
    }
    if let Some(n) = spec.get("last").and_then(Value::as_u64) {
        items.truncate(n as usize);
    }
    Ok(Value::Array(items))
}

// ---------------------------------------------------------------------------------------------
// Checking
// ---------------------------------------------------------------------------------------------

const STEP_KINDS: &[&str] = &["let", "do", "if", "each", "expect", "try", "ask", "tell", "wait", "think", "call"];
const BUILTIN_VARS: &[&str] = &["state", "event", "answer", "error", "result", "now", "timed_out", "answered_by"];

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub ok: bool,
    pub errors: Vec<String>,
    pub effects: Vec<Value>,
    /// The plan must wait for the human before acting (untrusted actions, or a second look is needed).
    pub needs_approval: bool,
    /// Why a second look is needed even if every action is trusted: low confidence, open unknowns, first time.
    pub review: Vec<String>,
    /// The AI's own assessment, shown to the human.
    pub assessment: Value,
    /// The plan relies on information from outside (email, web, messages): it can't act on the outside world
    /// on its own unless the human has given standing permission for that, and must cite the facts it uses.
    pub outside: bool,
    /// The cited outside facts, as the core recorded them (provenance from the core, not the AI).
    pub outside_facts: Vec<Value>,
    pub explain: Vec<String>,
}

/// A target the human could actually see and say "always" to: a literal value, not `$var`, `{$var}` or a pattern.
fn literal_target(t: &str) -> bool {
    !t.contains('$') && !t.contains('*')
}

fn covered(mandates: &[(String, String)], verb: &str, target: &str) -> bool {
    // A standing permission names the value the human said yes to. A mandate recorded against a variable (before this
    // rule) would cover whatever the variable holds later, so it's ignored.
    mandates.iter().filter(|(_, t)| t == "*" || t == "outside:*" || literal_target(t.trim_end_matches('*'))).any(|(v, t)| {
        (v == verb || v == "*")
            && (t == "*" || t == target || (t.ends_with('*') && target.starts_with(t.trim_end_matches('*'))))
    })
}

fn collect_var_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if is_var(s) => out.push(s[1..].split('.').next().unwrap_or("").to_string()),
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("{$") {
                let tail = &rest[i + 2..];
                let end = tail.find(['.', '}']).unwrap_or(tail.len());
                out.push(tail[..end].to_string());
                rest = &tail[end..];
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_var_refs(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect_var_refs(x, out)),
        _ => {}
    }
}

pub fn check(plan: &Value, mem: &Memory, source: &str, threshold: f64) -> Report {
    check_with(plan, mem, source, threshold, None)
}

const ASSESS_HELP: &str = "before acting, assess it: add \"assess\": {\"confidence\": 0-1, \"consequences\": [what this will cause], \"outcomes\": [{\"if\": what might happen, \"then\": what you'd do}], \"unknowns\": [what you don't know yet], \"research\": what you looked into, \"verify\": [what the plan's own steps check about the result, if any]}";

/// `novelty` is Some(reason) when the plan is a first-time situation (decided by the core from playbooks and cases).
pub fn check_with(plan: &Value, mem: &Memory, source: &str, threshold: f64, novelty: Option<&str>) -> Report {
    let mut r = Report::default();
    let steps = match plan.get("steps") {
        Some(Value::Array(s)) => s.clone(),
        _ => {
            r.errors.push("plan needs a \"steps\" list".into());
            return r;
        }
    };
    if plan.get("plan").and_then(Value::as_str).unwrap_or("").is_empty() {
        r.errors.push("plan needs a name in \"plan\"".into());
    }
    // Standing permissions given for actions based on outside information are kept apart ("outside:" targets):
    // ordinary trust doesn't cover acting on what strangers sent, and vice versa.
    // A plan that waits for a message (a broker's reply) acts on what a stranger sends: it's outside from the start,
    // so its effects need the same OK as any plan resting on outside information.
    let outside = plan["taint"] == "outside" || waits_on_outside(&steps);
    let mandates: Vec<(String, String)> = mem.mandates().into_iter().filter_map(|(v, t)| match t.strip_prefix("outside:") {
        Some(rest) => outside.then(|| (v, rest.to_string())),
        None => (!outside).then_some((v, t)),
    }).collect();
    r.outside = outside;
    let mut defined: Vec<String> = BUILTIN_VARS.iter().map(|s| s.to_string()).collect();
    if let Some(Value::Object(w)) = plan.get("with") {
        defined.extend(w.keys().cloned());
    }
    check_block(&steps, "steps", &mut defined, &mandates, &mut r, 0);
    let ai = source == "ai";
    let has_effects = r.effects.iter().any(|e| e["effect"] != "internal");
    let uncovered = r.effects.iter().any(|e| e["covered"] == false);
    if ai && has_effects {
        assess(plan, threshold, novelty, &mut r);
    }
    if plan["taint"] == "outside" && has_effects {
        // Show the human exactly which outside facts this rests on, as recorded when they arrived.
        let cited = plan["reasoning"]["facts"].as_array().cloned().unwrap_or_default();
        if cited.is_empty() {
            r.errors.push("this plan relies on outside information: cite the stored facts it uses in reasoning.facts (object ids)".into());
        }
        for f in cited {
            match f.as_str().and_then(|id| mem.object(id)) {
                Some(o) if o["superseded_by"].is_string() => r.errors.push(format!(
                    "fact {} was corrected by {}: cite the newer one", display(&f), display(&o["superseded_by"]))),
                Some(o) if o["trust"] == "outside" => r.outside_facts.push(o),
                Some(_) => r.errors.push(format!("reasoning.facts: {} isn't a recorded outside fact (cite the facts \
                    extracted from the message or page)", display(&f))),
                None => r.errors.push(format!("reasoning.facts: no stored fact {f}")),
            }
        }
    }
    r.needs_approval = ai && (uncovered || !r.review.is_empty());
    r.ok = r.errors.is_empty();
    r
}

/// Event kinds that carry what other people wrote.
fn outside_event(kind: &str) -> bool {
    kind == "*" || kind.starts_with("message.") || kind.starts_with("invite.") || kind.starts_with("web.")
}

fn waits_on_outside(steps: &[Value]) -> bool {
    steps.iter().any(|s| {
        s.get("wait").and_then(Value::as_str).map(|k| parse_duration(k).is_none() && outside_event(k)).unwrap_or(false)
            || ["then", "else", "steps", "try"].iter().any(|k| s.get(*k).and_then(Value::as_array).map(|a| waits_on_outside(a)).unwrap_or(false))
    })
}

fn strings(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().map(|x| match x {
            Value::Object(o) if o.contains_key("if") => format!("if {}: {}", display(&o["if"]), display(o.get("then").unwrap_or(&Value::Null))),
            other => display(other),
        }).filter(|s| !s.trim().is_empty()).collect(),
        Value::String(s) if !s.trim().is_empty() => vec![s.clone()],
        _ => vec![],
    }
}

/// The AI must understand what it's about to do before doing it: confidence, consequences, what it would do
/// in each outcome, what's still unknown. Low confidence, unknowns or a first-time situation get a second look.
fn assess(plan: &Value, threshold: f64, novelty: Option<&str>, r: &mut Report) {
    let a = plan.get("assess").cloned().unwrap_or(Value::Null);
    if !a.is_object() {
        r.errors.push(ASSESS_HELP.into());
        return;
    }
    let conf = a.get("confidence").or(plan.get("conf")).and_then(Value::as_f64);
    let consequences = strings(&a["consequences"]);
    let outcomes = strings(&a["outcomes"]);
    let unknowns = strings(&a["unknowns"]);
    let research = strings(&a["research"]);
    let verify = strings(&a["verify"]);
    if conf.is_none() || consequences.is_empty() || outcomes.is_empty() {
        r.errors.push(format!("the assessment needs confidence, consequences and outcomes; {ASSESS_HELP}"));
        return;
    }
    let conf = conf.unwrap().clamp(0.0, 1.0);
    if conf < threshold {
        r.review.push(format!("my confidence is {:.0}%", conf * 100.0));
    }
    if !unknowns.is_empty() {
        r.review.push(format!("still unknown: {}", unknowns.join("; ")));
    }
    if let Some(reason) = novelty {
        if research.is_empty() {
            r.errors.push(format!("{reason}: research it first (playbooks, similar cases, read-only investigation) and put what you found in assess.research, or ask the human"));
            return;
        }
        r.review.push(reason.to_string());
    }
    r.assessment = json!({"confidence": conf, "consequences": consequences, "outcomes": outcomes,
                          "unknowns": unknowns, "research": research, "verify": verify});
}

fn check_block(steps: &[Value], path: &str, defined: &mut Vec<String>, mandates: &[(String, String)], r: &mut Report, depth: usize) {
    let pad = "  ".repeat(depth);
    for (i, step) in steps.iter().enumerate() {
        let at = format!("{path}[{i}]");
        let Value::Object(m) = step else {
            r.errors.push(format!("{at}: a step must be an object"));
            continue;
        };
        let kinds: Vec<&str> = STEP_KINDS.iter().copied().filter(|k| m.contains_key(*k)).collect();
        let kind = match kinds.as_slice() {
            [k] => *k,
            ["let", _] | [_, "let"] => "let",
            [] => {
                r.errors.push(format!("{at}: unknown step (expected one of {})", STEP_KINDS.join(", ")));
                continue;
            }
            more => {
                // `do` + `as`, `ask` + `as` etc. are fine; several step keywords are not.
                r.errors.push(format!("{at}: step mixes {}", more.join(" + ")));
                continue;
            }
        };
        let mut refs = Vec::new();
        let scan: Vec<&Value> = match kind {
            "if" => vec![&m["if"]],
            "each" => vec![&m["each"]],
            "expect" => vec![&m["expect"]],
            "try" => vec![],
            _ => m.values().collect(),
        };
        scan.iter().for_each(|v| collect_var_refs(v, &mut refs));
        for name in refs {
            if !name.is_empty() && !defined.contains(&name) {
                r.errors.push(format!("{at}: ${name} is used before it is defined"));
            }
        }
        match kind {
            "let" => {
                match m.get("let").and_then(Value::as_str) {
                    Some(n) => defined.push(n.to_string()),
                    None => r.errors.push(format!("{at}: let needs a variable name")),
                }
                if !(m.contains_key("find") || m.contains_key("calc") || m.contains_key("value") || m.contains_key("do")) {
                    r.errors.push(format!("{at}: let needs find, calc, value or do"));
                }
                if let Some(Value::String(verb)) = m.get("do") {
                    check_do(verb, m, &at, mandates, r, &pad);
                } else {
                    r.explain.push(format!("{pad}look up {}", m["let"].as_str().unwrap_or("?")));
                }
            }
            "do" => {
                let verb = m["do"].as_str().unwrap_or("");
                check_do(verb, m, &at, mandates, r, &pad);
                if let Some(n) = m.get("as").and_then(Value::as_str) {
                    defined.push(n.to_string());
                }
            }
            "if" => {
                r.explain.push(format!("{pad}if {}:", short(&m["if"])));
                let then = m.get("then").and_then(Value::as_array).cloned().unwrap_or_default();
                if then.is_empty() && m.get("else").is_none() {
                    r.errors.push(format!("{at}: if needs then and/or else"));
                }
                check_block(&then, &format!("{at}.then"), defined, mandates, r, depth + 1);
                if let Some(Value::Array(e)) = m.get("else") {
                    r.explain.push(format!("{pad}otherwise:"));
                    check_block(e, &format!("{at}.else"), defined, mandates, r, depth + 1);
                }
            }
            "each" => {
                let var = m.get("as").and_then(Value::as_str).unwrap_or("item").to_string();
                defined.push(var.clone());
                r.explain.push(format!("{pad}for each {var} in {}:", short(&m["each"])));
                let body = m.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
                if body.is_empty() {
                    r.errors.push(format!("{at}: each needs steps"));
                }
                check_block(&body, &format!("{at}.steps"), defined, mandates, r, depth + 1);
            }
            "expect" => {
                r.explain.push(format!("{pad}make sure {}", short(&m["expect"])));
                if let Some(e) = m.get("else").and_then(Value::as_str) {
                    if !["stop", "think"].contains(&e) {
                        r.errors.push(format!("{at}: expect else must be \"stop\" or \"think\""));
                    }
                }
            }
            "try" => {
                let body = m["try"].as_array().cloned().unwrap_or_default();
                check_block(&body, &format!("{at}.try"), defined, mandates, r, depth + 1);
                if let Some(Value::Array(e)) = m.get("else") {
                    r.explain.push(format!("{pad}if that fails:"));
                    check_block(e, &format!("{at}.else"), defined, mandates, r, depth + 1);
                }
            }
            "ask" => {
                defined.push(m.get("as").and_then(Value::as_str).unwrap_or("answer").to_string());
                let fb = m.get("fallback").map(|f| format!(" (no answer by {}: {})", display(m.get("deadline").unwrap_or(&json!("the deadline"))), display(f))).unwrap_or_default();
                r.explain.push(format!("{pad}ask you: {}{fb}", display(&m["ask"])));
            }
            "tell" => r.explain.push(format!("{pad}tell you: {}", display(&m["tell"]))),
            "wait" => r.explain.push(match m.get("where") {
                Some(w) => format!("{pad}wait for {} where {}", display(&m["wait"]), short(w)),
                None => format!("{pad}wait for {}", display(&m["wait"])),
            }),
            "think" => r.explain.push(format!("{pad}stop and think: {}", display(&m["think"]))),
            "call" => r.explain.push(format!("{pad}run saved plan {}", display(&m["call"]))),
            _ => {}
        }
    }
}

fn check_do(verb: &str, m: &Map<String, Value>, at: &str, mandates: &[(String, String)], r: &mut Report, pad: &str) {
    let Some(spec) = verbs::find(verb) else {
        r.errors.push(format!("{at}: unknown verb '{verb}'"));
        return;
    };
    for (arg, required) in spec.args {
        if *required && !m.contains_key(*arg) {
            r.errors.push(format!("{at}: {verb} needs '{arg}'"));
        }
    }
    let known: Vec<&str> = spec.args.iter().map(|(a, _)| *a).chain(["do", "as", "let"]).collect();
    for k in m.keys() {
        if !known.contains(&k.as_str()) {
            r.errors.push(format!("{at}: {verb} has no argument '{k}'"));
        }
    }
    let target = m.get(spec.target).map(display).unwrap_or_default();
    let args = Value::Object(m.clone());
    let text = verbs::explain(spec, &args);
    let is_covered = spec.effect == Effect::Internal || covered(mandates, verb, &target);
    r.explain.push(format!("{pad}{text}"));
    r.effects.push(json!({"at": at, "verb": verb, "target": target, "effect": spec.effect.name(),
                          "explain": text, "covered": is_covered}));
}

fn short(v: &Value) -> String {
    let s = match v {
        Value::Array(a) if a.len() == 3 && a[0].is_string() => {
            format!("{} {} {}", display(&a[1]), display(&a[0]), display(&a[2]))
        }
        other => display(other),
    };
    s.chars().take(80).collect()
}

// ---------------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------------

enum Flow {
    Next,
    Push(Frame),
    Pause,
    Stop(String),
}

pub fn parse_duration(s: &str) -> Option<f64> {
    let t = s.trim().trim_start_matches('+');
    let (n, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit() && c != '.')?);
    let n: f64 = n.parse().ok()?;
    let mult = match unit.trim() {
        "s" | "sec" | "secs" | "seconds" => 1.0,
        "m" | "min" | "mins" | "minutes" => 60.0,
        "h" | "hr" | "hrs" | "hours" => 3600.0,
        "d" | "day" | "days" => 86400.0,
        "w" | "week" | "weeks" => 604800.0,
        _ => return None,
    };
    Some(n * mult)
}

pub fn new_exec(plan: &Value, source: &str) -> Exec {
    let steps = plan.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut vars = Map::new();
    if let Some(Value::Object(w)) = plan.get("with") {
        vars.extend(w.clone());
    }
    Exec {
        id: new_id("p"),
        name: plan.get("plan").and_then(Value::as_str).unwrap_or("unnamed").to_string(),
        why: plan.get("why").and_then(Value::as_str).unwrap_or("").to_string(),
        source: source.to_string(),
        plan: plan.clone(),
        vars,
        stack: vec![Frame { steps, idx: 0, kind: FrameKind::Block }],
        status: "running".into(),
        wait: Value::Null,
        counter: 0,
        approved: !source.eq("ai"),
        error: None,
        created: now(),
        human_answered: false,
    }
}

/// Start a checked plan. AI plans that need approval first pause with a question to the human.
pub fn start(exec: &mut Exec, report: &Report, ctx: &Ctx) {
    if report.needs_approval && !exec.approved {
        request_approval(exec, report, ctx);
        return;
    }
    exec.approved = true;
    ctx.bus.emit(ctx.mem, "plan.started", exec.summary());
    run(exec, ctx);
}

/// Pause the plan and ask the human to OK the actions the AI isn't trusted with yet.
fn request_approval(exec: &mut Exec, report: &Report, ctx: &Ctx) {
    exec.approved = false;
    {
        let lines: Vec<String> = report
            .effects
            .iter()
            .filter(|e| e["effect"] != "internal")
            .map(|e| format!("• {}", display(&e["explain"])))
            .collect();
        let why = if exec.why.is_empty() { String::new() } else { format!(" ({})", exec.why) };
        let a = &report.assessment;
        let mut q = format!("I'd like to {}{}:\n{}", exec.name, why, lines.join("\n"));
        let list = |k: &str| a[k].as_array().map(|v| v.iter().map(display).collect::<Vec<_>>()).unwrap_or_default();
        for (label, key) in [("What I expect", "consequences"), ("If it goes differently", "outcomes"),
                             ("Still unknown", "unknowns"), ("What I looked into", "research")] {
            let items = list(key);
            if !items.is_empty() {
                q.push_str(&format!("\n{label}: {}", items.join("; ")));
            }
        }
        if let Some(c) = a["confidence"].as_f64() {
            q.push_str(&format!("\nConfidence: {:.0}%", c * 100.0));
        }
        // The reasoning modules (facts, policies, chain, decision, downstream), as the AI laid them out.
        let rsn = &exec.plan["reasoning"];
        let items = |k: &str| rsn[k].as_array().map(|v| v.iter().map(display).collect::<Vec<_>>()).unwrap_or_default();
        let policies = items("policies");
        if !policies.is_empty() {
            q.push_str(&format!("\nRules applied: {}", policies.join("; ")));
        }
        let chain = items("chain");
        if !chain.is_empty() {
            q.push_str(&format!("\nReasoning: {}", chain.join(" → ")));
        }
        if let Some(d) = rsn["decision"].as_str() {
            q.push_str(&format!("\nDecision: {d}"));
        }
        let down = items("downstream");
        if !down.is_empty() {
            q.push_str(&format!("\nIf this runs: {}", down.join("; ")));
        }
        if !report.outside_facts.is_empty() {
            q.push_str("\nBased on outside information:");
            for f in &report.outside_facts {
                q.push_str(&format!("\n• {}", describe_fact(f)));
            }
        }
        q.push_str("\nOK?");
        let question = q;
        let untrusted = report.effects.iter().any(|e| e["effect"] != "internal" && e["covered"] == false);
        let reason = match (report.review.is_empty(), untrusted) {
            (false, _) => format!("I want a second look before acting: {}.", report.review.join("; ")),
            (true, true) if report.outside => "This acts on information from outside, and you haven't told me I can do that on my own yet. Answer \"always\" to let me do this kind of thing from outside information.".to_string(),
            (true, _) => "You haven't told me I can do these on my own yet. Answer \"always\" and I won't ask about them again.".to_string(),
        };
        exec.status = "waiting:answer".into();
        let t0 = now();
        exec.wait = json!({"question": question, "opts": ["yes", "no", "always"], "approval": true,
                           "effects": report.effects, "review": report.review, "outside": report.outside,
                           "why": reason,
                           "reminders": [t0 + 2.0 * 3600.0, t0 + 8.0 * 3600.0], "asked_at": t0});
        persist(exec, ctx);
        ctx.bus.emit(ctx.mem, "plan.waiting", exec.summary());
    }
}

/// How a reply to an approval reads: Some(true) for always, Some(false) for a plain yes, None for no.
/// Anything that isn't clearly a yes is a no.
pub fn read_approval(text: &str) -> Option<bool> {
    let low = text.trim().to_lowercase();
    let words: Vec<&str> = low.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    let negative = ["no", "nope", "nah", "not", "never", "don't", "dont", "stop", "cancel", "wait", "hold"];
    if words.is_empty() || words.iter().any(|w| negative.contains(w)) {
        return None;
    }
    let yes = ["yes", "y", "yeah", "yep", "ok", "okay", "sure", "go", "approve", "approved", "do", "always", "fine"];
    if !yes.contains(&words[0]) {
        return None;
    }
    Some(words.contains(&"always"))
}

pub(crate) fn persist(exec: &Exec, ctx: &Ctx) {
    ctx.mem.save_plan(&exec.id, &exec.name, &exec.source, &exec.status, &serde_json::to_value(exec).unwrap());
}

/// Feed the human's answer to a plan waiting on `ask` (or on approval).
pub fn answer(exec: &mut Exec, text: &str, by: &str, ctx: &Ctx) {
    if exec.status != "waiting:answer" {
        return;
    }
    if exec.wait.get("approval").and_then(Value::as_bool).unwrap_or(false) {
        if let Some(always) = read_approval(text) {
            if always {
                // Trust learned from conversation: these kinds of actions no longer need asking.
                for e in exec.wait["effects"].as_array().cloned().unwrap_or_default() {
                    if e["effect"] != "internal" && e["covered"] == false {
                        // "Always" covers the value the human saw. A target that's a variable or a pattern ("$broker",
                        // "s*") could become anything later, so this yes counts for this plan only.
                        if !literal_target(&display(&e["target"])) {
                            continue;
                        }
                        let outside = exec.wait["outside"] == true;
                        let target = if outside { format!("outside:{}", display(&e["target"])) } else { display(&e["target"]) };
                        ctx.mem.add_mandate(e["verb"].as_str().unwrap_or(""), &target, if outside { "human:always:outside" } else { "human:always" });
                    }
                }
                ctx.bus.emit(ctx.mem, "trust.granted", json!({"plan": exec.name}));
            }
            exec.approved = true;
            exec.status = "running".into();
            exec.wait = Value::Null;
            ctx.bus.emit(ctx.mem, "plan.started", exec.summary());
            run(exec, ctx);
        } else {
            exec.status = "stopped".into();
            exec.error = Some(format!("human said: {text}"));
            persist(exec, ctx);
            ctx.bus.emit(ctx.mem, "plan.stopped", exec.summary());
        }
        return;
    }
    let var = exec.wait.get("as").and_then(Value::as_str).unwrap_or("answer").to_string();
    exec.vars.insert(var, json!(text));
    exec.vars.insert("answer".into(), json!(text));
    exec.vars.insert("answered_by".into(), json!(by));
    if by == "human" {
        exec.human_answered = true;
    }
    exec.status = "running".into();
    exec.wait = Value::Null;
    run(exec, ctx);
}

/// The AI's judgment for a plan that hit `think`: either more steps to run, or stop.
pub fn resolve_thought(exec: &mut Exec, steps: Option<Vec<Value>>, report: Option<&Report>, ctx: &Ctx) {
    if exec.status != "needs_thought" {
        return;
    }
    exec.wait = Value::Null;
    match steps {
        Some(s) => {
            exec.status = "running".into();
            exec.stack.push(Frame { steps: s, idx: 0, kind: FrameKind::Block });
            match report {
                Some(r) if r.needs_approval => request_approval(exec, r, ctx),
                _ => run(exec, ctx),
            }
        }
        None => {
            exec.status = "stopped".into();
            persist(exec, ctx);
            ctx.bus.emit(ctx.mem, "plan.stopped", exec.summary());
        }
    }
}

/// Resume plans waiting for this event (or whose wait timed out).
/// The id of an exchange's thread object.
pub fn thread_id(key: &str) -> String {
    let slug: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.:".contains(c) { c } else { '-' }).collect();
    format!("obj:thread/{slug}")
}

/// Update (or start) an exchange's state. Given fields replace, the rest are kept; the last few states stay as history.
pub fn track(ctx: &Ctx, a: &Map<String, Value>, plan: &str) -> String {
    let key = str_arg(a, "thread");
    let id = thread_id(&key);
    let mut t = ctx.mem.object(&id).unwrap_or_else(|| json!({"thread": key, "status": "open", "history": [], "started": now()}));
    if let Some(prev) = t.get("state").filter(|s| !s.is_null()).cloned() {
        if a.get("state").map(|s| *s != prev).unwrap_or(false) {
            let mut h = t["history"].as_array().cloned().unwrap_or_default();
            h.push(json!({"state": prev, "at": t["updated"]}));
            let n = h.len();
            t["history"] = json!(h.into_iter().skip(n.saturating_sub(6)).collect::<Vec<_>>());
        }
    }
    for k in ["with", "about", "state", "waiting_for", "status"] {
        if let Some(v) = a.get(k).filter(|v| !v.is_null()) {
            t[k] = v.clone();
        }
    }
    t["updated"] = json!(now());
    t["by_plan"] = json!(plan);
    if let Value::Object(m) = &mut t {
        m.remove("id");
        m.remove("type");
    }
    ctx.mem.put_object_with_id(&id, "thread", &t);
    ctx.bus.emit(ctx.mem, "thread.updated", json!({"id": id, "thread": key, "state": t["state"], "status": t["status"]}));
    id
}

/// Whether a plan waiting on an event takes this one: the kind matches and, if the wait has a `where`, it holds
/// (evaluated with the plan's variables plus $event).
pub fn awaits(vars: &Map<String, Value>, wait: &Value, kind: &str, event: &Value, ctx: &Ctx) -> bool {
    if !event_matches(wait["kind"].as_str().unwrap_or(""), kind) {
        return false;
    }
    match wait.get("where").filter(|w| !w.is_null()) {
        None => true,
        Some(cond) => {
            let mut v = vars.clone();
            v.insert("event".into(), event.clone());
            eval(cond, &v, ctx).map(|x| truthy(&x)).unwrap_or(false)
        }
    }
}

pub fn on_event(exec: &mut Exec, kind: &str, event: &Value, ctx: &Ctx) {
    if exec.status == "waiting:event" && awaits(&exec.vars, &exec.wait, kind, event, ctx) {
        exec.vars.insert("event".into(), event.clone());
        exec.vars.insert("timed_out".into(), json!(false));
        exec.status = "running".into();
        exec.wait = Value::Null;
        run(exec, ctx);
    }
}

pub fn on_tick(exec: &mut Exec, t: f64, ctx: &Ctx) {
    if exec.status == "waiting:answer" {
        // Follow up on schedule; at the deadline, take the fallback answer (and say so) if there is one.
        let due: Vec<f64> = exec.wait["reminders"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let (fired, left): (Vec<f64>, Vec<f64>) = due.into_iter().partition(|r| *r <= t);
        let deadline = exec.wait.get("until").and_then(Value::as_f64).filter(|u| t >= *u);
        if fired.is_empty() && deadline.is_none() {
            return;
        }
        exec.wait["reminders"] = json!(left);
        if deadline.is_some() {
            exec.wait["until"] = Value::Null;
            if let Some(fb) = exec.wait.get("fallback").and_then(Value::as_str).map(String::from) {
                let q = display(&exec.wait["question"]);
                ctx.bus.emit(ctx.mem, "ai.tell", json!({"plan": exec.name, "plan_id": exec.id,
                    "text": format!("I didn't hear back on \"{q}\" in time, so I went with \"{fb}\" as planned. Tell me if you want it changed.")}));
                answer(exec, &fb, "fallback", ctx);
                return;
            }
        }
        persist(exec, ctx);
        ctx.bus.emit(ctx.mem, "plan.reminder", exec.summary());
        return;
    }
    let until = exec.wait.get("until").and_then(Value::as_f64);
    if let Some(u) = until {
        if t >= u && (exec.status == "waiting:time" || exec.status == "waiting:event") {
            exec.vars.insert("timed_out".into(), json!(exec.status == "waiting:event"));
            exec.status = "running".into();
            exec.wait = Value::Null;
            run(exec, ctx);
        }
    }
}

pub fn event_matches(pattern: &str, kind: &str) -> bool {
    pattern == "*" || pattern == kind || (pattern.ends_with(".*") && kind.starts_with(&pattern[..pattern.len() - 1]))
}

pub fn run(exec: &mut Exec, ctx: &Ctx) {
    // Saved before anything happens, so a crash during the very first step still leaves a plan to resume.
    persist(exec, ctx);
    let mut guard = 0u32;
    loop {
        guard += 1;
        if guard > 100_000 {
            fail(exec, "plan ran too many steps".into(), ctx);
            return;
        }
        let Some(frame) = exec.stack.last_mut() else {
            exec.status = "done".into();
            persist(exec, ctx);
            ctx.bus.emit(ctx.mem, "plan.finished", exec.summary());
            return;
        };
        if frame.idx >= frame.steps.len() {
            if let FrameKind::Each { var, items, pos } = &mut frame.kind {
                *pos += 1;
                if *pos < items.len() {
                    let (name, item) = (var.clone(), items[*pos].clone());
                    frame.idx = 0;
                    exec.vars.insert(name, item);
                    continue;
                }
            }
            exec.stack.pop();
            continue;
        }
        let step = frame.steps[frame.idx].clone();
        frame.idx += 1;
        exec.counter += 1;
        match exec_step(exec, &step, ctx) {
            Ok(Flow::Next) => {}
            Ok(Flow::Push(f)) => exec.stack.push(f),
            Ok(Flow::Pause) => {
                persist(exec, ctx);
                let kind = if exec.status == "needs_thought" { "plan.needs_thought" } else { "plan.waiting" };
                ctx.bus.emit(ctx.mem, kind, exec.summary());
                return;
            }
            Ok(Flow::Stop(msg)) => {
                exec.status = "stopped".into();
                exec.error = Some(msg);
                persist(exec, ctx);
                ctx.bus.emit(ctx.mem, "plan.stopped", exec.summary());
                return;
            }
            Err(e) => {
                if let Some(pos) = exec.stack.iter().rposition(|f| matches!(f.kind, FrameKind::Try { .. })) {
                    let otherwise = match &exec.stack[pos].kind {
                        FrameKind::Try { otherwise } => otherwise.clone(),
                        _ => unreachable!(),
                    };
                    exec.stack.truncate(pos);
                    exec.vars.insert("error".into(), json!(e));
                    exec.stack.push(Frame { steps: otherwise, idx: 0, kind: FrameKind::Block });
                    continue;
                }
                fail(exec, e, ctx);
                return;
            }
        }
        persist(exec, ctx);
    }
}

fn fail(exec: &mut Exec, e: String, ctx: &Ctx) {
    exec.status = "failed".into();
    exec.error = Some(e);
    persist(exec, ctx);
    ctx.bus.emit(ctx.mem, "plan.failed", exec.summary());
}

fn str_arg(m: &Map<String, Value>, k: &str) -> String {
    m.get(k).map(display).unwrap_or_default()
}

fn exec_step(exec: &mut Exec, step: &Value, ctx: &Ctx) -> Result<Flow, String> {
    let m = step.as_object().ok_or("step must be an object")?;
    let vars = exec.vars.clone();
    if let Some(name) = m.get("let").and_then(Value::as_str) {
        let v = if let Some(spec) = m.get("find") {
            find(spec, &vars, ctx)?
        } else if let Some(e) = m.get("calc") {
            eval(e, &vars, ctx)?
        } else if m.contains_key("do") {
            do_verb(exec, m, ctx)?
        } else {
            eval(m.get("value").unwrap_or(&Value::Null), &vars, ctx)?
        };
        exec.vars.insert(name.to_string(), v);
        return Ok(Flow::Next);
    }
    if m.contains_key("do") {
        let result = do_verb(exec, m, ctx)?;
        if let Some(n) = m.get("as").and_then(Value::as_str) {
            exec.vars.insert(n.to_string(), result.clone());
        }
        exec.vars.insert("result".into(), result);
        return Ok(Flow::Next);
    }
    if let Some(cond) = m.get("if") {
        let branch = if truthy(&eval(cond, &vars, ctx)?) { m.get("then") } else { m.get("else") };
        return Ok(match branch.and_then(Value::as_array) {
            Some(s) if !s.is_empty() => Flow::Push(Frame { steps: s.clone(), idx: 0, kind: FrameKind::Block }),
            _ => Flow::Next,
        });
    }
    if let Some(list) = m.get("each") {
        let items = match eval(list, &vars, ctx)? {
            Value::Array(a) => a,
            Value::Null => vec![],
            other => vec![other],
        };
        let var = m.get("as").and_then(Value::as_str).unwrap_or("item").to_string();
        let body = m.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
        if items.is_empty() || body.is_empty() {
            return Ok(Flow::Next);
        }
        exec.vars.insert(var.clone(), items[0].clone());
        return Ok(Flow::Push(Frame { steps: body, idx: 0, kind: FrameKind::Each { var, items, pos: 0 } }));
    }
    if let Some(cond) = m.get("expect") {
        if truthy(&eval(cond, &vars, ctx)?) {
            return Ok(Flow::Next);
        }
        let why = format!("expectation failed: {}", short(cond));
        return Ok(match m.get("else").and_then(Value::as_str) {
            Some("think") => {
                exec.status = "needs_thought".into();
                exec.wait = json!({"note": why});
                Flow::Pause
            }
            _ => Flow::Stop(why),
        });
    }
    if let Some(body) = m.get("try") {
        let otherwise = m.get("else").and_then(Value::as_array).cloned().unwrap_or_default();
        let steps = body.as_array().cloned().unwrap_or_default();
        return Ok(Flow::Push(Frame { steps, idx: 0, kind: FrameKind::Try { otherwise } }));
    }
    if let Some(q) = m.get("ask") {
        // Undecided: reach out with the question and the reason, follow up, and fall back if time matters.
        let question = display(&eval(q, &vars, ctx)?);
        let opts = m.get("opts").cloned().unwrap_or(json!([]));
        let var = m.get("as").and_then(Value::as_str).unwrap_or("answer");
        let text = |k: &str| -> Result<Value, String> {
            Ok(m.get(k).map(|v| eval(v, &vars, ctx)).transpose()?.map(|v| json!(display(&v))).unwrap_or(Value::Null))
        };
        let t0 = now();
        let reminders: Vec<Value> = m.get("remind").and_then(Value::as_array).map(|a| {
            a.iter().filter_map(|d| parse_duration(&display(d))).map(|s| json!(t0 + s)).collect()
        }).unwrap_or_default();
        let deadline = m.get("deadline").map(display).and_then(|d| parse_duration(&d)).map(|s| t0 + s);
        exec.status = "waiting:answer".into();
        exec.wait = json!({"question": question, "opts": opts, "as": var, "why": text("why")?, "lean": text("lean")?,
                           "reminders": reminders, "until": deadline, "fallback": text("fallback")?, "asked_at": t0});
        return Ok(Flow::Pause);
    }
    if let Some(t) = m.get("tell") {
        let text = display(&eval(t, &vars, ctx)?);
        ctx.bus.emit(ctx.mem, "ai.tell", json!({"text": text, "plan": exec.name, "plan_id": exec.id}));
        return Ok(Flow::Next);
    }
    if let Some(w) = m.get("wait") {
        let what = display(&eval(w, &vars, ctx)?);
        if let Some(secs) = parse_duration(&what) {
            exec.status = "waiting:time".into();
            exec.wait = json!({"until": now() + secs});
        } else {
            let max = m.get("max").map(display).and_then(|s| parse_duration(&s));
            exec.status = "waiting:event".into();
            exec.wait = json!({"kind": what, "until": max.map(|s| now() + s)});
            // Only the event this plan is actually waiting for (a reply from this broker about this load, say),
            // checked with the plan's own variables plus $event. `about` says what's expected, in words.
            if let Some(cond) = m.get("where") {
                exec.wait["where"] = cond.clone();
            }
            if let Some(a) = m.get("about") {
                exec.wait["about"] = json!(display(&eval(a, &vars, ctx)?));
            }
            if let Some(t) = m.get("thread") {
                exec.wait["thread"] = json!(display(&eval(t, &vars, ctx)?));
            }
        }
        return Ok(Flow::Pause);
    }
    if let Some(note) = m.get("think") {
        exec.status = "needs_thought".into();
        exec.wait = json!({"note": display(&eval(note, &vars, ctx)?)});
        // A named situation ("service.failed:cups"): once the AI's answer to it has worked, the core replays that
        // answer next time instead of waking the AI (a habit).
        if let Some(h) = m.get("habit") {
            exec.wait["habit"] = json!(display(&eval(h, &vars, ctx)?));
        }
        return Ok(Flow::Pause);
    }
    if let Some(name) = m.get("call").and_then(Value::as_str) {
        let saved = ctx.mem.named(name).ok_or_else(|| format!("no saved plan named {name}"))?;
        if let Some(Value::Object(w)) = m.get("with") {
            for (k, v) in w {
                exec.vars.insert(k.clone(), eval(v, &vars, ctx)?);
            }
        }
        let steps = saved.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
        return Ok(Flow::Push(Frame { steps, idx: 0, kind: FrameKind::Block }));
    }
    Err(format!("unknown step: {}", short(step)))
}

fn do_verb(exec: &mut Exec, m: &Map<String, Value>, ctx: &Ctx) -> Result<Value, String> {
    let verb = m.get("do").and_then(Value::as_str).ok_or("do needs a verb")?;
    let spec = verbs::find(verb).ok_or_else(|| format!("unknown verb {verb}"))?;
    let vars = exec.vars.clone();
    let mut args = Map::new();
    for (k, v) in m {
        if !["do", "as", "let"].contains(&k.as_str()) {
            args.insert(k.clone(), eval(v, &vars, ctx)?);
        }
    }
    let args = Value::Object(args);
    match verb {
        "tell" => {
            let text = str_arg(args.as_object().unwrap(), "text");
            ctx.bus.emit(ctx.mem, "ai.tell", json!({"text": text, "plan": exec.name, "plan_id": exec.id}));
            return Ok(Value::Null);
        }
        "remember" => {
            let a = args.as_object().unwrap();
            let id = ctx.mem.put_object("note", None, &json!({"text": str_arg(a, "text"),
                "kind": a.get("kind").cloned().unwrap_or(json!("general")), "plan": exec.name}));
            return Ok(json!(id));
        }
        "track" => {
            let a = args.as_object().unwrap();
            return Ok(json!(track(ctx, a, &exec.name)));
        }
        "agree" => {
            let a = args.as_object().unwrap();
            let reference = str_arg(a, "reference");
            let id = crate::agreements::id(&reference);
            // It's an agreement only if the human made it: their own plan, or their own answer in this one. Otherwise
            // (the AI's reading of an email, say) it's proposed: documents are still compared with it, but a match
            // isn't filed on its strength until the human confirms it.
            let by_human = exec.source == "human" || exec.human_answered;
            let status = if by_human { "agreed" } else { "proposed" };
            let fields = json!({"reference": reference, "terms": a.get("terms").cloned().unwrap_or(json!({})), "with": a.get("with"),
                "about": a.get("about"), "thread": a.get("thread"), "status": status, "confirmed_by_human": by_human,
                "documents": [], "by_plan": exec.name, "agreed": now()});
            ctx.mem.put_object_with_id(&id, "agreement", &fields);
            ctx.bus.emit(ctx.mem, if by_human { "agreement.made" } else { "agreement.proposed" },
                         json!({"id": id, "reference": reference, "terms": fields["terms"]}));
            if !by_human {
                ctx.bus.emit(ctx.mem, "ai.tell", json!({"plan": exec.name, "plan_id": exec.id, "text": format!(
                    "I noted what I believe was agreed on {reference}: {}. It counts once you confirm it.", human("terms", &fields["terms"]))}));
            }
            return Ok(json!(id));
        }
        "enable_rule" => {
            let name = str_arg(args.as_object().unwrap(), "name");
            if !ctx.mem.set_rule_enabled(&name, true) {
                return Err(format!("no rule named {name}"));
            }
            ctx.bus.emit(ctx.mem, "rule.enabled", json!({"rule": name}));
            return Ok(json!(true));
        }
        "store" => {
            let a = args.as_object().unwrap();
            let typ = str_arg(a, "type");
            // Provenance and system records come only from the core's own paths (email ingestion, recorded web facts,
            // agreements, habits...). Otherwise a plan could store a "fact from dana@..., sender verified" and cite it.
            const SYSTEM_TYPES: &[&str] = &["agreement", "habit", "job", "thread", "message", "sent_message", "system"];
            const PROVENANCE: &[&str] = &["source", "trust", "superseded_by", "confirmed_by_human", "reliability", "checks"];
            if SYSTEM_TYPES.contains(&typ.as_str()) {
                return Err(format!("'{typ}' records are kept by the core itself; store can't write them"));
            }
            if let Some(k) = a.get("fields").and_then(Value::as_object).and_then(|f| f.keys().find(|k| PROVENANCE.contains(&k.as_str()))) {
                return Err(format!("store can't set '{k}': where a record came from is recorded by the core, not written by a plan"));
            }
            let handle = a.get("handle").map(display);
            let id = ctx.mem.put_object(&typ, handle.as_deref(), a.get("fields").unwrap_or(&json!({})));
            ctx.bus.emit(ctx.mem, "memory.stored", json!({"id": id, "type": typ}));
            return Ok(json!(id));
        }
        _ => {}
    }
    // Idempotency: a step that already completed (e.g. before a crash) is not repeated.
    let key = format!("{}:{}", exec.id, exec.counter);
    if let Some((status, result)) = ctx.mem.journal_status(&key) {
        match status.as_str() {
            "done" => return Ok(serde_json::from_str(&result).unwrap_or(Value::Null)),
            "started" if spec.effect >= Effect::External => {
                return Err(format!("unsure whether '{verb}' completed before a restart; not repeating it"))
            }
            _ => {}
        }
    }
    let text = verbs::explain(spec, &args);
    let jid = ctx.mem.journal_begin(&exec.id, &key, verb, &args, &text);
    ctx.bus.emit(ctx.mem, "action.started", json!({"plan": exec.name, "plan_id": exec.id, "explain": text}));
    // The calendar is the core's own; everything else goes to the organs. Both are journaled, so nothing repeats.
    let result = match verb {
        "schedule" => crate::calendar::add(ctx.mem, ctx.bus, &args, "ai"),
        "unschedule" => crate::calendar::cancel(ctx.mem, ctx.bus, args["id"].as_str().unwrap_or("")),
        "reschedule" => crate::calendar::change(ctx.mem, ctx.bus, args["id"].as_str().unwrap_or(""), &args),
        "answer_invite" => crate::calendar::answer_invite(ctx.mem, ctx.bus, args["id"].as_str().unwrap_or(""), args["answer"].as_str().unwrap_or(""), "ai"),
        // Rehearsal records it without sending, like every other action.
        "send_email" if std::env::var("LINUXAI_REHEARSAL").map(|v| v == "1").unwrap_or(false) =>
            Ok(json!({"rehearsal": true, "verb": verb, "args": args})),
        "send_email" => crate::send::SmtpConfig::from_env().and_then(|cfg| crate::send::send(ctx.mem, &cfg, &args)).inspect(|v| {
            ctx.bus.emit(ctx.mem, "message.sent", json!({"id": v["id"], "to": v["to"], "subject": args["subject"],
                                                          "in_reply_to": args["in_reply_to"], "plan": exec.name}));
        }),
        _ => ctx.organs.invoke(verb, &args),
    };
    match result {
        Ok(v) => {
            if let Some(job) = v["job"].as_str().filter(|_| verb == "run" && args["background"] == true) {
                // Kept on record, so a restart while it runs is noticed and reported, not silently lost.
                ctx.mem.put_object_with_id(&format!("obj:job/{job}"), "job", &json!({"job": job, "status": "running",
                    "program": args["program"], "args": args["args"], "plan": exec.name, "plan_id": exec.id, "started": now()}));
            }
            ctx.mem.journal_end(jid, "done", &v.to_string());
            ctx.bus.emit(ctx.mem, "action.done", json!({"plan": exec.name, "plan_id": exec.id, "explain": text, "result": v}));
            Ok(v)
        }
        Err(e) => {
            ctx.mem.journal_end(jid, "failed", &e);
            ctx.bus.emit(ctx.mem, "action.failed", json!({"plan": exec.name, "plan_id": exec.id, "explain": text, "error": e}));
            Err(format!("{text}: {e}"))
        }
    }
}

/// One line describing an outside fact for the human, from the core's own record of it.
pub fn describe_fact(f: &Value) -> String {
    let typ = f["type"].as_str().unwrap_or("fact");
    let src = &f["source"];
    let from = src["from"].as_str().or(f["from"].as_str()).unwrap_or("unknown sender");
    let auth = match src["authenticated"].as_bool() {
        Some(true) => "sender verified",
        Some(false) => "sender NOT verified",
        None => "unverified",
    };
    let skip = ["id", "type", "source", "trust", "created", "handle", "category", "checks", "reliability", "corrects", "status"];
    let detail: Vec<String> = f.as_object().map(|m| m.iter().filter(|(k, v)| !skip.contains(&k.as_str()) && !v.is_null() && !v.is_object() && !v.is_array())
        .take(6).map(|(k, v)| format!("{k}: {}", display(v))).collect()).unwrap_or_default();
    let mut line = format!("{typ} from {from} ({auth}): {}", detail.join(", "));
    if let Some(rec) = f["reliability"]["track_record"].as_str() {
        line.push_str(&format!(" [sender's record: {rec}]"));
    }
    if let Some(st) = f["status"].as_str() {
        line.push_str(&format!(" [{st}]"));
    }
    if let Some(c) = f["corrects"].as_str() {
        line.push_str(&format!(" [corrects {c}]"));
    }
    let checks: Vec<String> = f["checks"].as_array().map(|a| a.iter().map(display).collect()).unwrap_or_default();
    if !checks.is_empty() {
        line.push_str(&format!(" ⚠ {}", checks.join("; ")));
    }
    line
}
