# LinuxAI checklist

The rule: **the core logic has to be sound and working before features get added.** Items are done in order
within each stage.

## Stage 1: core logic (now)

- [x] Action language engine: check before running, durable plans (ask/wait survive restarts), journal with
      idempotency keys
- [x] Trust learned from conversation (approvals; "always" grants a mandate)
- [x] Linux host organ: senses (power, pressure, temperature, failed services, scheduler) and controls (power
      profile, sched_ext policy, cgroups, services, sysctl)
- [x] Rules that run without the model; `think` hands judgment back to the AI
- [x] Undecided means reach out: why / lean / follow-ups / deadline / fallback
- [x] Bootable ISO, tested in QEMU
- [x] **Soundness pass** (see below)
- [ ] Run against the real Claude model inside the image (needs an API key in the VM)
- [ ] Verify the sched_ext policy switch on real hardware
- [x] Prune old history: raw events become daily digests (counts plus notable moments) before removal;
      finished plans, old journal entries and old playbook versions expire; nothing in progress is touched.
      Defaults: events 30 days, finished plans 14 days, finished task trees 180 days, journal 365 days,
      50 playbook versions
      (`LINUXAI_KEEP_*`)
- [x] Soak harness (`tests/soak.py`): days of activity compressed into minutes, with crashes, pruning,
      invariants and resource tracking
- [x] Compressed soak runs passed: 30 minutes (82,670 events, 4,374 macro tasks, 3,487 AI plans, 9,996 human
      answers, 602 long actions, 7 kill -9 restarts) and a 10-minute rerun after expiring finished task trees.
      Memory flat at about 10 MB, 2 threads at rest, p99 response 2.5 ms, nothing stuck, no action done twice,
      database levels off at about 4 MB
- [x] Playbook protections: small edits only, rendered markdown you can edit, validation gate against past
      cases, held proposals, helped/hurt feedback ([TASKS.md](TASKS.md#playbook-protections))
- [x] Outside information is facts, never commands: outside-labeled plans must cite facts, need separate
      standing permission to act on the outside world, and show provenance from the core's records
- [x] Email organ, headless ([MAIL.md](MAIL.md)): code-decided provenance and authentication, sanitized body
      (hidden text removed), tool-less extractor into typed facts, raw mail kept out of the AI's reach.
      Sending (`send_email`): with your OK, the whole message shown, until "always" for that recipient
- [x] Reasoning modules in plans (facts, rules applied, chain, decision, downstream), shown in approvals
- [x] OS parts in Rust: the reach service and the whole email organ moved from Python into the core binary
      (`linuxai-core reach`, `linuxai-core mail`). Python is left only where the AI model is called (the mind,
      the session, `cortex extract`). Reach: 4 MB of memory in Rust vs 75 MB for a Python process
- [ ] Later: port the model-facing layer (mind, session, extractor) to Rust once its behavior settles
- [ ] Multi-day soak on the real laptop (same harness, `--minutes 4320`)
- [x] Second look before acting: assessments required; unknowns, low confidence and first-time situations go
      to the human; first-time situations need research
- [x] **Tasks, micro and macro** (see [TASKS.md](TASKS.md)), plus category **playbooks** (markdown, editable,
      with history) and **cases** (situations with their variables, for learning by repetition): you hand the OS a task of any size. It breaks big ones into subtasks
      (run by itself or by subagents), tracks what depends on what, and coordinates the parts that need a
      human: an **action** (something only you can do, like signing a paper or making a call), an
      **evaluation** (is this result good?), a **critique** (how would you improve it?), or a **decision**.
      It reaches you for each one with context, and continues as soon as you respond. The interface stays
      mostly invisible: you see a task's state only when you look or when you're needed.

Deliberately **not** planned: separating the human's commands from the AI's. The OS is one mind that you
steer, subagents included. Security hardening waits until something sensitive is actually at risk.

## Stage 2: install

- [ ] Install to disk from the live ISO (keeps what it learns)
- [ ] Dual boot alongside Windows on the laptop

## Stage 3: the Workspace screen

- [ ] Plans carry their reasoning and decisions as data (options, scores, evidence, applied principles)
- [ ] Comments on any step; re-evaluation with an impact preview; apply / branch / discard; lessons become
      principles
- [ ] The Workspace as the LinuxAI shell (full-screen UI on a minimal Wayland compositor)

## Stage 4: the LinuxAI Android app

- [ ] Two-way messaging with the AI: questions arrive with their reasons, and you reply from the phone
- [ ] Undecided items, follow-ups and fallbacks on the phone; replies continue the plan
- [ ] The Workspace views on the phone
- [ ] Later: become the phone's SMS app

## Later

- Email (imported mailbox), calendar, files as memory, bills and budgets, the freight-broker workflow
- Always-on listener (tablet server or small cloud server)
- Own email domain, local models, hardening
- Agents with sub-agents that talk through the core, and a running ledger of every decision and how it was made
  (proposer, objections, evidence, the human's say, outcome); a checker agent for plans resting on outside
  information. Execution over token cost (see HANDOFF §7)

---

## Soundness pass (done)

Each flaw was first shown by a failing test, then fixed:

| Flaw | Fix |
|---|---|
| A long action (e.g. a 5-minute command) froze the whole core: no events, no rules, no answers | Memory is safe to use from many threads; plans run independently and are claimed one at a time; event- and timer-triggered work runs on its own threads |
| "No, not always" counted as **yes, always**, and granted trust | Replies are read strictly: anything that isn't clearly a yes is a no, and "always" only counts after a yes |
| Steps the AI added later (after `think`) skipped checking and trust | They're checked like any plan; untrusted actions go to the human first |
| Rules written by the AI ran immediately, with full authority | AI-made rules start off; the human is asked to turn them on |
| A plan interrupted by a crash or reboot was never resumed | On start, the core resumes every plan that was mid-way, without repeating completed actions |
| A plan wasn't saved until its first step finished, so a crash during step 1 lost it | Plans are saved before they start |
| A rule that triggered itself looped | Rules are throttled at 10 firings a minute, with a warning event |
| The same plan could be advanced by two threads at once | One thread per plan at a time |

Verified by: 21 core tests, including 200 plans with answers and events running concurrently from 8 threads,
and 32 Python tests, including **killing the real daemon with `kill -9` in the middle of a plan and restarting
it**. Then the rebuilt ISO is boot-tested in QEMU.

## Review fixes (2026-11-21)

From an objective review of everything built so far. Each was first shown by a failing or new test.

| Gap | Fix |
|---|---|
| The AI could answer any plan's question, including an approval, so it could grant itself trust ("always") | The core records who answered. Approvals come only from the human directly (screen, text, prompt); the AI can relay an ordinary answer, and the plan knows it did |
| The picture tool let the AI send `approved: true` itself, skipping the monthly budget question | The AI's `approved` is dropped; only the human's own yes sets it |
| An agreement could be recorded from an email with no human involved, and a matching invoice was then filed automatically | Agreements the human didn't make are only proposed; matches against them aren't filed until the human confirms |
| A habit was learned when a plan merely finished, worked or not | Learned only once the fix held: its own check passed, or the situation stayed away 15 minutes |
| A background job's result was lost on a restart, and its plan waited out its time limit | Jobs are on record; after a restart, ones cut off are reported as lost at once |
| A cut-off or malformed tool input could run | Inputs are checked against each tool's definition first; nothing runs if they don't fit |
| No outgoing email: negotiations and agreements couldn't finish | `send_email`, with the human's OK and the whole message shown, until "always" for that recipient |

Also added: a fuzz test (3,000 random and malformed plans through the checker and the core, and plans at the
socket's maximum depth). Nothing crashed.

### Independent review (2026-11-21, issue #22) and the owner's decision

The review found ways around approvals. The owner's operating model (HANDOFF §1: LinuxAI has its own dedicated
accounts, logins and temporary card; broker deals confirmed by phone; capability first) puts them at medium. Fixed,
adding no new questions for the human:

| Review | Gap | Fix |
|---|---|---|
| H1 | "Always" on a plan whose target was a variable (`$p`, `$broker`) covered any later value | "Always" is recorded only for a literal target, the value the human saw; variable/pattern mandates are ignored |
| H2 | A plan could set `answered_by: "human"` and make its agreement look like the human's | Only the core records that the human answered (`Exec.human_answered`, set in `answer`) |
| H3 | A plan could `store` a record with forged provenance ("sender verified") and cite it as an outside fact | `store` can't write provenance or system records; only records labelled outside by the core can be cited |
| H5 | The Workspace's answer-as-human bridge could be put on a page someone names | It serves only the configured Workspace page |

Deferred by the owner (capability first; fix blockers as met): C1 (sandbox for "read-only" commands, separate AI
user, peer-authenticated socket), H4 (approvals on browser actions), M1–M5, L1–L4. Tracked in issue #22.

### Crashes and dependencies (2026-11-21, from "how are bugs, crashes and security handled?")

| Gap | Fix |
|---|---|
| A panic while holding a lock "poisoned" it, so every later request touching that lock panicked too: one bug could take the whole core down | Every lock recovers after a panic (`crash::Locked`); the data behind them stays consistent |
| A panic while running a plan left it claimed and "running" forever | The plan fails on its own with "internal error", is saved and freed; everything else carries on |
| A panic answering a request dropped the connection with no answer | The caller gets an error, and the core keeps serving |
| Crashes were only in the journal | You're texted about each one (`linuxai.crash`): caught panics, panics in organs (browser, Craft, Workspace…), and the core restarting after an unexpected stop (systemd's restart count) |
| The dependency audit ran by hand | `cargo audit` and `pip-audit` run in CI on every push; a vulnerability fails the build |

## Next pieces (decided 2026-11-18; any order; install to disk waits until you're at the laptop)

- [x] **Prompt library** (`~/prompts/`, core `src/prompts.rs`, starter set in `library/prompts/`): markdown prompts by use case, with fill-in slots, which model they suit,
      helped/hurt counts, searchable. You edit them directly; the AI suggests changes, which are held for you.
      Starts with a starter set across freight, email, research, summaries, images and tasks.
- [x] **Pictures** (`linuxai-core image`, core `src/images.rs`; see [IMAGES.md](IMAGES.md)): the OS detects what the
      machine can do and picks how: precise edits always local (ImageMagick); making or changing a picture from a
      description on a local graphics-card model, else a paid service within your monthly budget (asks past it),
      else the CPU. Falls back when one fails. Every picture recorded with what made it and the cost. Adapters for
      ComfyUI, Gemini ("Nano Banana") and stable-diffusion.cpp.
- [ ] Pictures: the photo view with comments; try the real backends on the laptop; set up ComfyUI and a model
      automatically when the card can run one.
- [x] **Shared browser** (`linuxai-core browser`, core `src/browser.rs`; see [BROWSER.md](BROWSER.md)): one
      real Chromium, headless when there's no screen, that you and the AI use at the same time. Private pipe, no
      port. Per-tab owners: the AI reads any tab and acts only in its own (in its own window); tabs are handed back
      and forth, and a handoff texts you. Pages are outside information.
- [ ] Browser: live view for the Workspace screen and the phone; browser steps in the action language.
- [x] **Voice and talking about what's on screen** ([PAGES_AND_VOICE.md](PAGES_AND_VOICE.md)): push to talk with
      local speech to text (whisper.cpp) and speaking (espeak-ng, Piper or any command); the browser knows what's
      under your pointer (look), highlights and magnifies what the AI talks about (point), and pulls things up in
      front of you while you talk (show).
- [ ] Voice: try with a real microphone on the laptop; natural voice (Piper) on the image; wake word.
- [x] **The screen** ([SCREEN.md](SCREEN.md)): Hyprland with a Quickshell status line, the compositor organ
      (windows, focus, pointer, arranging, effects off on battery), falling back to the text session where
      it can't draw.
- [x] Quickshell: the AI's questions as cards you tap, the voice indicator, its pointer over any app; the AI's
      `screen` tool (windows, what you're in, screenshots, pointing, arranging).
- [x] Magnifier over any app: a lens beside the area, twice as large; open questions say where to answer.
- [x] **The Workspace (the book)**: the main screen in the shared browser's first window, connected to the OS by a
      private channel; what needs you, the work, steps, activity, and web pages live inside it (click, scroll, type).
- [x] Workspace layout: the main window (70%); the AI's page windows out of sight, their pages inside the Workspace.
- [ ] Workspace: the reasoning view and comments from the mock-up; the AI's pointer inside the page picture.

### To decide later: ideas mined from similar projects ([RESEARCH_SIMILAR_PROJECTS.md](RESEARCH_SIMILAR_PROJECTS.md))

Not decided yet. Each is an option to discuss, not planned work.

- Undo for what the AI changes: a filesystem snapshot before each plan that changes the system (installed system)
- Heartbeat checklist: a markdown "keep an eye on" list the mind checks now and then
- Your messaging apps (Telegram, WhatsApp, Signal) as two-way reach channels
- "Show me how" mode: a ghost cursor and voice guide you instead of doing it
- Hold-to-talk hotkey; wake word
- Local vision model for private or offline screen reading
- Measure how often it asks you, and make it go down

### From the Storytold / ArtCraft repositories ([RESEARCH_STORYTOLD.md](RESEARCH_STORYTOLD.md))

- [x] Finished work says what was checked and what wasn't (core-enforced for the AI; shown in the Workspace)
- [x] Long results come as a summary and pages, never silently cut
- [x] One table of which model does which job, yours to set
- [x] Staged copy then verified export: the rule for every file-editing organ
- [x] Scene-first picture prompt
- [x] The 15 Craft apps as tools the AI uses to finish tasks ([CRAFT.md](CRAFT.md)): verified installs on first use, a sandbox for every app, results kept beside originals
- [x] Offline background removal and subject selection (PhotoCraft)
- [ ] Later: an MCP surface for the verb registry

### To decide later: creative applications ([CREATIVE_TOOLS.md](CREATIVE_TOOLS.md))

- Whether live bridges may change the human's open scene while they work in it
- Unity and Unreal accounts and licences: a setup task the AI walks you through, or yours alone

### Blender ([BLENDER.md](BLENDER.md))

- [x] Built into the image (decided: keep it built in; +1 GB on the ISO, +2.3 GB installed)
- [x] Headless jobs on staged copies: build, change, reopen and compare, preview and check, render, export
- [x] Scripts inside files never run; Python the AI writes needs your OK
- [x] Live bridge in your open Blender: see what's selected; change it only when asked, as one undo step
- [ ] Long renders as background tasks with progress
- [ ] Plan verbs for Blender steps
- [ ] UV/texturing ops, geometry nodes

### Calendar ([CALENDAR.md](CALENDAR.md))

- [x] Events, repeats, free time, conflicts; invitations from email wait for you; subscribed calendars read-only
- [x] Part of the AI, not an app: kept from conversation, in the AI's context every turn; reminders by text
- [ ] Reply to the organizer by email when you answer an invitation
- [ ] Two-way sync with calendar servers (CalDAV)
- [ ] Exceptions to repeating events (EXDATE, moved occurrences)

### Open: should the OS's own prompts live in the prompt library?

The prompts the OS runs on are things like the email extractor's instructions and the mind's planning and
reflection prompts.

| | In the library (editable) | Built in |
|---|---|---|
| For | You can read exactly how the OS thinks and adjust it; one place for all prompts; improvements by the AI go through the same review; nothing hidden | Stable: an edit can't silently break core behavior; tested together with the code; simpler |
| Against | An edit can break things (the extractor's form, the safety wording about outside information); needs versioning and a way back | Opaque to you; changing how the OS thinks needs a code change |

A middle path: OS prompts are **readable** in the library (marked "system") with their history; editing one
needs your explicit OK, and protected parts (the rules about outside information and the forms code depends on)
stay locked in code. Decide later.
