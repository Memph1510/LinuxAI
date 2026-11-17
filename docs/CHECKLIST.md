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
- [ ] Soak test: run the daemon for days; watch memory, database size and thread count
- [ ] Prune old events and finished plans (summarize them into the AI's memory first)
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
