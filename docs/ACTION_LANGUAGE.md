# LinuxAI Action Language: Draft 2

> Status: **draft for discussion.** Name: **LinuxAI** (the language shares the system's name).
> Companion document: [SYSTEM_INTERFACE.md](SYSTEM_INTERFACE.md).

## In plain terms

There are two languages in LinuxAI, and they have different jobs:

| | Between you and the AI | Between the AI and the computer |
|---|---|---|
| Language | **English**, by text or voice, like a normal conversation | **The LinuxAI action language** (this document) |
| Built for | You | The AI and the machine, for speed, precision and reliability |
| Do you see it? | Yes, it's how you talk | Only if you ask. The system explains any of it in English on request ("what did you just do?", "why?"). |

This document is about the second one. You never have to read or write it.

## Design goal: as efficient as possible for the AI

"Efficient" here means four things.

1. **Few round trips.** Today's AI agents (including the `cortex` prototype) think, run one command, read the
   output, think again, run the next command. Every step costs a full model call: seconds and money. In LinuxAI
   the AI writes a **whole plan at once**, with branches for the likely outcomes ("if the bill is unusually high,
   ask; otherwise pay"). The machine then runs it at machine speed. The AI is only woken up again when something
   happens that the plan didn't cover.
2. **No mistakes in form.** AI models are most reliable when they produce **structured data that matches a strict
   schema**. The API enforces the shape, so a plan can never be malformed. Inventing a brand-new text syntax would
   cost tokens to teach and cause errors. So the language is a **typed data structure** (JSON), not text code.
3. **Few tokens.** Plans refer to things by short handles and ids (`@verizon`, `budget:bills`), not prose. Plans
   that worked are saved and reused by name, so the AI doesn't rewrite them every time. Keys stay short but
   meaningful: cryptic encodings save little and make the model err more.
4. **Free when possible.** Plans the AI has learned become **rules** that run with no AI model at all: instantly,
   offline, at no cost. Over time most routine work runs this way.

## What a plan looks like

The AI writes this (through one strictly-typed tool call):

```json
{
  "plan": "pay-verizon",
  "why": "bill arrived by email; due 2026-11-28",
  "conf": 0.9,
  "steps": [
    {"let": "bill",  "find": {"type": "bill", "from": "@verizon", "where": {"status": "unpaid"}, "pick": "newest"}},
    {"let": "usual", "calc": ["avg", "amount", {"find": {"type": "bill", "from": "@verizon", "last": 6}}]},
    {"if": [">", "$bill.amount", ["*", "$usual", 1.2]],
     "then": [
       {"ask": "Verizon is {$bill.amount}, about {pct($bill.amount,$usual)} above the usual {$usual}. Pay it?",
        "opts": ["pay", "hold"], "as": "ans"},
       {"expect": ["=", "$ans", "pay"], "else": "stop"}
     ]},
    {"do": "pay", "what": "$bill", "from": "budget:bills"},
    {"do": "remember", "text": "Paid Verizon {$bill.amount} on {today}"}
  ]
}
```

If you ask what it did, you get:

> I paid the Verizon bill ($84.12) from the bills budget. It was in the normal range, so I didn't need to check
> with you.

## 1. Values

| Kind | Form |
|---|---|
| text, number, yes/no | JSON `"…"`, `3.5`, `true` |
| money | `{"$": 84.12}` or `"$84.12"`, with currency (USD by default) |
| time | ISO dates/times, or relative forms the engine resolves at check time: `"today"`, `"+3d"`, `"-6mo"`, `"next fri 9:00"` |
| references | `"@verizon"` (handle), `"#denver-trip"` (tag/project), `"budget:bills"`, `"obj:bill/01JDQ8K3..."` |
| variables | `"$name"`, `"$name.field"` |
| expressions | prefix lists: `[">", "$a", 10]`, `["*", "$usual", 1.2]`, `["avg", "amount", <list>]`, `["get", obj, "field"]`, `["pick", list, "field", value]` (first item whose field equals value) |

## 2. Step kinds

| Step | Purpose |
|---|---|
| `let` + `find` / `calc` / value | Look things up in memory, compute, name the result |
| `do` | Use a **verb** (an action an organ provides): `pay`, `send`, `book`, `schedule`, `set`, `limit`, `restart`, `install`, `run`, `remember`, `believe`, … |
| `if` / `then` / `else` | Branch |
| `each` | Repeat over a list (always finite) |
| `expect` + `else` | Check a condition; on failure `stop`, `ask`, or `think` (see below) |
| `try` / `else` | Handle a failing step |
| `ask` | Ask you something. The plan **pauses durably**, through days and reboots, until you answer by text or on screen. |
| `tell` | Send you a message and continue |
| `wait` | Wait for an event (`{"wait": "payment.settled", "about": "$bill", "max": "3d"}`) or a time. `where` takes only the event that matches, checked with the plan's variables and `$event` (a reply from this broker about this load); `about` says in words what's expected; `thread` links it to an exchange kept with `track`. A plan that waits for a message rests on outside information from the start. |
| `think` | Hand back to the AI model with the current state: "something unexpected happened, decide what to do next". This is how a plan asks the AI for judgment only when it's needed. |
| `call` | Run a saved plan or solution by name, with arguments |

### Exchanges, decided ahead

When the AI sends something that expects an answer (a counter-offer with `send_email`, a question to a broker), it
writes the follow-up at the same time: keep the state with `track`, wait for exactly that reply, and decide now
what each foreseeable answer means. The reply is then handled when it arrives, with no rereading and no model:

```json
{"plan": "negotiate HL-3100", "steps": [
  {"do": "track", "thread": "load:HL-3100", "with": "dana@hollis-logistics.com",
   "state": "they offered $3,000; we countered $3,500", "waiting_for": "their answer to $3,500"},
  {"wait": "message.received", "where": ["exists", ["pick", "$event.data.claims", "reference", "HL-3100"]],
   "thread": "load:HL-3100", "about": "Dana's answer to our $3,500 counter", "max": "2d"},
  {"let": "usd", "calc": ["/", ["get", ["pick", "$event.data.claims", "reference", "HL-3100"], "rate_cents"], 100]},
  {"if": [">=", "$usd", 3500], "then": [{"do": "track", "thread": "load:HL-3100", "state": "agreed at $3,500", "status": "closed"}],
   "else": [{"if": [">=", "$usd", 3100],
     "then": [{"ask": "Hollis came back at ${$usd} (they offered $3,000, we asked $3,500). Take it?",
               "opts": ["take", "counter", "pass"], "lean": "take"}],
     "else": [{"think": "Hollis answered at ${$usd}, below what I planned for. They offered $3,000; we countered $3,500."}]}]}]}
```

- An email's event carries the values it claims inline (`$event.data.claims`), so a plan can branch on "$3,200"
  without reading anything.
- The event is marked `expected_by` the waiting plan, so the AI isn't woken for it. A reply the plan didn't
  foresee goes to `think`, and the note carries the state.
- `{"op": "threads"}` lists open exchanges with where each stands and which plan handles the next reply; the AI
  sees them in its context.

### Long work, decided ahead

Updates, builds, renders and big downloads run in the background, and the plan decides now what each outcome
means. Nobody checks on the job while it runs, and the model isn't involved unless the outcome is a surprise:

```json
{"plan": "system update", "steps": [
  {"do": "run", "program": "pacman", "args": ["-Syu", "--noconfirm"], "background": true, "as": "job"},
  {"wait": "job.finished", "where": ["=", "$event.data.job", "$job.job"], "max": "6h", "about": "the system update"},
  {"if": "$event.data.ok", "then": [{"tell": "Updated: {$event.data.output}"}],
   "else": [{"if": ["has", "$event.data.output", "conflicting files"],
     "then": [{"call": "fix pacman file conflicts"}],
     "else": [{"think": "The update failed in a way I didn't plan for: {$event.data.output}"}]}]}]}
```

`job.finished` carries the job's id, program, exit code, `ok`, the end of its output and how long it took. If the
core restarts while a job runs, its result is lost; the wait's `max` covers that.

### What was agreed, checked when documents come in

When a deal is settled, the plan records it: `{"do": "agree", "reference": "HL-3100", "terms": {"rate_cents":
320000}}`. A rate confirmation or invoice that cites HL-3100 is then compared with it by the core as it arrives:
a match is filed with the agreement and you're told in a line, with no model; only a difference reaches the AI,
spelled out ("rate agreed $3,200, the document says $3,000"). `{"op": "agreements"}` lists them.

## 3. Verbs

Every verb is declared by an organ with a typed signature:

```json
{"verb": "pay", "organ": "payments",
 "args": {"what": "bill|money", "from": "budget", "using": "card?"},
 "effect": ["external", "money"], "undo": null,
 "explain": "pay {what} from {from}"}
```

- `effect` tells the engine what kind of change this is: none, local, external, money.
- `undo` names the reverse verb when there is one.
- `explain` is the template used to describe the step to you in English.

**Escape hatch:** the `run` verb (programs organ) runs any ordinary program or shell command. It stays
**unlimited** for now, so the AI is never stuck while the set of verbs grows. When the AI keeps using `run` for
the same job, that's a signal to add a proper verb.

## 4. Rules

A rule is a plan triggered by an event. It runs without the AI model:

```json
{"rule": "unplugged-save-power",
 "when": "power.unplugged",
 "steps": [
   {"do": "set", "what": "power.profile", "to": "saver"},
   {"do": "set", "what": "cpu.policy",    "to": "efficiency"}
 ]}
```

The AI creates rules from what it has learned. It tells you in English ("From now on, when you unplug, I'll
switch to power-saving automatically"), and you can say no.

## 5. How a plan runs

1. **Receive.** The plan arrives from the model as schema-checked data, so its shape is always valid.
2. **Check.** References resolve, types match, verbs exist, relative times become exact, money is charged against
   budgets, and steps outside what the AI is trusted to do are marked (see "Trust" in SYSTEM_INTERFACE.md).
   Problems go straight back to the model to fix, before anything happens.
3. **Run.** Steps execute at machine speed. Outside-world steps carry idempotency keys, so a crash can't pay or
   send twice. `ask` and `wait` pause the plan durably.
4. **Journal.** Each step is recorded with intent, result, cost and undo. This is what the AI uses to explain
   itself to you in English.
5. **Learn.** Outcomes update memory. Repeated successes become rules.

## 6. Implementation notes

- The plan schema is a JSON Schema, given to the model as a strict tool definition.
- The engine (checker + runner + rule matcher) is written in Rust, inside the LinuxAI core.
- Plans, rules and saved solutions are stored as objects in memory, so they're searchable, versioned and
  reusable like everything else.
- An optional compact text form can come later for debugging. The canonical form is the data structure.

## 7. Open questions

1. First verbs for v0.1. The proposal is the host verbs (power, CPU policy, memory limits, services, devices,
   network, install and update) plus `run`, `ask`, `tell`, `remember`. Anything else?
2. When should a repeated pattern become a rule? Proposal: after the AI has handled the same situation 3 times
   with the same outcome, it offers the rule.
