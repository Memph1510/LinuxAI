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
| 2½. **Expectations** | A reply the AI already planned for (the broker's answer to its counter) is matched by the plan waiting for it and handled by the branch decided in advance. | none | core: `wait` with `where`, `track` |
| 4. **Deliberation** | New, unclear or high-stakes work: everything you asked for, and anything the checks flag (first time, low confidence, outside information). High effort and the full toolset. | as needed | as before |

### Habits, precisely

- **Opting in.** A rule's `think` step can name its situation: `"habit": "service.failed:{$event.data.unit}"`.
- **Learning.** When the AI answers that situation and the plan **finishes successfully**, the answer becomes a
  habit. `LINUXAI_HABIT_AFTER` sets how many successes are needed; the default is one.
- **Replaying.** Next time, the core replays the habit **through the same path the AI's answer takes**. It's
  checked again, and trust, approval and the assessment apply as usual. If the check now objects (for example,
  outside information is involved), the AI is woken instead.
- **Retiring.** If a replay **fails**, the habit is retired and the AI thinks about that situation fresh.
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

Outgoing email doesn't exist yet, so "counter at $3,500" and "take $3,200" reach you as messages and questions
with the AI's lean. Once LinuxAI can send email, the same plans can send the counter, with your OK.

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

The day: 9 situations (cups fails three times, nginx twice, memory pressure four times), 11 emails (two
negotiations with a broker, each an offer and a reply; a newsletter, a phishing attempt, 5 routine), 3 tasks you
asked for, and keeping an eye on a load board for Dallas to Atlanta loads at $2,500 or more (three new ones appear),
over 10 hours. In one negotiation the broker answers $3,200 (foreseen); in the other, $2,800 (not). Before, the
board is checked every 30 minutes; with the ladder, it's watched.

| Strategy | Requests | Handled by habit | Replies expected | Output incl. thinking | Est. cost (Opus 5.5) |
|---|---:|---:|---:|---:|---:|
| Before (old prompts and cache) | 187 | 0 | 0 of 2 | 174,000 | $7.10 |
| Old prompts, new cache | 187 | 0 | 0 of 2 | 174,000 | $4.40 |
| **The ladder** | **36** | 6 of 9 | 2 of 2 | **18,000** | **$0.94** (13%) |

Three days: **$2.77 instead of $21.42 (13%)**, with 90 requests instead of 561. Habits handled 24 of the 27
situations, all 6 negotiation replies were expected, and all 9 new loads were reported. In every run the work done
was identical: nothing left unhandled, every service failure restarted, every reply and every new matching load
brought to you.

One scenario alone:
- **Negotiations** (`--only negotiation`): 6 requests instead of 18. A foreseen reply takes no model request at all
  (before: 5, rereading the exchange); an unforeseen one takes 2, with the state handed over.
- **The load board** (`--only board`): 9 requests instead of 100: one to set the watch, two per new load. Before,
  the AI also had to remember on its own which loads it had already told you about.

```sh
python3 tests/thinking_bench.py            # one day
python3 tests/thinking_bench.py --days 3   # habits pay off more as situations repeat
python3 tests/thinking_bench.py --only negotiation   # the broker exchange alone
python3 tests/thinking_bench.py --only board         # keeping an eye on a load board
python3 tests/thinking_bench.py --real     # the real model (needs ANTHROPIC_API_KEY; costs real money)
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
| `LINUXAI_HABIT_AFTER` | `1` | successes before a solution is replayed without the model |
| `LINUXAI_REHEARSAL` | off | `1`: actions are recorded, not performed (benchmarks, trying plans safely) |
| `LINUXAI_RULE_MAX_PER_MINUTE` | `10` | the loop guard on rules |

## Next

- **Local small model for triage** (level 1.5): classifying mail and events on the machine, no API, before
  anything reaches Claude. Measurable with this benchmark once added.
- **Effort sweep with the real model**: the benchmark's `--real` run plus the acceptance scenarios, to confirm
  medium and low effort hold quality on routine work.
- **More habit-ready rules**: name the situations of the AI's own recurring plans.
