# Token efficiency

How LinuxAI keeps what its AI sends to the Claude API small and cheap, what's measured, and what's next.
Model: Claude Opus 5.5. Prices per million tokens: $4 input, $20 output, $0.20 for input read from cache.
Writing to the cache costs 1.25× input for 5 minutes, or 2× for 1 hour.

## What every request carries

| Part | Tokens (estimate) |
|---|---:|
| System prompt (rules, action language, verb list, browser) | about 4,000 |
| Tool definitions (39 tools plus web search and web fetch) | about 8,100 |
| **Fixed prefix** | **about 12,000** |

The largest tools are Blender (about 1,000), Craft (680), browser (680), image (500) and calendar (425).
These are character-based estimates. `tests/token_profile.py` gives exact counts from the free
token-counting endpoint once `ANTHROPIC_API_KEY` is set.

Uncached, the prefix costs about **$0.05 per request**. Read from the cache, it costs about
**$0.0024**. So keeping the cache working is by far the biggest lever.

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
   `~/.local/share/linuxai/usage.jsonl`. Cache writes were previously uncounted.
   `cortex usage` shows tokens and cost per day, role and model, plus the share of input read from the cache.
6. **One table of which model does which job** (from the ArtCraft work). Background work and email reading can
   run on a cheaper model if you choose: Claude Haiku 5.5 is $0.10 / $0.50 per million, 40× cheaper than Opus
   on input. That's your decision, set in the Workspace. The AI never downgrades itself to save money.

## How the AI spends its thinking

The biggest lever isn't the size of a request but how many the AI needs to run the machine. See
[THINKING.md](THINKING.md): reflexes, habits, watches, expectations (replies decided ahead), briefs, batches and effort by stakes. On a
simulated day the same work costs about 13% of what it did, with 36 to 40 model requests instead of 192, measured with
`tests/thinking_bench.py`.

## Next, once there are measurements

Measure first with `tests/uat.py` and the ledger, then change one thing at a time:

| Lever | Expected effect | Why it waits |
|---|---|---|
| **Effort per job** (the session is at `high`; background and email at `medium` or `low`) | Fewer thinking and output tokens. On research-like work, `medium` often matches `high`. | Needs a sweep on real scenarios to make sure answers hold up. |
| **Load rarely used tools on demand** (tool search with `defer_loading` for Blender, Craft, calendar, image) | About 3,300 fewer prefix tokens | Cached prefix tokens are already cheap, and deferring can add discovery turns. Worth it only if the measurement shows it doesn't. |
| **Cheaper models for background work and email reading** (the model table) | Up to 40× cheaper input on those jobs | Your call, after seeing quality on the acceptance scenarios. |
| **Batch API for work that can wait** (overnight playbook reviews, digests) | 50% off every token | There's no overnight job yet. |
| **Compaction for very long sessions** (server-side, beta) | Pays only past about 150,000 tokens | Background conversations start fresh per event, and interactive ones rarely get that long. |
| **Clearing old tool results** (server-side context editing) | A context-window tool, not a saving: each clear rewrites the cache | Only if conversations hit the context limit. |

To measure, add `ANTHROPIC_API_KEY` as an environment variable (see TESTING.md), then run:

```sh
python3 tests/token_profile.py   # exact prefix and per-tool tokens (free)
python3 tests/uat.py             # acceptance scenarios with their token cost (a few cents each)
cortex usage                     # what real use costs, from the ledger
```
