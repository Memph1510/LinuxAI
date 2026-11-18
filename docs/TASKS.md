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

## A second look before acting (enforced by the core)

The AI never acts on a guess, whether or not it was given direct instructions.

- **Every plan that changes something carries an assessment:** the AI's confidence, the consequences it
  expects, the possible outcomes with what it would do in each, what's still unknown, and what it researched.
  The core **rejects** a plan without one.
- **Second look:** even for actions the AI is already trusted with, the plan waits for a conversation with you
  when its confidence is below the threshold (80%), when it lists unknowns, or when it's a **first-time
  situation**: its category has no playbook and no past cases, in that category or a broader or narrower one.
  You see the plan and the whole assessment.
- **First-time situations need research first:** the core won't accept the plan until the assessment includes
  what the AI looked into (playbooks, similar cases, read-only investigation). Then it still goes over it with
  you.
- **Blockers:** when a step fails, the AI gets it to think about (`think`), and its fix goes through the same
  gate. You're told about the blocker right away, by message.
- **Unfamiliar or unclear directions:** the AI is instructed to ask (or create a human step) instead of
  guessing. When it does act, its listed unknowns trigger the second look.

Answering "always" only extends trust to *actions*. It never skips the second look for a new or uncertain
*situation*. As experience builds up (cases, playbooks), situations stop being new, and the second look
happens less often.

## Research includes the internet

Research for a first-time or uncertain situation isn't limited to what's on the machine. The AI uses web search
and web fetch (Anthropic's server-side tools; `LINUXAI_WEB=0` turns them off) for rates, weather, regulations,
reputations and documentation, and cites the URLs it relied on in the assessment.

Credible sources are decided as they're encountered. Each website the AI relies on is rated for a category
(**trusted**, **useful**, **doubtful**, **blocked**) with the reason, and with whether the rating came from you or
from the AI. Your rating always wins. Ratings for broader categories apply to narrower ones. Before researching,
the AI checks what's already rated, prefers trusted sources, and corroborates doubtful or unrated ones. Blocked
sites are excluded from searches automatically.

## Playbook protections

From the research in [RESEARCH_SELF_LEARNING.md](RESEARCH_SELF_LEARNING.md) (ACE, Memento-Skills, Recuris):

- **Small edits, never rewrites.** A playbook is a set of entries: points under *What matters*, *How I
  decide*, *Exceptions* and *Open questions*, plus any sections you add. Each has an id, its evidence, who wrote
  it, and how often it **helped** or **hurt**. The AI can only add one, revise one or retire one, and plain
  code merges the change. The AI is refused if it tries to rewrite a whole playbook, so a playbook can't
  collapse into a short summary that loses what was learned.
- **The markdown file is generated from the entries and stays yours.** Edit it in any editor. A changed line
  updates its entry, a new line becomes an entry (marked as yours), and a deleted line retires its entry.
- **Validation gate.** Before an AI change is kept, the AI checks it against the most recent cases in that
  category (the core picks them): would each past decision have come out the same, better or worse? The
  change is **held for you** if it would have made any past decision worse, if it changes or removes a point
  **you** wrote, or if it changes a large part of the playbook at once. You get the change and the reason, and
  "yes" applies it. Duplicates of existing points are refused.
- **Feedback.** After each task the AI records which points helped or hurt. A point that keeps hurting is
  flagged "often wrong" in the file, and the AI is told to reconsider it.
