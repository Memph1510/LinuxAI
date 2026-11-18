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
      Not yet: sending replies
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
