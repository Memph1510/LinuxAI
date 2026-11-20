# Testing

| Kind | What it covers | How to run | Where |
|---|---|---|---|
| **Unit and integration (core)** | Every organ and the plan engine, against a real in-memory core: calendar, mail, tasks, habits, browser (real Chromium), Blender (real Blender when installed), craft (a stand-in app with a real GPG signature), memory failures, and more. 93 tests. | `cd core && cargo test --release` | CI, every push |
| **Unit and integration (AI layer)** | The AI's tools and loop against a live core, with a scripted model: trust and outside information, paging, the model table, prompt-cache stability, picture sizing, the usage ledger, the mind's thinking ladder. 52 tests. | `pytest -q` | CI, every push |
| **End-to-end (the OS)** | Builds the image and boots it in QEMU, then runs 31 checks plus screenshots: services, plans, power, mail, calendar, browser, pictures, Blender, Craft sandbox, the screen, the Workspace, the always-on layer. | `./image/build.sh && python3 image/test-boot.py out/linuxai-*.iso` | Before every merge |
| **End-to-end (Craft apps)** | All 15 apps: verified install (pinned hash and GPG signature), sandboxed start, a tool call, close. | `scratchpad`-style script in an Arch container with bubblewrap (see the craft PR) | When the manifest or the craft organ changes |
| **Soak / regression** | Days of activity compressed into minutes: event storms, macro tasks with a simulated human, AI plans, `kill -9` every few minutes, pruning. Checks memory, threads, file handles, database size, response times and invariants. | `python3 tests/soak.py --minutes 20` | Before releases |
| **Sanity** | Every `linuxai-core` and `cortex` entry point starts and answers. | the CLIs with `caps`, `verbs`, `snapshot`, `check` | After builds |
| **Static analysis and dependency audit** | Rust lints (clippy), Python lints (ruff), known vulnerabilities in dependencies (cargo-audit, pip-audit). | `cargo clippy --all-targets`, `ruff check linuxai tests`, `cargo audit`, `pip-audit` | Before releases |
| **User acceptance (the real AI)** | Scenarios from real use: the cache works live; the calendar fills from conversation; an email that tries to instruct the AI is refused and reported; a destructive change waits for the human, and "no" means nothing happens; the end of a long log is found. Each reports its token cost. | `python3 tests/uat.py` (needs `ANTHROPIC_API_KEY`; a few cents per scenario) | When the AI's prompts or tools change |
| **Thinking efficiency** | Simulated days through the real core (rehearsal mode), mind and tools, comparing how the AI spends its thinking: requests, tokens, cost, situations handled by habit, and that the same work got done. No API key needed; `--real` uses the real model. See THINKING.md. | `python3 tests/thinking_bench.py [--days 3]` | When the mind, prompts or rules change |
| **Token profile** | Exact tokens of the fixed prefix and of each tool. | `python3 tests/token_profile.py` (needs the key; free) | When prompts or tools change |

## The API key

The acceptance tests and the token profile call the Claude API. In this project's cloud environment, a Claude
API key can only be stored as an environment variable, `ANTHROPIC_API_KEY`, set in the environment's settings.
Anyone using that environment, and any command run there, can read it. A new session picks it up. On a LinuxAI
machine, `cortex` asks for the key on first run and keeps it on that machine.

## Last full run (November 2026)

- **Core:** 93 passed.
- **AI layer:** 52 passed.
- **Boot test:** 31 of 31 passed (on the craft merge).
- **Craft end-to-end:** 15 of 15 apps. WordCraft failed on the first run and was fixed: some apps take only one
  read folder.
- **Dependency audit:** one medium SQLite CVE (RUSTSEC-2026-0459), fixed by moving to rusqlite 0.42. Now
  clean. `chrono` and `chrono-tz` are marked unmaintained, with no replacement yet.
- **Lints:** no correctness findings. One test that accepted any exception was tightened.
- **Sanity:** `linuxai-core check` silently checked an empty plan when it couldn't read its input. Fixed.
- **Thinking benchmark:** the ladder does the same work for 24% of the cost on one day (25 requests instead of 77), and 23% over three days.
- **Soak:** 20 minutes, 11 of 11 checks, p99 under 19 ms, memory flat at 15 MB.
- **Acceptance and token profile:** waiting for the API key.
