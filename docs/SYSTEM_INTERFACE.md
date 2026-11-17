# LinuxAI System Interface: Draft 1

> Status: **first draft for discussion.** Every name and shape here can still change.
> Companion document: [ACTION_LANGUAGE.md](ACTION_LANGUAGE.md).

## In plain terms

A normal operating system is built on three ideas: **files** (data in folders), **programs** (apps that own their
own data), and **commands** (a human telling the computer exactly what to do).

LinuxAI replaces them with four ideas:

| Normal OS | LinuxAI | What it means |
|---|---|---|
| Files and folders | **Objects** | Everything the system knows (a person, an email, a bill, a trip, a project, a photo) is one object in a single connected memory. Each object knows how it relates to the others, where it came from, and why it matters. |
| Notifications, logs | **Events** | Anything that *happens* (a message arrives, the battery runs low, a bill is due, you say something) is an event. The AI notices events and decides what to do. |
| Commands, scripts | **Actions** | Everything the AI *does* is an action written in the LinuxAI action language. It is checked before it runs, carried out exactly, and recorded so you can read it, question it or undo it. |
| Apps | **Views** | Email, calendar, files and photos are not apps that own data. They are *ways of looking* at the one memory. The AI can also make a new view whenever you need one ("show me everything about the Denver trip"). |

Two more pieces connect these four to you and to the world:

- **Mandates and budgets.** Standing directions you give ("handle my bills", "trips under $900"). Inside a
  mandate the AI acts. Outside it, or when it isn't confident, it asks you.
- **Organs.** The parts that touch the outside world and the hardware: the kernel (CPU, memory, power, devices),
  email, text messages, payments, the web. Each organ adds new things the AI can do (actions) and new things it can
  notice (events).

```
                 you ── talk / look ──┐
                                      ▼
   ┌───────────────────────── LinuxAI core (the mind) ──────────────────────────┐
   │  events ──▶ attention ──▶ think / reflex ──▶ plan (action language)        │
   │                                    │                │                      │
   │                     memory (objects) ◀── learn ── check ▶ run ▶ journal    │
   │                                                     │                      │
   │                               mandates & budgets ───┘                      │
   └───────────────▲─────────────────────────────────────────┬──────────────────┘
                   │ events                          actions │
   ┌───────────────┴─────────────────────────────────────────▼──────────────────┐
   │  organs:  host (kernel/drivers) · mail · sms · payments · web · programs   │
   └────────────────────────────────────────────────────────────────────────────┘
```

---

## 1. Design rules

1. **One memory.** No part of the system owns its own private data. Email, texts, calendar and bills are all
   objects in the same memory, linked together.
2. **Everything changes through an event or an action.** Nothing changes silently. That gives you a complete,
   readable history of the machine and of the AI's work.
3. **The AI acts only through the action language.** It does not run raw shell commands as its normal way of
   working. Programs and shell commands are still available, as one organ among others, for jobs no verb covers yet.
4. **Readable by the human.** Every object, event, action and plan can be shown in plain language.
5. **Kernel-independent.** The interface is a protocol (messages between the core and the organs). Linux is
   simply the first *host* that implements the kernel organ. A LinuxAI-native kernel could implement the same
   protocol later.
6. **Durable.** Plans can pause for days (waiting for your reply or for a date) and pick up where they left off,
   even after a reboot.

---

## 2. Objects (memory)

### 2.1 Shape of an object

```yaml
id:        obj:bill/01JDQ8K3T7M2   # type + unique, time-sortable id
type:      bill
handle:    "Verizon, Nov 2026"     # human-friendly name
fields:                            # typed, depending on the object type
  amount:  $112.40
  due:     2026-11-28
  status:  unpaid
links:                             # typed connections to other objects
  - from:      obj:org/verizon
  - arrived_in: obj:message/01JDQ8K2...   # the email it came in
  - for:       obj:account/verizon-wireless
body:      blob:sha256:9f2c...     # optional content (text, PDF, image), stored once by hash
source:    imported:mail           # where it came from: imported | organ:<name> | human | ai
sensitivity: private               # normal | private | secret (secret = AI sees a handle, never the value)
history:   [v1, v2, ...]           # every change is a new version; nothing is overwritten
```

- **Ids** are `obj:<type>/<ULID>`. ULIDs sort by time, so ids double as rough timestamps.
- **Handles** let you and the AI refer to objects naturally: `@verizon`, `@mom`, `#denver-trip`.
- **Links** are what turn separate data into understanding. The bill links to the email it arrived in, the
  company, the account, the payment that settled it, and the budget it came out of.
- **History:** objects are append-only and versioned. "What did this look like last week" always has an answer.

### 2.2 Core object types (first set)

| Group | Types |
|---|---|
| People | `person`, `org`, `account` (a login or service relationship), `place` |
| Communication | `message` (email, SMS and chat are one type with a `channel` field), `conversation` |
| Time | `appointment` (calendar entries), `reminder` |
| Work | `project`, `task`, `document`, `media` (photos, video, audio) |
| Money | `budget`, `bill`, `transaction`, `card` |
| Plans | `trip`, `booking`, `order` |
| Machine | `device`, `program`, `service`, `network` |
| Understanding | `belief`, `preference`, `solution`, `note` |
| Directions | `mandate`, `intent` |

New types can be added. A type is a small schema (fields, allowed links, how to show it in one line).

### 2.3 Understanding: beliefs and preferences

The AI's picture of you is stored as objects too, so you can see it and correct it:

```yaml
type: preference
handle: "Prefers aisle seats on flights"
fields: { confidence: 0.85 }
links:
  - evidence: obj:booking/01J...    # three past bookings chose aisle
  - evidence: obj:message/01J...    # "always get me an aisle seat"
```

When you correct one ("actually window on night flights"), that becomes new evidence and a new version. This is
how "it knows which airlines we use and why we're going" works in practice: through linked, inspectable beliefs,
not hidden model weights.

### 2.4 Finding things

Memory is searched in three ways at once, using the same `find` that the action language uses:

- **by structure:** `find bill where from = @verizon and date > 6 months ago`
- **by meaning:** `find anything about "the wedding in Denver"` (semantic search over text and media)
- **by connection:** `find people linked to #denver-trip`

### 2.5 Legacy files

Ordinary programs still expect files. A `document` or `media` object can be **materialized** as a normal file
when a program needs it, and changes to that file flow back into the object as a new version. Files become a
compatibility layer, not the way the system is organized.

---

## 3. Events

### 3.1 Shape of an event

```yaml
id:      evt:01JDQ8K2Z1
time:    2026-11-17T09:14:03Z
kind:    message.received           # dotted name: area.what
subject: obj:message/01JDQ8K2...    # the object it is about (if any)
data:    { channel: email, from: obj:org/verizon }
source:  organ:mail
```

### 3.2 Where events come from

| Source | Examples |
|---|---|
| **Host (kernel)** | `power.unplugged`, `battery.low`, `cpu.pressure`, `memory.pressure`, `device.added`, `network.changed`, `service.failed`, `temperature.high` |
| **Organs** | `message.received`, `bill.arrived`, `payment.settled`, `booking.changed`, `price.dropped` |
| **Time** | `time.reached` (dates, deadlines, schedules) |
| **Human** | `human.said` (anything you type or text), `human.did` (something you did in a view), `human.corrected` |
| **The AI itself** | `plan.finished`, `plan.failed`, `plan.waiting` |

### 3.3 The event log

Every event is kept, in order, forever (subject to your retention settings). It is the system's **episodic
memory** ("what happened last Tuesday") and, together with the action journal, its complete history.

### 3.4 Reacting to events

Reactions are written in the action language as **rules** (`when … do …`, see the language document). Rules
cover what the system has learned to handle on its own. Everything else goes to the AI to think about.

---

## 4. Actions, mandates and budgets

### 4.1 Verbs

Each organ provides **verbs**: the things the AI can do. Every verb declares:

```yaml
verb:       pay
organ:      payments
takes:      { what: bill | amount: money, from: budget, using?: card }
effects:    external, costs-money        # what kind of change this is
reversible: no                           # some verbs declare an undo (e.g. move ↔ move back)
reads_as:   "pay {what} from {from}"     # how it is shown to you in plain language
```

Effect kinds: `none` (read-only), `local` (changes memory or the machine), `external` (the outside world sees
it: sending, booking), `costs-money`.

### 4.2 The life of an action

1. **Plan.** The AI (or a rule) writes a plan in the action language.
2. **Check.** Before anything runs, the core checks the plan: references exist, values have the right types,
   verbs exist, and the plan fits a mandate and a budget. The check also lists which steps are external or cost
   money.
3. **Decide.** If the plan fits a mandate and the AI is confident, it runs. Otherwise the plan pauses at an
   `ask` and you get a message.
4. **Run.** Steps run in order. Each external step gets an **idempotency key**, so a crash or reboot mid-plan
   never pays a bill twice.
5. **Journal.** Every step is recorded: what was intended, why, what happened, what it cost, and how to undo it
   if it can be undone.
6. **Learn.** Results feed back into memory: new beliefs, solutions and preferences.

### 4.3 Mandates: your standing directions

A mandate is how you say "this is yours to handle". You can say it in normal words; the AI writes it in the
action language and shows you the result to confirm.

```
mandate "Bills"
  allow pay for bill from @verizon, @xfinity, @state-farm
  within budget:bills
  when amount is within 20% of usual
```

```
mandate "Travel"
  allow search, hold, book for flight, hotel
  within budget:travel, max $900 per trip
  prefer airline: @southwest, seat: aisle
```

Anything a plan does outside every mandate becomes a question to you. You can answer with a one-time yes, or
with "yes, and from now on…", which updates the mandate.

### 4.4 Budgets

`budget` is an object type: `{ limit: $400, period: month | week | task, spent: $212.18, category: travel }`.
Every verb that costs money is charged to a budget at check time (estimated) and at run time (actual). Budgets
are how "monthly, weekly or per task" works.

### 4.5 Confidence

Every plan carries the AI's own **confidence** (0–1) and a one-line **reason**. Below your threshold (for
example 0.7), it asks even inside a mandate. Early on, confidence will be low and it will ask a lot. As beliefs
and solutions build up, it asks less. This is the "less waiting, more understanding" you described, and it is
earned rather than switched on.

### 4.6 Anomalies

Every verb that touches money or the outside world compares the request with what's normal (the bill's history,
usual prices, usual recipients). Something unusual becomes a note in the plan ("33% above the usual Verizon
bill"). It lowers confidence, so the AI asks unless a mandate says otherwise.

---

## 5. Views

### 5.1 What a view is

A view is a **query plus a layout**, kept live:

```yaml
view:   inbox
show:   find message where needs_reply or important, newest first
layout: list
item:   "{from} · {subject} · {time ago}"
actions: [reply, archive, "ask AI about this"]
```

| What you'd call an app | As a LinuxAI view |
|---|---|
| Email | messages with `channel: email`, as a list or conversation |
| Calendar | appointments, plans and deadlines on a time grid |
| Files | documents and projects, by project, time or meaning instead of folders |
| Photos | media, by people, places and trips |
| Activity | the live journal: what the AI is doing right now, and why |
| Money | budgets, bills and transactions, with anomalies highlighted |

### 5.2 Views the AI makes

Because a view is just a description, the AI can create one on the spot ("everything about the Denver trip"
gives you flights, hotel, the wedding invite, the people going, the budget, and open questions) and throw it away
afterwards.

### 5.3 Acting inside a view

When you do something in a view (archive an email, drag an appointment), it becomes a `human.did` event. The AI
sees it, learns from it, and stays in sync. You and the AI work on the same memory.

### 5.4 Rendering

Views are declarative descriptions. The **shell** (the login screen and everything you see) draws them. The phone
app draws the same views in its own way. On Linux the shell starts as a single full-screen program on a minimal
Wayland compositor. How it looks is a design discussion still to come.

---

## 6. Organs

An organ is a separate process that speaks the interface protocol. It declares:

```yaml
organ:  mail
types:  [message, conversation, person]       # object types it fills in
events: [message.received, message.sent]
verbs:  [send, reply, archive, label]
needs:  { network: [imap.example.com, smtp.example.com], secrets: [mail-password] }
```

First organs:

| Organ | Purpose |
|---|---|
| **host** | The kernel and hardware: CPU scheduling policy (sched_ext), cgroup CPU/memory/I/O shares, power profiles, devices, network, services, processes. On Linux it uses systemd, cgroups, sched_ext and udev. |
| **programs** | Runs ordinary programs and shell commands, for anything no verb covers yet. This is where the `cortex` prototype's abilities live. |
| **mail** | IMAP/SMTP mailbox (the new address with your imported history) |
| **sms** | The Android app: texting with the AI, later your SMS |
| **payments** | Cards and virtual cards, bill pay |
| **web** | A browser the AI can drive, for booking and accounts with no API |
| **model** | The AI models themselves: Claude for hard thinking, a cheap model for bulk work, local models later |

Secrets (passwords, card numbers) belong to organs. The AI refers to them by handle and never sees the values,
so they can't leak into a conversation or a log.

---

## 7. The core loop

```
event arrives
  │
  ├─ a rule matches? ──yes──▶ run the rule's plan (no model needed, instant)
  │
  └─ no ──▶ attention: does this matter now?  (cheap model or heuristics)
              │
              └─ yes ──▶ think (Claude): gather memory → write a plan → check
                                        │
                     fits mandate + confident? ──yes──▶ run → journal → learn
                                        │
                                        no ──▶ ask you (text / screen), wait, continue
```

**Learning to act on its own:** when the AI solves the same kind of situation repeatedly, it proposes a **rule**
(a `when … do …` plan). Once you accept it, that situation is handled instantly and for free from then on. Over
time more and more of life runs on rules the AI wrote and you approved, and the model handles what's new.

---

## 8. Protocol (kernel independence)

- The core and every organ talk over one **message protocol**: request/response plus event streams, on local
  sockets. CBOR on the wire, and JSON for debugging.
- Message kinds: `declare` (organ → core: types, verbs, events), `event` (organ → core), `invoke`
  (core → organ: run a verb, with an idempotency key), `result`, `query` (memory), `subscribe` (views).
- A **host** is the organ that owns the hardware. The Linux host is a set of system services. A future
  LinuxAI-native kernel would be a new host implementing the same messages. Nothing above it would change.

---

## 9. How this maps onto v0.1 (it works as an OS)

| v0.1 piece | Interface pieces it exercises |
|---|---|
| Boot into the LinuxAI session | shell + activity view |
| AI manages CPU, memory, power, devices, services | host organ (verbs + events) + rules |
| Learns, asks when unsure | beliefs, solutions, `ask`, confidence |
| Works offline | rules run without the model; host keeps the last policy |
| Can still do anything Linux can | programs organ |

Email, SMS, payments and web come after v0.1. They are already in the interface, so adding them doesn't change
the foundation.

---

## 10. Implementation choices (proposed)

- **Core and language runtime in Rust.** It is long-running, needs to be reliable and fast, and could later run
  on a kernel other than Linux. Existing Python code (`cortex`) stays useful as the programs organ and for quick
  experiments.
- **Memory store:** SQLite for objects, links, versions, events and journal, plus a content-addressed blob store.
  Semantic search is added as an index, not as a separate database.
- **How the model writes plans:** the language reference sits in the model's (cached) instructions. The model
  submits a plan through one tool. The checker returns errors the model fixes before anything runs.

## 11. Questions to settle together

1. Do the four ideas (objects, events, actions, views) match how you picture it? Is anything missing?
2. Mandates: do you want to set them by talking ("handle my bills") and confirming what the AI writes, or edit
   them directly?
3. What confidence level should mean "go ahead" when the AI is inside a mandate? (Proposal: start at 0.8 and lower
   it as trust grows.)
4. Which views matter most on day one? (Proposal: activity, conversation with the AI, then everything else.)
5. Should the history of everything be kept forever, or should you be able to set retention (e.g. delete raw
   messages after 2 years and keep what was learned from them)?
