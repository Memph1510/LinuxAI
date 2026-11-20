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

The day: 9 situations (cups fails three times, nginx twice, memory pressure four times), 9 emails (2 urgent load
offers, a newsletter, a phishing attempt, 5 routine) and 3 tasks you asked for, over 10 hours.

| Strategy | Requests | Handled by habit | Output incl. thinking | Est. cost (Opus 5.5) |
|---|---:|---:|---:|---:|
| Before (old prompts and cache) | 77 | 0 | 71,700 | $2.88 |
| Old prompts, new cache | 77 | 0 | 71,700 | $2.03 |
| **The ladder** | **25** | 6 of 9 | **12,100** | **$0.69** (24%) |

Three days (27 situations, 27 emails, 9 tasks): **$2.00 instead of $8.64 (23%)**, with 63 requests instead of 231.
Habits handled 24 of the 27 situations. In every run the work done was identical: nothing left unhandled, and
every service failure restarted.

```sh
python3 tests/thinking_bench.py            # one day
python3 tests/thinking_bench.py --days 3   # habits pay off more as situations repeat
python3 tests/thinking_bench.py --real     # the real model (needs ANTHROPIC_API_KEY; costs real money)
```

The unit test `test_the_mind_spends_thinking_by_level` checks the ladder's decisions on every push:
- handled situations don't wake the AI;
- briefs carry the playbook and cases;
- urgent mail goes now and routine mail waits and goes together;
- spam never reaches the model;
- effort follows the stakes.

The core test `a_solved_situation_becomes_a_habit_replayed_without_the_model_until_it_fails` checks the habit
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
