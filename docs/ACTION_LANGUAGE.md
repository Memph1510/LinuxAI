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
| expressions | prefix lists: `[">", "$a", 10]`, `["*", "$usual", 1.2]`, `["avg", "amount", <list>]` |

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
| `wait` | Wait for an event (`{"wait": "payment.settled", "about": "$bill", "max": "3d"}`) or a time |
| `think` | Hand back to the AI model with the current state: "something unexpected happened, decide what to do next". This is how a plan asks the AI for judgment only when it's needed. |
| `call` | Run a saved plan or solution by name, with arguments |

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
