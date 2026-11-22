# Token efficiency

How LinuxAI keeps what its AI sends to the Claude API small and cheap, what's measured, and what's next.
Model: Claude Opus 5.5. Prices per million tokens: $4 input, $20 output, $0.20 for input read from cache.
Writing to the cache costs 1.25× input for 5 minutes, or 2× for 1 hour.

## What every request carries (measured)

Exact counts from the token-counting endpoint (`tests/token_profile.py`, 2026-11-22, Claude Opus 5.5), for the
full session: core, browser and web on.

| Part | Tokens |
|---|---:|
| System prompt (rules, action language, verb list, browser) | 6,212 |
| Tool definitions (39 tools plus web search and web fetch) | 16,331 |
| **Fixed prefix** | **22,543** |

The earlier character-based estimate (about 12,000) was low by almost half. Counted one at a time, the largest tools
are web search (5,874) and web fetch (4,543), then Blender (1,475), browser (1,364), Craft (1,030), image (896) and
calendar (797). A tool counted alone includes some fixed tool-use overhead, so these add up to more than the total.
The two server tools are over half of all tool tokens. The background AI's prefix measured 22,300–23,000 tokens in
real requests. A `cortex` one-shot without the core is about 9,300.

Per request, the prefix costs **$0.090 uncached**, **$0.180 to write to the 1-hour cache** and **$0.0045 read from
the cache**. In every real run the cache worked as designed:
- the second request of a conversation read the whole prefix;
- the 1-hour marker carried it across separate processes and conversations: the second background event, minutes
  after the first, read 22,330 tokens instead of writing them;
- across the acceptance scenarios, 93–98% of input was read from the cache.

Fresh (uncached) input was 2–14 tokens per request.

## Done

1. **A stable, cached prefix.**
   - The tool set is fixed when a session starts. Before, the screen tool could appear or vanish mid-session,
     and any change to the tool list invalidates the whole cache.
   - The system prompt carries an explicit **1-hour** cache marker. The background AI starts a fresh
     conversation per event, and events (or your replies) often come more than 5 minutes apart, so a
     5-minute entry would have expired and every such event paid the full write again.
   - The growing conversation keeps automatic 5-minute caching.
   - The history is only ever appended to. On this model, editing earlier turns on our side would break both
     the cache and the AI's earlier reasoning, so nothing does.
   - A test checks that two requests carry byte-identical tools and system, and that the second request's
     history starts with the first's.
2. **Long results come in pages** (summary and page 1 of N, with `more` for the rest). A 4,000-line log
   doesn't enter the conversation whole.
3. **Looking again costs only what changed.** Rereading a file, command output or page in the same conversation
   gives the differences (a 12,099-character log with two new lines: 322 characters). `read_file` `new_only` gives
   a log's new lines since the AI last looked, across conversations. Pages the AI keeps an eye on are watched by
   the browser instead of reread (see BROWSER.md).
4. **Pictures are sized once**, before they're first sent: at most 1,280 px on the long side. A picture costs
   about width × height / 750 tokens every time a request carries it. A 1920×1080 screenshot is about 2,760
   tokens and becomes about 1,180. Pictures are never shrunk later, because that would be a history edit.
5. **A usage ledger.** Every request's tokens (fresh input, read from cache, written to cache, output) go to
   `~/.local/share/linuxai/usage.jsonl`. Cache writes were previously uncounted. Each row also keeps how much of the
   write was for 1 hour, as the API reports it, so `cortex usage` prices 1-hour writes at 2× and 5-minute writes at
   1.25× instead of assuming the upper bound. `cortex usage` shows tokens and cost per day, role and model, plus the
   share of input read from the cache.
6. **One table of which model does which job** (from the ArtCraft work). Background work and email reading can
   run on a cheaper model if you choose: Claude Haiku 5.5 is $0.10 / $0.50 per million, 40× cheaper than Opus
   on input. That's your decision, set in the Workspace. The AI never downgrades itself to save money.

## How the AI spends its thinking

The biggest lever isn't the size of a request but how many the AI needs to run the machine. See
[THINKING.md](THINKING.md): reflexes, habits, watches, expectations (replies decided ahead), briefs, batches and effort by stakes. On a
simulated day the same work costs about 13% of what it did, with 36 to 40 model requests instead of 192, measured with
`tests/thinking_bench.py`.

## What the first real runs cost (2026-11-22)

| Run | Requests | Cost |
|---|---:|---:|
| One `cortex` turn ("how much free disk space?", one command) | 2 | $0.08 (writing a cold prefix) |
| One background event (a plan's `think` step: investigate, resolve, ask the human) | 3 | about $0.05 (prefix already cached) |
| The 5 acceptance scenarios, cold cache | 18 | $0.35, of which $0.17 is the first prefix write |
| The 5 acceptance scenarios, warm cache | 15 | $0.16 |
| Email extraction, per email | 1 | $0.022 (Opus 5.5) · $0.0007 (Haiku 5.5) |

Output, including thinking, came to under 100 to about 850 tokens per request. The scripted benchmark assumes 450 at medium effort
and 900 at high. The real ladder day is in THINKING.md.

## Next, once there are measurements

Measure first with `tests/uat.py` and the ledger, then change one thing at a time:

| Lever | Expected effect | Why it waits |
|---|---|---|
| **Effort per job** (the session is at `high`; background and email at `medium` or `low`) | Fewer thinking and output tokens. On research-like work, `medium` often matches `high`. | Needs a sweep on real scenarios to make sure answers hold up. |
| **Load rarely used tools on demand** (tool search with `defer_loading` for Blender, Craft, calendar, image) | Up to about 4,000 fewer prefix tokens (their measured sizes, less shared overhead) | Cached prefix tokens are already cheap ($0.0045 per request for all 22,543), and deferring can add discovery turns. Worth it only if the measurement shows it doesn't. |
| **Cheaper models for background work and email reading** (the model table) | Email reading: measured 30× cheaper on Haiku 5.5 with the same score (MAIL.md) | Your call. |
| **Batch API for work that can wait** (overnight playbook reviews, digests) | 50% off every token | There's no overnight job yet. |
| **Compaction for very long sessions** (server-side, beta) | Pays only past about 150,000 tokens | Background conversations start fresh per event, and interactive ones rarely get that long. |
| **Clearing old tool results** (server-side context editing) | A context-window tool, not a saving: each clear rewrites the cache | Only if conversations hit the context limit. |

To measure again (`ANTHROPIC_API_KEY` must be set; see TESTING.md):

```sh
python3 tests/token_profile.py   # exact prefix and per-tool tokens (free)
python3 tests/uat.py             # acceptance scenarios with their token cost (a few cents each)
python3 tests/extract_eval.py    # email extraction accuracy and cost (about 2 cents per email)
cortex usage                     # what real use costs, from the ledger
```
