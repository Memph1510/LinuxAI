//! Verb registry: every action the AI can take, its arguments, its kind of effect, and how to explain it.

use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Effect {
    /// Changes nothing outside the AI's own memory (tell, remember).
    Internal,
    /// Changes this machine (power, services, programs).
    Local,
    /// Visible to the outside world (sending, booking).
    External,
    /// Costs money (payments organ, after v0.1).
    #[allow(dead_code)]
    Money,
}

impl Effect {
    pub fn name(self) -> &'static str {
        match self {
            Effect::Internal => "internal",
            Effect::Local => "local",
            Effect::External => "external",
            Effect::Money => "money",
        }
    }
}

pub struct VerbSpec {
    pub name: &'static str,
    pub organ: &'static str,
    /// (argument, required)
    pub args: &'static [(&'static str, bool)],
    pub effect: Effect,
    /// Which argument names the thing acted on (used for trust/mandates).
    pub target: &'static str,
    pub explain: &'static str,
    pub doc: &'static str,
}

pub const VERBS: &[VerbSpec] = &[
    VerbSpec { name: "tell", organ: "core", args: &[("text", true)], effect: Effect::Internal, target: "",
        explain: "tell you: {text}", doc: "Send the human a message (shown in the session, later by text)." },
    VerbSpec { name: "remember", organ: "core", args: &[("text", true), ("kind", false)], effect: Effect::Internal,
        target: "", explain: "remember: {text}", doc: "Store a note in the AI's own memory." },
    VerbSpec { name: "track", organ: "core",
        args: &[("thread", true), ("with", false), ("about", false), ("state", false), ("waiting_for", false), ("status", false)],
        effect: Effect::Internal, target: "thread", explain: "note where {thread} stands: {state}",
        doc: "Keep the state of an ongoing exchange (a negotiation, a back-and-forth) in one place, so the next reply is \
              understood without rereading anything. thread: a stable key (\"load:L-88213\"); with: who; about: what; \
              state: where it stands now (\"we countered $3,500; their offer $3,000\"); waiting_for: what we expect next; \
              status: open (default) or closed. Fields not given are kept." },
    VerbSpec { name: "agree", organ: "core",
        args: &[("reference", true), ("terms", true), ("with", false), ("about", false), ("thread", false)],
        effect: Effect::Internal, target: "reference", explain: "note what was agreed on {reference}: {terms}",
        doc: "Record what was agreed (a rate, an amount, dates), so documents that come in later (a rate confirmation, \
              an invoice) are checked against it by the core: a match is filed without you, only a difference is \
              brought up. reference: what the documents will cite (\"HL-3100\"); terms: {rate_cents: 320000, ...} \
              using the same field names as email claims." },
    VerbSpec { name: "browse", organ: "browser",
        args: &[("site", true), ("op", true), ("tab", false), ("url", false), ("ref", false), ("target", false), ("selector", false),
                ("text", false), ("enter", false), ("key", false)],
        effect: Effect::External, target: "site", explain: "{op} on {site}",
        doc: "Act on a web page where it reaches the site: click, type or a key in your own tab, or open an address that \
              carries data (a query or fragment) while working on outside information. The first time per site needs \
              the human's OK; if they say always, the browser tool acts on that site directly after. site: the page's \
              origin (\"https://dat.com\"); op: click | type | key | go | open; the rest as in the browser tool." },
    VerbSpec { name: "send_email", organ: "mail",
        args: &[("to", true), ("subject", true), ("text", true), ("in_reply_to", false)],
        effect: Effect::External, target: "to", explain: "email {to}: \"{subject}\"\n{text}",
        doc: "Send an email (plain text). to: one address or a list; in_reply_to: the id of a stored message, so it \
              threads under it. It acts on the outside world: the human is asked first (with the whole message in \
              front of them) until they answer \"always\" for that recipient. Every message sent is kept." },
    VerbSpec { name: "store", organ: "core", args: &[("type", true), ("fields", true), ("handle", false)],
        effect: Effect::Internal, target: "type", explain: "store a {type} in memory",
        doc: "Create a memory object of a type (bill, person, preference, ...) with fields." },
    VerbSpec { name: "schedule", organ: "core",
        args: &[("title", true), ("at", true), ("minutes", false), ("end", false), ("where", false), ("notes", false), ("rrule", false)],
        effect: Effect::Local, target: "title", explain: "put \"{title}\" on your calendar at {at}",
        doc: "Add an event to the human's calendar. at: \"2026-06-12 08:05\" (local) or Unix seconds; minutes (default 60) \
              or end; rrule for repeating (e.g. FREQ=WEEKLY;BYDAY=MO,WE). Returns its id and any overlaps." },
    VerbSpec { name: "unschedule", organ: "core", args: &[("id", true)], effect: Effect::Local, target: "id",
        explain: "take {id} off your calendar", doc: "Cancel an event on the human's calendar (not subscribed ones)." },
    VerbSpec { name: "reschedule", organ: "core", args: &[("id", true), ("at", false), ("end", false), ("title", false), ("where", false), ("notes", false)],
        effect: Effect::Local, target: "id", explain: "change {id} on your calendar", doc: "Move or edit an event on the human's calendar." },
    VerbSpec { name: "answer_invite", organ: "core", args: &[("id", true), ("answer", true)], effect: Effect::Local, target: "id",
        explain: "answer the invitation {id}: {answer}",
        doc: "Answer a pending invitation: accept (puts it on the calendar), decline, or tentative. The invitation itself \
              is outside information; decide by the calendar playbook or ask the human." },
    VerbSpec { name: "enable_rule", organ: "core", args: &[("name", true)], effect: Effect::Local, target: "name",
        explain: "turn on the automatic rule {name}", doc: "Turn on a rule (reflex) so it runs by itself from now on." },
    VerbSpec { name: "set", organ: "host", args: &[("what", true), ("to", true)], effect: Effect::Local,
        target: "what", explain: "set {what} to {to}",
        doc: "Set a machine control. what: power.profile (power-saver|balanced|performance), \
              cpu.policy (off|balanced|efficiency|responsive|gaming: sched_ext scheduler policy), \
              cpu.governor (powersave|schedutil|performance|...), screen.brightness (0-100), sysctl.<key>." },
    VerbSpec { name: "limit", organ: "host",
        args: &[("unit", true), ("cpu_weight", false), ("memory_high", false), ("memory_max", false), ("io_weight", false)],
        effect: Effect::Local, target: "unit", explain: "limit {unit} resources",
        doc: "Set cgroup resource shares/limits for a systemd unit or slice (e.g. user.slice, app-steam.scope). \
              cpu_weight/io_weight 1-10000 (default 100); memory_high/memory_max like \"4G\"." },
    VerbSpec { name: "start", organ: "host", args: &[("unit", true)], effect: Effect::Local, target: "unit",
        explain: "start {unit}", doc: "Start a systemd unit." },
    VerbSpec { name: "stop", organ: "host", args: &[("unit", true)], effect: Effect::Local, target: "unit",
        explain: "stop {unit}", doc: "Stop a systemd unit." },
    VerbSpec { name: "restart", organ: "host", args: &[("unit", true)], effect: Effect::Local, target: "unit",
        explain: "restart {unit}", doc: "Restart a systemd unit." },
    VerbSpec { name: "run", organ: "programs", args: &[("program", true), ("args", false), ("timeout", false), ("background", false)],
        effect: Effect::Local, target: "program", explain: "run {program}",
        doc: "Escape hatch: run any program with an argument list (no shell unless program is \"sh\"). \
              Returns {code, output}. Long work (updates, builds, renders, downloads): background: true returns {job} \
              at once; wait for job.finished where its job matches, and branch on its ok, code and output." },
];

pub fn find(name: &str) -> Option<&'static VerbSpec> {
    VERBS.iter().find(|v| v.name == name)
}

/// Machine-readable registry for the model's instructions.
pub fn registry_json() -> Value {
    Value::Array(
        VERBS
            .iter()
            .map(|v| {
                json!({"verb": v.name, "organ": v.organ, "effect": v.effect.name(), "doc": v.doc,
                       "args": v.args.iter().map(|(a, req)| json!({"name": a, "required": req})).collect::<Vec<_>>()})
            })
            .collect(),
    )
}

/// Fill an explain template with argument values.
pub fn explain(spec: &VerbSpec, args: &Value) -> String {
    let mut out = spec.explain.to_string();
    if let Value::Object(m) = args {
        for (k, v) in m {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&format!("{{{k}}}"), &s);
        }
    }
    out
}

/// Organs carry out verbs. The core handles `core` verbs itself; everything else goes through this.
pub trait Organs: Send + Sync {
    fn invoke(&self, verb: &str, args: &Value) -> Result<Value, String>;
}
