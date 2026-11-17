# Tasks, playbooks and cases

How LinuxAI takes on work of any size, coordinates the human where needed, and gets better at it over time.
Built in the core (`core/src/tasks.rs`) and used by the AI (`create_task`, `plan_task`, `read_playbook`,
`update_playbook`, `record_case`, `find_cases`).

## Tasks, micro and macro

You hand the OS a task: "pay the Verizon bill" or "keep the truck loaded at good rates this month". Big tasks
become a tree of subtasks:

```
Get the truck a load home from Dallas            [freight/negotiate]   running
├─ Find loads                          work      done
├─ Which load?                         decision  waiting on you ─┐ reached you by text, with why + lean
├─ Sign the rate confirmation          action    pending  (after "Which load?")
└─ Put the pickup on the calendar      work      pending  (after "Sign…")
```

- **Work** is done by the AI: with a plan written into the task, or, if it has none, the AI is asked to plan
  it when it's ready, using the category's playbooks and similar past cases.
- **Human steps** are the four ways a person is needed:
  - **action**: only you can do it (sign, call, pick up);
  - **evaluation**: is this result good?
  - **critique**: how would you improve it?
  - **decision**: which option?

  Each one reaches you right away with its question, why, and what the AI leans toward. It follows up, can
  fall back at a deadline, and the task continues the moment you respond.
- **Order:** `after` makes a step wait for its siblings. A failure stops whatever depended on it, and the AI is
  told so it can rethink.
- **Authority:** plans written into a task run with the authority of whoever created the task. If the AI
  creates one with an action it isn't trusted with yet, the task waits on your OK.
- **Durability:** tasks survive restarts and pick up where they were.
- **Mostly invisible:** you see a task when you look, or when it needs you.

## Categories and playbooks (markdown you can edit)

Every task has a category such as `freight/negotiate/hollis`. Each category can have a **playbook**: a short
markdown file that governs how that kind of task is done.

```
~/playbooks/
├── freight.md                      applies to all freight work
├── freight/negotiate.md            applies to all negotiating
└── freight/negotiate/hollis.md     what's specific to this broker
```

When the AI works a task, it reads the whole chain, broadest first. Suggested sections:

```markdown
## What matters        (the variables: broker, weather, destination, trip length, days to deliver, rate…)
## How I decide        (rules of thumb)
## Exceptions          (when the rules bend, and why)
## Open questions      (what isn't understood yet; worth asking the human)
```

The AI rewrites a playbook when it learns something: from repetition, a new situation, or your answers and
critiques. It credits you where a point came from you. Every change keeps the previous version and a reason,
in the playbook history. You can edit the files yourself too; they're plain markdown.

## Cases: learning by repetition

Each real situation is recorded as a **case**: its category, its variables, what was decided, and how it turned
out.

```json
{"category": "freight/negotiate", "broker": "@hollis", "weather": "snow", "destination": "Atlanta",
 "trip_miles": 781, "days_to_deliver": 3, "rate_per_mile": 2.55, "decision": "accepted", "outcome": "paid in 2 days"}
```

Before deciding, the AI looks up similar cases ("Hollis, under the floor, in winter"). After a task finishes, it
records the case and folds anything new into the playbook. Cases are the raw experience; playbooks are the
understanding distilled from it.

## The loop

```
task → (playbooks + similar cases) → plan → human steps where needed → done
     → record the case → update the playbook → the next similar task needs less of you
```

Problems are fixed by category, as they come up: a bad outcome updates that category's playbook, so the
fix reaches every future task of that kind and nothing else.
