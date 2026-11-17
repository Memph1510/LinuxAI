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

Every comment and change is recorded. Over time, notes and corrections become the AI's preferences ("leaves a
day early for weddings", "never Frontier").

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
