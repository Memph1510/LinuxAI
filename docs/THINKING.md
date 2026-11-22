# How the AI spends its thinking

LinuxAI's AI runs the machine. Most of what it handles is routine: a service that failed again, memory running
tight, the fifth invoice this week. If every one of those wakes a top model at high effort, and the model looks
everything up from scratch each time, running the OS becomes slow and expensive. So the system decides **how
much thinking each situation gets**, and measures the result.

## The ladder

Each situation goes to the cheapest level that can handle it:

| Level | What happens | Model? | Where |
|---|---|---|---|
| 0. **Reflexes** | Rules react to events directly: unplugged means power saving, too hot means cooling down. | none | core rules |
| 1. **Habits** | A situation the AI has already solved, and whose solution worked, is replayed by the core. Example: "the `cups` service failed" means restart it. | none | core: habits |
| 2. **Briefs** | The core gathers what the AI would have looked up (the playbook, similar past cases, the facts) into one message. The AI decides in one step, at medium effort, and can still investigate when the brief isn't enough. | 1 to 2 requests | `linuxai/mind.py` |
| 3. **Batches** | What can wait (routine email, reflecting on finished work) is handled together, at most 10 minutes later, in one conversation, at low effort. | 1 request per batch | `linuxai/mind.py` |
| 1½. **Watches** | A page the AI keeps an eye on is checked by the browser on its own; only new rows that match reach the AI, as just those rows. | none until a match | browser: `watch` |
| 2½. **Expectations: work and documents** | Long work runs in the background and its outcomes are decided when it starts; documents are checked by the core against what was agreed. | none unless it's a surprise | core: `run` background, `agree` |
| 2½. **Expectations: replies** | A reply the AI already planned for (the broker's answer to its counter) is matched by the plan waiting for it and handled by the branch decided in advance. | none | core: `wait` with `where`, `track` |
| 4. **Deliberation** | New, unclear or high-stakes work: everything you asked for, and anything the checks flag (first time, low confidence, outside information). High effort and the full toolset. | as needed | as before |

### Habits, precisely

- **Opting in.** A rule's `think` step can name its situation: `"habit": "service.failed:{$event.data.unit}"`.
- **Learning.** When the AI answers that situation, the answer becomes a habit only once it **held**: finishing
  isn't working. A fix that checks its own result (an `expect` step) counts at once; otherwise it counts when the
  same situation stays away for `LINUXAI_HABIT_HOLD` (15 minutes). If it comes back sooner, the fix didn't work and
  isn't learned. `LINUXAI_HABIT_AFTER` sets how many fixes that held are needed; the default is one.
- **Replaying.** Next time, the core replays the habit **through the same path the AI's answer takes**. It's
  checked again, and trust, approval and the assessment apply as usual. If the check now objects (for example,
  outside information is involved), the AI is woken instead.
- **Retiring.** If a replay **fails**, or the situation comes back within the hold window after it, the habit is
  retired and the AI thinks about that situation fresh.
- **Asking only when needed.** The mind checks a situation still needs a decision before thinking about it, so a
  habit, you, or a duplicate event never cause a second conversation.
- **Yours to manage.** `{"op":"habits"}` lists them. Only you can forget one (`habit_forget`, `source: human`).
- **Visible.** `habit.learned`, `habit.used` and `habit.retired` are events in the journal.

### Expectations: an exchange is decided ahead, not reread

Your example: a broker offers a load at $3,000; we counter at $3,500; they write back "$3,200". The old way, the
reply woke the AI as a new email: it read the claims, the broker's profile, then the earlier messages to work out
what this was about, and only then decided. Every reply in a back-and-forth paid for rereading the whole exchange.

Now, when the AI counters, it writes the follow-up as the same plan:

1. **The state is kept**, small, in one place: `track` writes the exchange (who, what, where it stands, what we're
   waiting for). It's in the AI's context as `<open_threads>`, so it never has to reconstruct it.
2. **The reply is expected**: the plan waits for `message.received` **where** the email claims that load
   reference. Other mail, even from the same broker about another load, doesn't match.
3. **The answers are foreseen**: "they accept $3,500" (close it), "they meet in the middle" (put it to you with
   the state and the AI's lean), "anything else" (`think`, with the state in the note).
4. **The values travel with the email**: the email event carries the claims inline (`rate_cents: 320000`), so
   the branch is chosen by the core, with no model and no lookup.
5. **Nobody is woken twice**: the event is marked `expected_by` that plan, and the mind leaves it to the plan.

A reply the plan didn't foresee ("$2,800") does reach the AI, once, with the state already written in the
note, so one short decision is enough.

**Safety is unchanged.** What a broker writes is still outside information: a plan that waits for a message is
treated as resting on outside information from the start, so anything it would do to the outside world needs
your OK, the same as before. Checked by the test `a_plan_that_waits_for_someones_reply_rests_on_outside_information`.

The counter goes out with `send_email`, with your OK (you see the whole message), until you say "always" for that
broker. Then the same plan waits for the reply.

### Long work and documents, decided ahead

The same idea as the negotiation, in two more places:

- **Long work** (an update, a build, a render): the plan starts it in the background (`run` with `background`),
  waits for `job.finished`, and branches on what it already decided: finished cleanly, a known error and its fix,
  or `think` for anything else. Nobody checks on it while it runs, and a clean finish needs no model. Checked by
  `long_work_runs_in_the_background_and_its_outcome_was_decided_ahead`.
- **Documents against agreements**: when a deal is settled, the plan records what was agreed (`agree`). A rate
  confirmation or invoice citing it is compared by the core as it arrives: a match is filed with no model and you're
  told in a line; only a difference reaches the AI, spelled out. Before, the AI read the document, the sender's
  profile and the earlier messages to work out what had been agreed: 5 requests for each rate confirmation, now
  none. Checked by `a_document_is_checked_against_what_was_agreed` and
  `test_a_document_that_matches_what_was_agreed_needs_no_model`.

### Looking again costs only what changed

- **In one conversation**, reading the same file, read-only command or page again gives only the differences, or
  "unchanged". A 12,099-character log with two new lines comes back as 322 characters; unchanged, as 55. `full`
  gives everything. A new conversation starts with nothing seen, so it always gets the whole thing: a difference
  only means something next to what it changed from.
- **Across conversations**, `read_file` with `new_only` gives just the lines added since the AI last looked, for
  logs. If the file was rotated or cut short, it says so and shows the end.

### Watches: a page is checked by the browser, not by the AI

See [BROWSER.md](BROWSER.md). Keeping an eye on a load board used to mean the AI looking at the whole page every
time (several thousand tokens each, plus working out what was new). Now it sets a watch once, and hears only about
the new loads that match, with just those rows.

### How it talks to itself

- Briefs replace lookups: one message carrying the playbook, cases and facts, instead of three or four tool round
  trips, each re-sending the whole conversation.
- Plans are written whole, in the action language, with branches for likely outcomes, and the core runs them at
  machine speed. The AI isn't consulted step by step.
- Steps are written to fit the next occurrence (`{$event.data…}`), so a good answer becomes a habit.
- Long results come as a summary and pages, and pictures are sized once (see TOKEN_EFFICIENCY.md).

## Measuring it, without an API key

`tests/thinking_bench.py` runs simulated days through the real system and compares strategies on the same events:

- **The real core** in rehearsal mode (`LINUXAI_REHEARSAL=1`: plans, checks, trust and habits all run, but
  nothing touches the machine).
- **The real mind and tools**, so tool results such as playbooks, cases and facts have their real sizes.
- **The real request payloads**, and the prompt cache worked out from their bytes and markers.

**What's assumed** (and printed with the results):
- The model follows its instructions. Under the old prompts it looks the playbook and cases up, because its system
  prompt says to. With a brief it decides at once.
- Thinking per response: low 150, medium 450, high 900 tokens.
- About 3.5 characters per token.

`--real` runs the same day against the real model, which replaces all three assumptions.

The day: 9 situations (cups fails three times, nginx twice, memory pressure four times), 12 emails (two
negotiations with a broker, each an offer and a reply, and the rate confirmation for the one settled; a newsletter, a phishing attempt, 5 routine), 3 tasks you
asked for, and keeping an eye on a load board for Dallas to Atlanta loads at $2,500 or more (three new ones appear),
over 10 hours. In one negotiation the broker answers $3,200 (foreseen); in the other, $2,800 (not). Before, the
board is checked every 30 minutes; with the ladder, it's watched.

| Strategy | Requests | Handled by habit | Replies expected | Output incl. thinking | Est. cost (Opus 5.5) |
|---|---:|---:|---:|---:|---:|
| Before (old prompts and cache) | 192 | 0 | 0 of 2 | 178,000 | $7.39 |
| Old prompts, new cache | 192 | 0 | 0 of 2 | 178,000 | $4.53 |
| **The ladder** | **36–40** | 6 of 9 | 2 of 2 | **18,000–20,000** | **$0.91–$0.99** (about 13%) |

Three days: **$2.83 instead of $22.29 (13%)**, with 90 requests instead of 576. Habits handled 24 of the 27
situations, all 6 negotiation replies were expected, all 3 rate confirmations were checked against the agreement with no model,
and all 9 new loads were reported. In every run the work done
was identical: nothing left unhandled, every service failure restarted, every reply and every new matching load
brought to you.

The ladder's numbers vary a little from run to run (36 to 40 requests in a day): plans run on the core's own threads,
so routine work sometimes lands in one more batch. The work done never varies.

### The real model (2026-11-22, the ladder only)

Run with `--real --strategies ladder` (the owner chose this over the full three-way run, about $10–25; so the
"before" columns above are still the scripted estimate). Every run now starts with what the owner would already have
taught about the broker (`OWNER_KNOWLEDGE` in the benchmark: counter a $3,000 offer at $3,500; take $3,100 or more,
asking first; under that, bring it to him), the same for every strategy. The scripted model had this built in; the
first real run didn't have it.

| | Scripted model | Real, first run (no seed) | **Real, owner's terms seeded** |
|---|---:|---:|---:|
| Conversations | 19–20 | 29 | 22 |
| Requests | 36–40 | 110 | **86** |
| Situations by habit (no model) | 6 of 9 | 5 of 9 | 5 of 9 |
| Output incl. thinking | 18,000–20,000 | 45,547 | 40,386 |
| Cost | $0.91–$0.99 | $1.97 | **$1.66** |
| Left unhandled | 0 | 0 | 0 |
| New matching loads told to you | 3 of 3 | 3 of 3 | 3 of 3 |
| Negotiation replies already expected | 2 of 2 | 0 of 2 | **2 of 2** |
| Rate con checked against the agreement, no model | 1 of 1 | 0 of 1 | **1 of 1** |
| Service restarts | 5 of 5 | 0 of 5 | 0 of 5 |

Costs are with the 22k prefix already in the 1-hour cache; a cold start adds about $0.18.

So a real day of this workload costs about **$1.70**, with about twice the requests the scripted model needed. With
the owner's terms known, the real AI did what his example intended: it countered both $3,000 offers at $3,500 (with
his OK), wrote plans that already knew what each answer would mean, and handled both replies there ("Dana answered our
$3,500 counter on HL-0-1 with … Take it?"; "… under your $3,100 floor … Your call"). The rate confirmation was then
filed by the core against the agreement, no model. It also put the board's new loads next to Dana's open offer on
the same lane without being asked.

What's still different, read from the messages the AI sent the human (the benchmark saves them):
- **Service restarts, 0 of 5: correct for the machine it ran on.** The real AI checked first, found no systemd and
  no cups or nginx on the test container, and treated the synthetic failure events as false alarms. Measuring this
  needs a machine where those services exist (the VM image).
- **More conversations than the stand-in**: the synthetic tasks carry no details ("Weekly fuel report") and the
  synthetic routine emails are one line ("rate $1,630"), so the real AI asked instead of guessing.
- **Money was shown in cents to the human** ("320000 cents"): its plans put the raw `rate_cents` value in the question.
  Fixed in the core: a `_cents` field in text now reads as dollars ("$3,200"), and records read "rate $3,200".
- In the unseeded run the benchmark's scripted human answered "take" even when the AI offered other options; an
  answer outside the options read as "no", and the AI caught the contradiction and asked again. The scripted human
  now answers with the question's own "accept" option.

The first real run also found a bug: a tool failure (a screenshot reply without a tab) ended the whole background
AI. Now any tool failure goes back to the AI as an error result (see TESTING.md).

One scenario alone:
- **Negotiations** (`--only negotiation`): 6 requests instead of 23. A foreseen reply takes no model request at all
  (before: 5, rereading the exchange); an unforeseen one takes 2, with the state handed over; the rate
  confirmation that follows takes none (before: 5).
- **The load board** (`--only board`): 9 requests instead of 100: one to set the watch, two per new load. Before,
  the AI also had to remember on its own which loads it had already told you about.

```sh
python3 tests/thinking_bench.py            # one day
python3 tests/thinking_bench.py --days 3   # habits pay off more as situations repeat
python3 tests/thinking_bench.py --only negotiation   # the broker exchange alone
python3 tests/thinking_bench.py --only board         # keeping an eye on a load board
python3 tests/thinking_bench.py --real --strategies ladder   # the real model, the ladder only (about $1.70)
python3 tests/thinking_bench.py --real     # all three with the real model (about $10–25)
```

The unit test `test_the_mind_spends_thinking_by_level` checks the ladder's decisions on every push:
- handled situations don't wake the AI;
- briefs carry the playbook and cases;
- urgent mail goes now and routine mail waits and goes together;
- spam never reaches the model;
- effort follows the stakes.

The core test `a_reply_in_a_negotiation_is_already_understood` runs your example through real email ingestion:
unrelated mail isn't taken as the answer, the $3,200 reply is matched and put to you with the state, and the
exchange's history is kept. The core test `a_solved_situation_becomes_a_habit_replayed_without_the_model_until_it_fails` checks the habit
lifecycle.

## Settings

| Setting | Default | |
|---|---|---|
| `LINUXAI_MIND` | `ladder` | `full` restores one fresh conversation per event |
| `LINUXAI_MIND_BATCH` | `600` | the most seconds routine work waits to be batched |
| `LINUXAI_HABIT_AFTER` | `1` | fixes that held before a solution is replayed without the model |
| `LINUXAI_HABIT_HOLD` | `900` | seconds a situation must stay away after a fix for it to count (the benchmark sets 0: its hours pass in seconds) |
| `LINUXAI_REHEARSAL` | off | `1`: actions are recorded, not performed (benchmarks, trying plans safely) |
| `LINUXAI_RULE_MAX_PER_MINUTE` | `10` | the loop guard on rules |

## Next

- **Local small model for triage** (level 1.5): classifying mail and events on the machine, no API, before
  anything reaches Claude. Measurable with this benchmark once added.
- **Run the benchmark in the VM image**, where the failing services exist; then the full `--real` run (all three
  strategies) if the owner wants the before/after measured.
- **Effort sweep with the real model**: email extraction scored the same at low and medium effort (MAIL.md); the
  mind's routine work still needs the sweep.
- **More habit-ready rules**: name the situations of the AI's own recurring plans.
