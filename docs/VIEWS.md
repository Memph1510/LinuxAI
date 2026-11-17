# LinuxAI Views: the Workspace (draft 1)

> Status: **draft for discussion**, based on your mock-up. Clickable prototype: `prototypes/workspace.html`
> (published as a private artifact).

## What it has to do

You see **results, tasks, decisions and the reasoning behind them**. You can click any of them and comment, and a
comment can **change things downstream and upstream** of where you left it.

## The idea: every task is a graph of steps

```
 Intent ──▶ Reasoning ──▶ Decision ──▶ Action ──▶ Result
 (yours,     (facts it     (options      (what it    (what came
  or a rule)  relied on)    weighed,      does)       of it)
                            confidence)
```

- **Intent**: what started it. Your request ("book my flights"), a standing direction ("handle my bills"), or a
  rule.
- **Reasoning**: the facts the AI relied on, each linked to its evidence (the invite email, your past bookings, a
  log line).
- **Decision**: the options it weighed, a confidence for each, and why it chose one.
- **Action**: what it does, in plain English. The LinuxAI action language behind it is one click away, never
  in your face.
- **Result**: what came of it.

The steps are linked. Every step knows which steps it came from (upstream) and which ones depend on it (downstream).

## The screen

| Area | Shows |
|---|---|
| **Top bar** | The machine's vitals (core, power, CPU policy) and how many things need you |
| **Stream** (left) | Every task, with what needs you first, then recent work. Each shows its source (you, an event, a rule) |
| **Graph** (center) | The selected task's steps, laid out left to right in dependency order. Arrows show what led to what, and the selected step's whole chain lights up |
| **Inspector** (right) | The selected step: summary, evidence, options weighed with confidence, what runs behind the scenes, where it came from and what it leads to, earlier versions, and the **discussion** |

On a phone the three areas stack, and the graph scrolls sideways.

## Comments that change things

1. You comment on any step: "I'd rather leave Wednesday", "why not United?", "that's too expensive".
2. The AI works out what kind of comment it is:
   - **question**: it answers from the evidence and changes nothing;
   - **change**: it revises that step, every **downstream** step that depends on it, and any **upstream** step
     whose premise your comment shows was wrong;
   - **note**: a lasting preference to remember, with nothing to change now.
3. You see the **impact before anything happens**: which steps would change (old → new, and why), how many come
   before and after the one you commented on, and that nothing has been applied yet.
4. You choose:
   - **Apply changes**: the steps update. Earlier versions are kept on each step. Anything that costs money or
     reaches the outside world goes back to "needs you".
   - **Keep both (branch)**: an alternative version of the task appears next to the original, so you can compare
     them.
   - **Discard**.

Every comment and change is recorded.

## The human improves the machine: lessons

This is the reverse cyborg from the original vision. The AI shows its reasoning *so that you can improve it*, not
so it can be audited. Besides questions and changes, a comment can be a **lesson**: a better way to think ("also
weigh how fast the broker pays", "a wedding with a rehearsal dinner means arriving a day early").

- The AI re-runs the current task's reasoning with the lesson.
- It states the lesson as a **principle** in one sentence, with a scope (freight, travel, bills…). You keep it
  **from now on**, or use it **just this time**.
- Kept principles are listed under **Learned from you**, with where each came from and how often it's been used.
- Every decision shows **which of your principles it applied**. You can see your teaching at work, and correct
  it when it's applied wrongly.

In the system, a principle is a `preference`/`belief` object with a scope, its source comment as evidence, and a
use count. The mind includes the principles that are in scope whenever it reasons.

## Example: negotiating freight with a broker

A standing rule with judgment-call exceptions:

- **Your direction:** keep the truck loaded at $2.40/mi or more. **Your principle, taught earlier:** going under
  the floor is OK if it gets the truck home for the weekend with little deadhead.
- **What the AI sees:** the truck empties in Dallas Friday at 2pm, the lane is soft (about $2.21/mi), and the
  broker offers $2.10/mi.
- **It decides and acts:** counters at $2.50/mi, because the load was reposted, which suggests the broker is short
  on trucks.
- **The broker comes back at $2.28/mi.** The AI's reasoning: under the floor, but it matches your exception. It
  beats driving home empty (about $640 in cost). Confidence is only 66%, because it's further under the floor
  than you've ever accepted. So it **asks you**, and shows exactly why.
- **You can step in at any point**: approve, set a different counter, or teach it something ("Hollis pays in 2
  days, which is worth a little rate").

## How it maps onto the system

| In the view | In the system (see SYSTEM_INTERFACE.md) |
|---|---|
| Task | A plan or rule run, plus the `intent` object that started it |
| Reasoning, decision | New objects the AI records with each plan: `belief` and `decision` (options, scores, chosen, why, evidence links) |
| Action, result | Plan steps and their journal entries |
| Comment | A `human.commented` event on a step. The mind re-evaluates and proposes revisions as a new plan version |
| Apply / branch | Plan versions: apply replaces the pending version; branch keeps both |
| Needs you | Plans waiting on `ask` or on approval |

**To build next in the core:**

- plans carry their reasoning and decisions as structured data, not just `why` and `conf`;
- a `comment` API op;
- plan versioning (revise and branch);
- a query that returns a task as a graph.

The view itself becomes the LinuxAI shell: a full-screen web UI drawn by the session on a minimal Wayland
compositor, replacing today's text console. The same UI can be served to the phone app.

## Open questions

1. Should the graph be the main view, or should a plain "story" timeline of the task come first, with the graph
   one click away?
2. How much should the screen show while things just run fine: everything, or only what needs you plus a daily
   summary?
3. Can a comment on a finished task change the past? Undo what was done, or only change what happens next time?
