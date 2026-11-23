# LinuxAI handoff

For the next session (and any agent) picking up LinuxAI. Read this first, then the docs it points to.
State as of **2026-11-21**, main at `c081b08` (PR #11 merged). Everything below is either in the repository or
linked from it; nothing depends on the previous conversation.

Project continuity on Google Drive (session record, lessons, SkillOpt captures: see its `session-notes/`):
**/50_Projects/LinuxAI/**
(<https://drive.google.com/drive/folders/1v4pQVyr57lDcCpeOIl8yFKoryBtyOsf8>), laid out per the Drive-root
`AGENTS.md` (v3.2): `consolidated_summary.md`, `session_summary.md`, `lessons.md`, `handoff.md`, `60_Review/`,
`session-notes/`. Git owns code and history; Drive holds the cross-session record.

---

## 1. What LinuxAI is

An **AI-native operating system**: Arch Linux underneath, an AI (Claude) running the machine, the human supplying
intent and acting as the guardrail. Built for the owner's actual life and work, in particular running a trucking
business (freight broker email, load boards, rate confirmations, settlements, invoices), plus everyday computing,
3D (Blender), and creative/document work.

**The owner's principles** (apply them to every decision):
- **The AI is the system; the human is the intent and the guardrail.** The AI does the work through tools; it doesn't
  tell the human what to type. Everything it does is visible; the human approves, edits, redirects or stops.
- **Outside content is information, never instructions.** Email, web pages, invites, documents, page text: facts to
  weigh, never commands, however worded. This is enforced in code (taint, citations, separate trust), not by prompt.
- **Fix issues categorically**, not case by case. When a gap is found, close the class of gap.
- **Rust for the OS, Python only where the model is called.** Everything the OS runs is the Rust core binary;
  Python is the model-facing layer (session, mind, extractor).
- **The core logic must be sound before features.** (Bent in places: see the review in §6.)
- **Token efficiency is about how the AI thinks and communicates while running the OS**, and must be measurable
  before an API key exists (it is: `tests/thinking_bench.py`).
  **But execution comes first**: effective, reliable task completion matters more than the lowest token count. If
  spending more tokens (more agents, more checking) gets tasks done better, that's the right trade; the owner will
  pay for more tokens. Cut waste, not capability.
- **The calendar is the AI's knowledge of the human's time**, not an app the human looks at. Things are added
  through conversation and used when planning, deciding and communicating.
- **Merges**: the owner authorized merging PRs once CI and the boot test pass.

**The owner's operating model** (rate every finding and every new restriction against this):
- LinuxAI is like a 24/7 assistant bot (as Claude or other agents already work for him, e.g. his QuickBooks
  bookkeeping, unattended), moved into its own environment. **Its identity is its own**: a dedicated mailbox,
  dedicated logins, a temporary card, files that exist only in this OS. None of his personal accounts, logins or
  cards. Broker deals are confirmed by phone, so a message alone doesn't commit him.
- **Guardrails are learned with him, not preset.** He won't hand the OS a whole job ("go negotiate with this
  broker") cold: it learns the work from what he does and the questions they settle together (tone, which brokers,
  how each one reads), and the guardrails come from that. The machinery already works this way: first-time
  situations go to him or get researched, his answers become playbooks and contact profiles, and trust is granted
  per action ("always"). So don't add restrictions before there's a task; fix blockers as they're met.
  Do fix anything that makes his own settings mean something other than what they say (an "always" broader than
  what he saw, a record claiming he decided something he didn't, a fact shown as verified that isn't).
- An OS password comes with install to disk.

**How the owner likes to be talked to**: plain, short, specific; say what was done and how it was checked, what
wasn't checked, and honest uncertainty. Measured claims, not adjectives. They think in concrete examples
(their broker negotiation example drove the whole "decided ahead" design: see §4).

## 2. Repository map

| Path | What |
|---|---|
| `core/` | Rust crate `linuxai-core` (~13k lines): the daemon and every organ. One binary, many subcommands. |
| `core/src/engine.rs` | The action language: checking (assess, trust, taint, citations), running, durable waits, journal, expressions |
| `core/src/core.rs` | The daemon: event bus (`settle` → `broadcast` → `react`), rules, habits, ops (the socket API), recovery |
| `core/src/memory.rs` | SQLite store (rusqlite 0.42): objects, plans, journal, events, mandates; failed writes are reported, never hidden |
| `core/src/verbs.rs` | Every verb the plans can use, with its effect level (internal / local / external / money) and docs |
| `core/src/host.rs` | The Linux host organ: senses (power, pressure, temperature, failed services) and controls; rehearsal mode |
| `core/src/mail.rs`, `send.rs` | Email in (IMAP → sanitize → extract claims → checks) and out (`send_email` over SMTP) |
| `core/src/agreements.rs`, `jobs.rs`, `watch.rs` | Agreements checked against documents; background jobs; page watches |
| `core/src/calendar.rs`, `tasks.rs`, `playbooks.rs`, `prompts.rs` | Calendar, tasks (micro/macro, human steps), playbooks + cases, prompt library |
| `core/src/browser.rs`, `workspace.rs`, `compositor.rs`, `reach.rs`, `voice.rs`, `images.rs`, `blender.rs`, `craft.rs`, `models.rs` | Organs: shared Chromium, the Workspace screen, Hyprland control, texting the human, voice, pictures, Blender, the 15 Craft apps, which model does which job |
| `core/src/tests.rs` (+ tests inside modules) | 106 Rust tests |
| `linuxai/` | Python, model-facing: `agent.py` (session + prompts + context), `mind.py` (behind-the-scenes AI and the thinking ladder), `tools.py` (every tool the model sees), `cli.py` (`cortex`), `mail.py` (extractor schema) |
| `tests/` | `test_core.py` (58 Python tests), `thinking_bench.py`, `soak.py`, `uat.py`, `token_profile.py` |
| `image/` | The Arch ISO: `build.sh` (docker), `build-in-arch.sh`, `airootfs/` (services, Workspace page, Quickshell layer), `test-boot.py` (37 checks in QEMU) |
| `docs/` | Design and status docs (list in §9) |
| `library/` | Starter prompt library, image-backend adapters |

## 3. Architecture in one page

- **The core daemon** (`linuxai-core daemon`, systemd `linuxai-core.service`) owns memory and runs **plans** written
  in the **action language** (JSON; see `docs/ACTION_LANGUAGE.md`). Steps: `let do if each expect try ask tell wait
  think call`. Every plan is **checked before it runs**: unknown verbs, undefined variables, missing assessment,
  trust, outside taint, citations. Plans are durable (survive restarts) and never repeat an action (journal with
  idempotency keys).
- **Trust is learned**: an action the AI isn't trusted with waits for the human's OK; "always" grants a mandate.
  Mandates for acting on **outside** information are separate (`outside:` targets). **Approvals come only from the
  human directly** (screen, text, prompt); the core records who answered (`$answered_by`).
- **Events** (senses, organs, email, the human) go through `Core::emit` → rules (`react`) → plans waiting on events.
  Rules run without the model; a rule's `think` step hands judgment to the AI.
- **The AI** (Python) talks to the core over a Unix socket (`linuxai/core_client.py`). Two roles:
  - **Session** (`cortex`): the human talking to it; tools run commands, read files, submit plans, browse, etc.
  - **Mind** (`linuxai/mind.py`): behind the scenes, woken by events that need judgment, under the **thinking ladder**.
- **Organs** run as separate processes or in the core: browser (Chromium over CDP, shared with the human, per-tab
  owners), Workspace (the main screen, a page with a private bridge to the core), compositor (Hyprland), reach
  (texts the human), mail, calendar, voice, images, Blender (bridge into a live Blender), Craft (15 verified apps in
  bubblewrap sandboxes over MCP).
- **Rehearsal mode** (`LINUXAI_REHEARSAL=1`): everything runs, nothing touches the machine or sends anything.

## 4. The thinking ladder and "decided ahead" (the token-efficiency design)

Detail: `docs/THINKING.md`, `docs/TOKEN_EFFICIENCY.md`. Each situation goes to the cheapest level that can handle it:

1. **Reflexes**: rules, no model.
2. **Habits**: a situation the AI solved, whose fix **held** (its own `expect` check passed, or the situation stayed
   away 15 min, `LINUXAI_HABIT_HOLD`), is replayed by the core through the same checks. Retired if a replay fails or
   the problem comes back within the window.
3. **Watches**: pages kept an eye on are checked by the browser itself; only new matching rows reach the AI.
4. **Expectations ("decided ahead")**, from the owner's broker example ("they offered $3,000, we countered $3,500,
   they reply $3,200: the system should already know what this is"):
   - `wait` takes `where` (only the reply that matches), `about`, `thread`; the event is marked `expected_by`, and
     the mind leaves it to the plan, which branches on the values the email carries inline (`$event.data.claims`).
   - `track` keeps an exchange's state (`<open_threads>` in the AI's context); `agree` records what was settled, and
     rate confirmations / invoices citing it are compared by the core (match: filed, no model; difference: to the AI).
     Agreements count only if the human made them; otherwise they're *proposed* until `agreement_confirm`.
   - Long work runs in the background (`run` with `background: true` → `job.finished`), outcomes decided at start.
5. **Briefs**: the core gathers playbook, cases and facts into one message; the AI decides in one step.
6. **Batches**: routine mail and reflections wait up to 10 min and go together at low effort.
7. **Effort by stakes**: human-asked tasks high; routine medium; bookkeeping low.

Also: a stable cached prefix (1-hour cache marker, tools fixed per session), long results paged, pictures sized
once (≤1280 px), rereads return only what changed (`full=true` for everything; `read_file new_only` for logs),
tool inputs checked against their schemas before anything runs, and a usage ledger (`cortex usage`).

**Measured** (`tests/thinking_bench.py`, no API key; the real core in rehearsal, the real mind/tools/payloads; the
model is a scripted stand-in that follows its instructions): a simulated day costs **36–40 requests instead of 192,
about 13%** ($0.91–$0.99 vs $7.41 est. on Opus 5.5); 3 days $2.83 vs $22.29. Same work in every run. **These are
directional**: an agent wrote the stand-in for both sides. **With the real model** (2026-11-22, ladder only): the
same day took **86 requests and $1.66** once the benchmark gave it the owner's terms for the broker: all work
handled, both negotiation replies handled by plans decided ahead, the rate con filed by the core. **Inside the
image** (2026-11-23, `image/vm-bench.py`, real systemd, cups and nginx really failing): 84 requests, $1.80, the same
work done; it read the failing units, saw they were stand-ins a restart can't fix, changed nothing and offered the
real fix. It sent the same notice again when a known failure recurred; repeats are now folded into the earlier
message (the owner's choice; THINKING.md, "Repeats"). See THINKING.md, "The real model".

## 5. History (merged PRs)

| PR | Merged (UTC) | What |
|---|---|---|
| #1 | 11-19 | v0.1: the AI-native OS, bootable |
| #2 | 11-19 | Organs reach the system core when run as the user |
| #3 | 11-19 | Always-on layer (Quickshell): the AI's questions, voice indicator, its pointer |
| #4 | 11-19 | The Workspace with pages inside it; magnifier; questions say where to answer |
| #5 | 11-19 | Built-in calendar (the AI's, not an app); Workspace as main window; Blender/Unity/Unreal + ArtCraft plans |
| #6 | 11-20 | Blender organ; failed writes never hidden; ArtCraft ideas (checked/not-checked, paged results, model table) |
| #7 | 11-20 | Craft apps (15) as tools: verified installs (hash + GPG), a bwrap sandbox per app |
| #8 | 11-20 | The thinking ladder (measured), token efficiency, full test pass, expectations (negotiation) |
| #9 | 11-20 | Page watches; change-only rereads |
| #10 | 11-21 | Background jobs; agreements checked against documents |
| #11 | 11-21 | Review fixes (approvals only from the human, agreements, habits that held, jobs on record, tool inputs) + `send_email` |

## 6. Honest status (the objective review of 2026-11-21)

**Solid**: durability (checked plans, no repeats, resumed after `kill -9`), crash containment, the Rust/Python
split, real-component tests (Chromium, Blender in the VM, all 15 Craft apps, email ingestion, full image boot before
each merge). **The safety model is a set of settings, not a security boundary**: the independent review (issue #22,
2026-11-21) showed the AI could get around approvals. Under the owner's operating model (§1) that's rated medium,
not critical. What was fixed after the review (CHECKLIST, "Independent review"): an "always" covers only the literal value the
human saw; only the core records that the human answered; plans can't write provenance or system records, and only
recorded outside facts can be cited; the Workspace bridge serves only the Workspace page. **Deferred by the owner's
decision (capability first)**: sandboxing "read-only" commands, a separate user for the AI, peer-authenticated
"human" on the core socket, approvals on browser actions (review C1, H4, M1–M5; issues #16, #17). Revisit if a real
problem shows up, or when connecting anything that can commit money or the business without a phone call (e.g. a
load board that books loads directly).

**Unproven** (the biggest risks, in order):
1. ~~The real Claude model has never run.~~ **Ran 2026-11-22** (issue #14): the request shape was accepted as
   written; a real `cortex` turn, a real background event and all 5 acceptance scenarios passed (TESTING.md).
   Still thin: a handful of real conversations, not days of use.
2. ~~The email extractor has never run.~~ **Ran 2026-11-22** (issue #15): its schema was refused by the API and
   fixed; then 20/20 synthetic emails right, including 6/6 phishing and injection attempts (MAIL.md). Still
   unmeasured on real broker mail.
3. **The benchmark savings with the real model**: only the ladder was run for real (the owner's choice, to save
   cost); the "before" side is still the scripted estimate (THINKING.md).
4. **Never run on real hardware**: only QEMU TCG with `-cpu SandyBridge`, no GPU. Install-to-disk, dual boot, power
   and sched_ext controls, the screen on a real GPU: untested.
5. **Nobody has used it** for real days of mail and loads. Soak tests use synthetic events.
6. **Sending email** is tested only against a stand-in SMTP server, not a real account.

**Fixed after the review** (PR #11): the AI could approve its own plans (incl. "always") via `answer_plan` →
approvals only from the human; the picture tool's `approved: true` bypass → dropped; agreements from outside info
auto-filing matches → *proposed* until the human confirms; habits learned on mere completion → only fixes that held;
jobs lost on restart → reported as lost; cut-off tool inputs → schema-checked; no outgoing mail → `send_email`
with the human's OK. Fuzz test (3,000 malformed plans) found nothing in the core.

**Crashes** (PR #13): locks recover after a panic; a panic fails only its own plan or request; you're texted about
every crash, including the core restarting unexpectedly. **Dependency audits** run in CI.

**Known limits that remain** (tracked as GitHub issues): single-user trust boundary (the AI's commands run as the
user; in a session the human approves commands, behind the scenes only read-only commands run directly, and
"read-only" is a heuristic); the live ISO has autologin and passwordless sudo (must change before install); mail
passwords' file permissions unverified; a background job's result is lost (reported) on restart; the benchmark
varies 36–40 requests with thread timing; 111 Python style lint findings and ~10 Rust warnings outstanding (none
correctness); `chrono`/`chrono-tz` marked unmaintained upstream.

## 7. What to do next (in order)

**Session 1 with the API key: done 2026-11-22** except as noted (key stored as `LINUXAI_ANTHROPIC_API_KEY` in the
cloud environment; pass it as `ANTHROPIC_API_KEY="$LINUXAI_ANTHROPIC_API_KEY"` per command, see TESTING.md):
1. `python3 tests/token_profile.py`: exact prefix and per-tool tokens (free). Fix anything the API rejects.
2. One real `cortex` turn and one real mind event; fix request-shape errors first (see §6.1).
3. `python3 tests/uat.py`: the acceptance scenarios with their token cost (a few cents each).
4. `python3 tests/thinking_bench.py --real`: ladder only, by the owner's choice (`--strategies ladder`). The full
   three-strategy run (about $10–25) is still open, if the owner wants the before/after measured.
5. Real email extraction: done on 20 realistic synthetic emails (`tests/extract_eval.py`). Real samples only if
   the owner provides them.
6. Update `docs/TESTING.md` / `docs/THINKING.md` / `docs/TOKEN_EFFICIENCY.md` with measured numbers, replacing
   assumptions.

**Later (the owner's direction, 2026-11-22): agents with sub-agents that talk to each other**, like Grok-style
bots, through the core rather than a shared context, with **a running ledger of everything decided and how**: who
proposed it, who questioned it, the evidence, the human's say, the outcome, searchable and shown in the Workspace.
Builds on what exists: plans' reasoning modules, the journal, tasks with subtasks, cases and playbooks. Intended
benefits: faster, more reliable task completion (parallel specialists with focused context), and separation of
duties as a security aid (the agent that read a stranger's message isn't the one that approves acting on it; a
checker agent reviews, as the independent review did by hand). Costs more tokens, which the owner accepts.

**Then**: install on the owner's laptop (Stage 2 in `docs/CHECKLIST.md`) and a multi-day soak on real hardware;
real SMTP sending with the owner's account (approved messages only); a local small model for triage (level 1.5);
browser steps in plans (pages that need clicking before rows show); then features again.

The disposable-VM checks the Drive orchestrator report of 2026-11-21 suggested are part of the boot test since
2026-11-22: a local TLS mail sink in the VM; a refused message never leaves, an approved one arrives; an AI-proposed
agreement stays unconfirmed until the human confirms it. Synthetic only; no live mailbox or real broker records.

## 8. How to work in this repo (process and gotchas)

- **Branch**: the session's designated branch (this session used `ccr-c72ab9c4-93xs09`). After each merge, reset it
  to `origin/main` and `git push --force-with-lease`. Push only to the designated branch.
- **Commits** end with the attribution lines the session supplies; no model identifiers in commits or PRs. PR bodies
  end with the session's attribution. The owner's email and personal details stay private.
- **Every PR**: CI (`.github/workflows`: `test` = cargo test + pytest; `iso` = image build) and the **boot test**
  must pass, then merge (owner-authorized). Docs-only PRs don't change the image.
- **Tests**: `cd core && cargo test --release` · `.venv/bin/python -m pytest -q` · `cargo clippy --all-targets` ·
  `.venv/bin/ruff check linuxai tests` (baseline 111 style findings: compare, don't chase) ·
  `python3 tests/soak.py --minutes 5` · `python3 tests/thinking_bench.py [--days 3] [--only negotiation|board]`.
- **Boot test** (≈1 h): start Docker if needed (`dockerd &`, then wait for `docker info`), build from a clean
  worktree of the exact commit: `EXTRA_CA=/root/.ccr/ca-bundle.crt ./image/build.sh`, then
  `SERIAL_LOG=<file> python3 -u image/test-boot.py out/linuxai-*.iso` → expect 37 ✓ and `PASS`.
- **Gotchas learned the hard way**:
  - Arch's Blender needs AVX: the VM uses `-cpu SandyBridge`; no Cycles denoiser in TCG; llvmpipe crashes on
    emulated AVX2; don't add softpipe (it starved Hyprland).
  - The serial shell is zsh: no multi-line heredocs (they wedge it), globs that match nothing fail (use `find`),
    job notices garble typed commands (use subshells). Its line editor redraws a typed line even with `stty -echo`
    (never type a secret with it on); with it off, output can share the prompt's line; `REPORTTIME` adds timing lines.
  - Give the VM a random-number device (`-device virtio-rng-pci`): without one, the pacman keyring setup at boot
    sometimes stalls 5+ minutes and the serial login prompt never comes.
  - In this cloud environment even "direct" HTTPS is re-terminated by an egress gateway: a VM needs the host's API
    address and the gateway's CA (`vm-bench.py --ca`). The image's AI layer is in `/opt/linuxai/lib`
    (`PYTHONPATH`, as `cortex` sets it); its SDK uses `httpx2`, not `httpx`.
  - `pkill -f <pattern>` kills your own shell when the pattern is in its command line: use
    `kill $(pidof linuxai-core)`.
  - The benchmark compresses hours into seconds: it sets `LINUXAI_HABIT_HOLD=0` and lifts the rule throttle.
  - Plans run on the core's own threads: tests that check results after events must wait for quiet, not read once.
  - The 256 MB overlay of the live ISO fills fast (Chromium profiles): `cow_spacesize=50%`, failed writes are reported.
  - Running downloaded binaries on the host is blocked by policy: run third-party apps inside the Arch container.
  - The Docker daemon can be gone after the container is recycled: start it before building.
  - A fresh container may have no QEMU: `apt-get install -y qemu-system-x86` before the boot test.
  - The cloud environment stores the Claude key as `LINUXAI_ANTHROPIC_API_KEY`: pass it per command as
    `ANTHROPIC_API_KEY="$LINUXAI_ANTHROPIC_API_KEY"`, never print it.
  - Rerun the tests after lint fixes, before committing: a clippy autofix applied after the last test run broke the
    test build once (CI run 44863232113); caught by CI before merge.

## 9. Docs to read, by need

`docs/CHECKLIST.md` (stages, the soundness pass, the review fixes) · `docs/ACTION_LANGUAGE.md` · `docs/THINKING.md`
· `docs/TOKEN_EFFICIENCY.md` · `docs/TESTING.md` · `docs/MAIL.md` · `docs/BROWSER.md` · `docs/CALENDAR.md` ·
`docs/TASKS.md` · `docs/CRAFT.md` · `docs/BLENDER.md` · `docs/SCREEN.md` · `docs/PAGES_AND_VOICE.md` ·
`docs/IMAGES.md` · `docs/SYSTEM_INTERFACE.md` · `docs/BLUEPRINT.md` / `docs/SCOPE.md` (vision) ·
`docs/RESEARCH_*.md` (research behind decisions).

## 10. Settings reference

| Variable | Default | |
|---|---|---|
| `ANTHROPIC_API_KEY` | none | the Claude API key (environment variable; never in chat) |
| `LINUXAI_MIND` | `ladder` | `full` = one fresh conversation per event (the old way) |
| `LINUXAI_MIND_BATCH` | `600` | seconds routine work waits to be batched |
| `LINUXAI_HABIT_AFTER` / `LINUXAI_HABIT_HOLD` | `1` / `900` | fixes that held before replay / seconds a situation must stay away |
| `LINUXAI_FOLD_HOURS` | `24` | a repeat of a situation within this window folds into the earlier message |
| `LINUXAI_REHEARSAL` | off | `1`: record actions, perform none (incl. sending email) |
| `LINUXAI_RULE_MAX_PER_MINUTE` | `10` | the rule loop guard |
| `LINUXAI_IMAP_*`, `LINUXAI_AUTHSERV_ID` | none | incoming mail (`/etc/linuxai/mail.env`) |
| `LINUXAI_SMTP_URL/_USER/_PASSWORD`, `LINUXAI_MAIL_FROM` | none | outgoing mail (same file; the core reads it) |
| `LINUXAI_KEEP_*` | see CHECKLIST | history retention |
| `LINUXAI_EXTRACT_MODEL` | same as thinking | which model reads email; `local:qwen3-4b` reads it on this machine (MAIL.md) |
| `LINUXAI_LOCAL_MODEL_URL` | `http://127.0.0.1:8080` | the local model server; loopback addresses only |
| `LINUXAI_MODELS_DIR` | `/var/lib/linuxai/models` | where `linuxai-core models fetch` keeps local models |
