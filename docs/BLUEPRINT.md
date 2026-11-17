# LinuxAI Blueprint: an AI-Native Operating System

> **Core idea:** a persistent AI mind runs as the system's policy authority. Linux is its body.
> The GUI, the terminal and the file views are implants that let a human step inside, watch it and steer it.

This document turns that idea into a system you can build. It makes one correction to the idea up front, because
the rest of the design depends on it:

> **The AI is the control plane, not the data plane.**
> An LLM gives you roughly 20–150 tokens/s, with time-to-first-token from 100 ms to several seconds, and its output
> is not deterministic. A kernel scheduler makes decisions in microseconds, and a compositor frame takes 16 ms. If
> the model sat in the hot path of interrupts, keystrokes, frames or syscalls, the system would be unusable and
> unsafe. So the model sets *policy, intent and plans*. Deterministic machinery (the Linux kernel, systemd, the
> compositor, and rules the AI compiles itself) carries them out. "AI as kernel" means the AI holds *cognitive
> authority*: it decides what should happen. It does not mean the AI executes every instruction.

---

## 1. System Architecture & Stack

### 1.1 Layer model

```
 ┌──────────────────────────────────────────────────────────────────────────────┐
 │  HUMAN IMPLANTS (Wayland clients + TTY)                                      │
 │  Thought HUD · Gate · Intent Bar · Memory Browser · Terminal · Timeline      │
 └───────────────▲───────────────────────────────┬──────────────────────────────┘
                 │ cortex-api (subscribe/query)  │ intents / overrides / approvals
 ┌───────────────┴───────────────────────────────▼──────────────────────────────┐
 │  COGNITIVE KERNEL  (uid: cortex, NOT root, can be frozen/killed at any time) │
 │  ┌───────────┐ ┌──────────────┐ ┌─────────────┐ ┌────────────┐ ┌──────────┐  │
 │  │ Attention │ │ Task Engine  │ │ Inference   │ │ Memory     │ │ Skill    │  │
 │  │ Scheduler │ │ (durable SM) │ │ Router      │ │ Subsystem  │ │ Compiler │  │
 │  └─────▲─────┘ └──────┬───────┘ └──────┬──────┘ └────────────┘ └────┬─────┘  │
 │        │ events       │ typed actions  │ local ⇄ cloud              │ rules   │
 └────────┼──────────────┼────────────────┼────────────────────────────┼────────┘
          │              │                │                            │
 ┌────────┴──────┐ ┌─────▼────────────────────────────────────────────▼────────┐
 │ EVENT BUS     │ │  WARDEN  (small, deterministic Rust, the trust boundary)  │
 │ append-only   │ │  policy eval (Cedar) · capability tokens · risk tiers ·   │
 │ log + pub/sub │ │  approval gates · hash-chained audit · secret injection   │
 └───────▲───────┘ └─────┬─────────────────────────────────────────────────────┘
         │               │ executes ONLY via sandboxed MCP servers / transient units
 ┌───────┴───────────────▼────────────────────────────────────────────────────────┐
 │  MOTOR LAYER: MCP tool servers, each its own sandboxed systemd unit            │
 │  fs · systemd · pkg · net · compositor · a11y(AT-SPI) · browser · mail · shell │
 └───────────────────────────────┬────────────────────────────────────────────────┘
 ┌───────────────────────────────▼────────────────────────────────────────────────┐
 │  LINUX SUBSTRATE (HAL): kernel · systemd · D-Bus · udev · PipeWire · NM ·      │
 │  wlroots/Smithay compositor · btrfs · nftables · TPM2 · Landlock · BPF-LSM     │
 └────────────────────────────────────────────────────────────────────────────────┘
```

The most important boundary in the design: **the cortex (the AI) is not in its own trusted computing base.** The
Warden is a small, auditable, deterministic process. Every effect on the world passes through it. The AI proposes
and the Warden disposes.

### 1.2 Stripping Linux down to a HAL

**Kernel.** Don't build a custom kernel yet. Use the stock Arch `linux` (or `linux-zen`) and confirm these are enabled:

| Feature | Why the cortex needs it |
|---|---|
| cgroup v2 + freezer | Per-task resource budgets; instant freeze as a kill switch (`systemctl freeze`) |
| BPF LSM, eBPF, `CONFIG_BPF_SYSCALL` | Cheap telemetry (exec/connect/open) that becomes events without polling; extra enforcement hooks |
| Landlock | Unprivileged fs/net sandboxing for tool processes |
| fanotify (`FAN_REPORT_FID`) | Whole-filesystem change stream for the memory indexer |
| user namespaces, seccomp | Sandboxing for tool servers and untrusted code execution |
| TPM2, IMA (optional) | Measured boot, sealed secrets, signing the audit-log anchor |
| io_uring | Low-overhead I/O for the indexer and event bus |

You can swap in a custom kernel config later (Phase 3) to trim drivers and boot time. It is not on the critical path.

**Init: keep systemd. It is the motor cortex.** The AI should never `fork()`/`exec()` processes itself. It asks the
Warden, and the Warden creates **transient units** over D-Bus
(`org.freedesktop.systemd1.Manager.StartTransientUnit`) with sandbox properties attached. That gives you, for free:
cgroup accounting, resource limits, sandboxing, journald logs per action, clean teardown, and one name for
everything the AI ever started (`cortex-act-<uuid>.service`).

**Userland, kept to a minimum:**

| Keep | Drop |
|---|---|
| systemd, journald, udev, D-Bus (dbus-broker) | Display manager (use `greetd`, which launches straight into the cortex session) |
| PipeWire + WirePlumber | Desktop environment (GNOME/KDE), file manager, panels, tray |
| NetworkManager (has a full D-Bus API) | Anything driven only by a GUI that has no API |
| btrfs (snapshots = undo), nftables, TPM2 tools | Polkit rules that let a session become root without asking |
| A compositor (see below), AT-SPI2 bus | Xorg (Xwayland only on demand) |

**Compositor.** The compositor is the human's sensory interface, so the AI has to be able to see and arrange its
scene graph.

- **Prototype (Phases 1–2):** Sway or Hyprland, driven over their IPC sockets. Implants are `wlr-layer-shell`
  surfaces. Window management goes through IPC commands. The AI watches window and focus events through
  `wlr-foreign-toplevel-management`.
- **Product (Phase 3):** your own compositor on **Smithay** (Rust) or wlroots. Reasons: (a) the kill-switch hotkey and
  approval dialogs need to sit in a privileged layer that no client, including the cortex, can spoof or cover;
  (b) the scene graph can be exposed straight to the cortex as structured data; (c) "rooms" (task-scoped workspaces)
  become a first-class concept.
- **Perception:** give the AI **AT-SPI2** (the accessibility tree) as its main way to "see" apps. It is structured,
  cheap and exact. Screenshots plus a vision model are the fallback: slow, expensive, and they leak whatever happens to
  be on screen.

**Image strategy.** An AI that changes system state needs **atomic rollback**. Build the distro image-based: an
`mkosi` build from Arch packages, a read-only `/usr` with A/B updates through `systemd-sysupdate`, `/etc` overlays,
and `/var` + `/home` on btrfs with automatic snapshots. (NixOS is a strong alternative. "AI edits a declarative config
and switches generations" is a very clean model. The trade-off is Nix's learning curve and the gap from your Arch
preference.)

### 1.3 The cognitive kernel (`cortexd`) in practice

`cortexd` is a long-running daemon (`cortex.service`, `Type=notify`, `WatchdogSec=`), plus the separate
`warden.service`. Inside, it has six subsystems.

#### (a) Event bus: the nervous system

- **Implementation:** an append-only event log in SQLite (WAL mode) plus in-process pub/sub, exposed over a Unix
  socket. (Move to NATS JetStream only if you go multi-machine.) The log *is* episodic memory and the raw input to
  the audit trail. Every perception and action is one row.
- **Sensors (adapters that publish to the bus):** sd-journal cursor follower, udev monitor, D-Bus signal match
  (NetworkManager, UPower, logind, systemd unit state), fanotify, eBPF ring buffer (exec/connect), compositor IPC
  (focus, window open/close), timers (systemd `.timer` or internal cron), the user's intent channel, inbound
  webhooks/mail.
- **Event schema:**

```jsonc
{
  "id": "01JD…",               // ULID: sortable, unique
  "ts": "2026-11-17T09:14:03.120Z",
  "source": "udev",            // sensor name
  "kind": "device.added",
  "subject": "usb:1050:0407",  // entity key → links into graph memory
  "payload": { "...": "..." },
  "taint": "trusted",          // trusted | user | external | untrusted (see §3.6)
  "privacy": "local-only"      // local-only | cloud-ok
}
```

#### (b) Attention scheduler: what the mind thinks about

The scheduler is the real answer to "the AI as the main OS loop." It is a priority queue with **salience scoring**
and **budgets**, not a polling loop that runs the LLM.

1. **Reflex filter (no LLM, < 1 ms):** most events match a compiled rule (see (f)) or are dropped as noise. Target:
   more than 95% of events never reach a model.
2. **Salience (small model or heuristics, < 50 ms):** score novelty × relevance to active goals × urgency. Only events
   above a threshold become *thoughts*.
3. **Budgets:** tokens/min, cloud $/day, GPU-seconds, and *human interrupts per hour*, which is the scarcest
   resource of the lot. When a budget runs out, work degrades to lower tiers or waits in a queue. It never stalls the
   machine.
4. **Preemption:** user intent always preempts background cognition. Background tasks run at `Nice=19`,
   `CPUWeight=20` inside `cortex-bg.slice`.

#### (c) Task engine: durable thought

Each unit of work is a **persistent state machine** (`pending → planning → awaiting_gate → executing → verifying →
done/failed/rolled_back`) saved in SQLite after every step. This is durable execution, a small home-grown version of
Temporal. If `cortexd` crashes, gets frozen or the machine reboots, tasks pick up where they stopped. Sub-agents are
child tasks with their own scoped capability set and budget. That gives you a process tree for thoughts.

#### (d) Inference router: local ⇄ cloud hybrid

All model access goes through one router. The rest of the cortex asks for a capability class, never a specific model.

| Tier | Model class | Where it runs | Latency | Used for |
|---|---|---|---|---|
| R0 Reflex | none (rules, regex, compiled skills) | CPU | µs–ms | most events |
| R1 Fast | 1–4B instruct, quantized, always resident | CPU / iGPU / NPU | 50–300 ms | classify, route, salience, extraction, short tool calls |
| R2 Local deliberate | 8–32B (dense or MoE), loaded on demand | dGPU | 1–10 s | multi-step planning on private data |
| R3 Cloud | Claude (`claude-haiku-5-5` → `claude-sonnet-5-5` → `claude-opus-5-5`) | API | 1–30 s | hard planning, long context, code, ambiguity |

Routing inputs: task class, required context size, **privacy label of every item in context** (any `local-only` item
pins the task to R1/R2), R1's self-reported confidence, budget left, and whether the network is up. Escalate when
schema validation of the output fails, when confidence is low, or when a verifier disagrees.

Serving: `llama.cpp` server (or vLLM if you have the VRAM) behind one OpenAI-compatible endpoint, each model in its
own unit in `inference.slice` with `MemoryHigh=` and GPU arbitration. Latency tricks that matter:

- **Persistent KV/prefix caches.** The system prompt, tool schemas and a "current world state" header are large and
  rarely change. Cache them locally (llama.cpp slot save/restore) and use prompt caching for cloud calls.
  Time-to-first-token on repeat calls can drop by roughly an order of magnitude.
- **Speculative pre-thinking.** While the user is typing into the Intent Bar, R1 classifies the partial input and
  pre-fetches memory, so the context is already built when they hit enter.
- **Structured output** (grammar-constrained decoding locally, tool-use schemas in the cloud). The model emits typed
  actions, never shell strings to `eval`.
- **VRAM residency policy.** R2 gets unloaded when a game or render job asks for the GPU (the compositor and cgroup
  signals say so). The cortex has to give up the body's resources to the human.

#### (e) Memory subsystem: one store, several views

Use one SQLite database family as the source of truth. That keeps the data portable, backupable and inspectable.

| Store | Tech | Contents |
|---|---|---|
| Episodic | the event log + task log | what happened, when, why |
| Semantic | `sqlite-vec` (or LanceDB) + a local embedding model | chunks of files, notes, mail, pages; for retrieval |
| Graph | SQLite `entities`/`edges` tables + recursive CTEs | people, projects, devices, files, services and how they relate |
| Declarative / human-readable | **Markdown vault in git** (`~/.cortex/mind/`) | the AI's beliefs, preferences, procedures, project notes |
| Artifacts | the normal POSIX filesystem | real files, untouched, indexed through fanotify |

The Markdown vault matters most. It is the **canonical, human-editable** form of anything the AI "believes"
(for example "User prefers X", "Backups run nightly to Y"). Vectors and graph rows are *derived indexes* you can rebuild
from it. When you edit a note, you have edited the AI's mind, and git diffs show you how its beliefs changed over time.

**Context assembly** is its own component. For each model call it builds a context within a token budget:
system/world header (cached) → active task state → retrieved memory (hybrid BM25 + vector, re-ranked) → recent events.
Every item keeps its `taint` and `privacy` labels into the prompt and out the other side.

#### (f) Skill compiler: how the mind gets faster

This is the main fix for latency and cost. When the cortex solves a *recurring* trigger with an LLM (for example
"a USB drive with label `BACKUP` is inserted → mount it read-only and run restic"), it writes a **deterministic
skill**: an event pattern plus an action plan with typed parameters, stored as a reviewed file. After a human
approves it (or after N successful supervised runs, for low-risk tiers), the skill moves into the R0 reflex layer.
The system starts out slow and smart and becomes fast and smart: a learned set of reflexes that you can inspect and
undo.

```yaml
# ~/.cortex/mind/skills/backup-on-usb.yaml   (proposed by cortex, approved by human)
id: backup-on-usb
trigger: { kind: device.added, match: { fs_label: BACKUP } }
risk_tier: T1
actions:
  - tool: fs.mount        { device: "$event.devnode", mode: ro, target: /run/media/backup }
  - tool: systemd.start   { unit: restic-backup.service }
verify:  { event: unit.state, unit: restic-backup.service, expect: inactive(success), within: 2h }
on_fail: escalate        # hand to R2/R3 with the failure context
approved_by: human@fido2:2026-11-17T09:20Z
```

#### (g) Tools: MCP everywhere

Every capability is an **MCP server**, and each one runs as its own systemd unit with its own uid, sandbox and
network allowlist. The cortex never talks to tool servers directly. It sends a typed `ActionRequest` to the Warden.
The Warden checks policy, attaches any secrets, and forwards the call over the MCP stdio/socket transport. That gives
you a uniform tool interface the models already understand, and an enforcement point the AI can't get around.

Starter tool servers: `fs` (read/search/write with snapshot), `systemd` (unit control, transient units), `pkg`
(pacman/flatpak, plan-only by default), `net` (NetworkManager), `wm` (compositor IPC), `a11y` (AT-SPI read/act),
`shell` (runs commands inside a throwaway sandbox unit), `browser` (headless Chromium, `untrusted` taint),
`notes` (Markdown vault), `mail`/`calendar`.

---

## 2. Designing the Human Implants

Design rule: **the human never manages the system. The human inspects, steers and overrides the AI that does.** Every
implant is a client of one **cortex-api** (JSON-RPC over `$XDG_RUNTIME_DIR/cortex.sock`, with subscription streams).
None of them has privileges of its own. Approval UI is the exception: it is drawn by the Warden through a privileged
compositor layer.

| Implant | Surface | What it shows / does |
|---|---|---|
| **Thought Stream HUD** | `wlr-layer-shell` overlay, top layer; collapses to a 1-line status strip | Live task tree: current goal, step, tool call in flight, model tier, tokens/$ spent, confidence. Click a node to open its full reasoning and context ("why did you do that?"). Colour shows risk tier and taint. |
| **Gate** (approval) | Privileged compositor layer, **cannot be drawn or covered by any client, the cortex included** | A proposed action as a *diff*: files changed, commands, recipients, network destinations, data leaving the machine. Shows provenance ("this request came from text in an email from X"). Buttons: Approve · Deny · Edit · Approve-and-make-a-rule (scoped). T3 requires a FIDO2 touch. |
| **Intent Bar** | Global hotkey palette (Super+Space) | Type or speak intent ("get this repo building and tell me what broke"). Also accepts raw shell (`!cmd`), which skips the AI entirely. Shows a speculative plan preview before you commit. |
| **Memory Browser** | Full app | Replaces the folder tree as the *main* navigation. Facets: time (timeline), entities (graph), projects (collections the AI curates), semantic search, "what was I doing Tuesday". The real POSIX tree stays available as one view, never removed. Edit beliefs (Markdown) inline. |
| **Rooms** (workspaces) | Compositor | One task = one room. The AI opens and arranges the apps, terminals and docs a task needs and restores the room when you come back. Pinning a window or a layout is an override the AI must keep. |
| **Terminal** | Real PTY (foot/wezterm) + shell integration | A plain bash/zsh **running as your uid, outside cortex supervision**. It is the manual-override implant that always works. With opt-in, OSC 133 shell-integration marks feed commands and exit codes to the bus as `user`-taint events, so the AI learns from what you do and can suggest commands inline (ghost text). It never runs them itself. |
| **Timeline / Scrubber** | Full app | Hash-chained audit log as a scrubbable timeline. Each action links to its snapshot and inverse: **Undo this**, **Undo everything since**, **Replay with a different model**. |
| **Agent Inspector** | Full app / HUD drill-down | Sub-agent process tree with capabilities, budgets and taint per agent. Per-agent freeze/kill/revoke. Like `htop`, but for thoughts. |
| **Recovery console** | `brain-off.target` on a TTY | The system with no cortex at all: a normal Linux shell. Always reachable from the boot menu. |

UX principles that make "living inside the AI" bearable:

1. **Ambient by default, modal only for gates.** The AI spends a *human-interrupt budget*. It batches low-urgency
   questions into a digest instead of drip-feeding notifications.
2. **Every AI-made artifact is labelled.** Files, windows, messages and commits the cortex produced carry provenance
   (xattrs `user.cortex.task=<id>`, a window border tint, a commit trailer).
3. **Overrides are sticky, and they teach.** A manual change (moving a window, editing a file the AI manages,
   denying a gate) becomes a constraint the AI must keep, and a candidate preference note in the Markdown vault.
4. **Latency gets hidden honestly.** Show "thinking (R3, ~8 s)" with a cancel button rather than a spinner. Never
   block input on cognition.

---

## 3. Security, Autonomy & Guardrails

The threat model has three adversaries: (1) **external content that injects instructions** (web pages, mail, files,
repo READMEs), which is the main one; (2) **the model being wrong** (hallucinated paths, `rm` on the wrong
directory); (3) **classic attackers** going after a highly privileged daemon. The design assumes the cortex
*will* be compromised or confused sometimes, and limits what that can cost.

### 3.1 The AI is not root

- `cortexd` runs as uid `cortex`. No sudo, no polkit admin rule, `NoNewPrivileges=yes`, and an empty
  `CapabilityBoundingSet=`.
- Every effect goes to **`warden`** over a Unix socket. The Warden authenticates the caller with `SO_PEERCRED` and
  checks policy. Only the Warden (with narrowly scoped privileges) can create transient units, use tool-server
  credentials, or talk to the compositor's privileged layer.
- The Warden is small (target: < 10k lines of Rust), deterministic, contains **no LLM**, and is the only component
  that needs a careful security review. It can only be updated through a signed system update. The cortex can't
  change it.

### 3.2 Capabilities and policy

Actions are typed requests checked against **Cedar** policies. Cedar is a good fit here: it is fast, its policies can
be analysed, it has a Rust crate, and it is readable by humans.

```cedar
// Tier T0: reads inside the home dir are auto-approved unless the path is in the secrets set
permit (principal == Agent::"cortex", action == Action::"fs.read", resource)
when { resource.path like "/home/me/*" }
unless { resource in PathSet::"secrets" };

// Anything whose context includes untrusted content cannot send data off the machine without a human
forbid (principal, action in ActionGroup::"egress", resource)
when { context.max_taint == "untrusted" }
unless { context.human_approved == true };
```

Every action is assigned a **risk tier**:

| Tier | Examples | Gate |
|---|---|---|
| **T0** observe | read files, query state, search | auto, logged |
| **T1** reversible, local | write in a snapshotted subvolume, start/stop user units, arrange windows | auto, snapshot first, undoable |
| **T2** reversible, impactful | install packages, change system config, bulk file moves | soft confirm in the HUD (or a skill pre-approved for it) |
| **T3** irreversible / external | send mail or messages, push to git remotes, payments, delete without a snapshot, send private data to the cloud, add credentials | **Gate + FIDO2 touch** (WebAuthn/`libfido2`) |
| **T4** forbidden to the AI | change the Warden, policies, audit log, boot chain, secure-boot keys, disk encryption, its own capability grants | impossible over the API; only a human at the recovery console |

Capabilities are short-lived tokens `(tool, verbs, resource scope, expiry, max_taint)` issued per task, so a
sub-agent asked to "summarise these PDFs" literally holds no `mail.send` capability.

### 3.3 Sandboxing the actions themselves

Each action runs as a transient unit with sandbox properties picked by tool and tier:

```ini
# what warden passes to StartTransientUnit for a shell.exec action (T1)
User=cortex-act
ProtectSystem=strict
ProtectHome=tmpfs
BindPaths=/home/me/projects/foo           # only the task's scope
PrivateNetwork=yes                        # unless the capability grants net
NoNewPrivileges=yes
SystemCallFilter=@system-service
RestrictAddressFamilies=AF_UNIX
MemoryMax=2G
CPUQuota=200%
RuntimeMaxSec=600
```

Network egress per tool is enforced with nftables (`meta skuid`/cgroup socket matching) on top of the Warden's
policy. The cortex's own route to the cloud LLM goes through a **DLP egress proxy** that checks every outgoing prompt
for `local-only` labels and refuses to send them.

### 3.4 Secrets

The AI **never sees secret values.** Secrets live in `systemd-creds` sealed to the TPM. The Warden injects them into
the tool server's environment or request at execution time (the same pattern as a credential-injecting proxy). The
model sees `$CRED{github}` handles only, so a prompt injection can't extract a token the model has never had.

### 3.5 Reversibility and audit

- **Snapshots:** before any T1/T2 filesystem action, the Warden takes a btrfs snapshot of the affected subvolume
  (cheap, about 1 ms). Snapper handles retention.
- **Action journal:** each executed action records its inverse where one exists (unit started → stop; file moved →
  move back; package installed → snapshot ID to roll back to).
- **Audit log:** append-only, **hash-chained** (each record holds the SHA-256 of the one before it), written *only*
  by the Warden to a subvolume the cortex can't write to (`chattr +a`, separate uid). Every N minutes the chain head
  is signed with a TPM-resident key, so tampering shows up even after a full compromise of the cortex.

### 3.6 Prompt injection: taint tracking and a dual-model split

This is the part most likely to go wrong, so it gets real structure rather than "the system prompt says to ignore
instructions in data":

- Every piece of data carries a **taint label** from source to sink: `trusted` (policy, the human's own vault) >
  `user` (things you typed) > `external` (data from authenticated services) > `untrusted` (web, inbound mail,
  downloaded files).
- A task's **effective taint** is the worst label in its context. The policy engine uses it (see the `forbid` rule
  above). Untrusted context + egress or a T3 action = always a human gate. **No exceptions, and no skill can
  pre-approve it.**
- **Dual-model split** (after Simon Willison's dual-LLM pattern and DeepMind's CaMeL): the *privileged planner* never
  reads raw untrusted text. A *quarantined* model reads it and returns **typed values only** (for example
  `{sender: Email, dates: [Date], summary_ref: Handle}`). The planner writes a plan over handles. The Warden resolves
  handles at execution time and the taint goes with them.

### 3.7 Kill switches, from soft to hard

| Switch | Mechanism | Effect | Who can trigger it |
|---|---|---|---|
| **Pause** | Hotkey (Super+Esc) handled **inside the compositor**, not the cortex → `systemctl freeze cortex.slice` | cgroup freezer: every AI process stops mid-instruction, state kept; resume with the same key | Human, instant |
| **Revoke** | Warden drops every outstanding capability token | AI keeps thinking but can't act (read-only mode) | Human, HUD/hotkey |
| **Sever** | nftables: drop all traffic for uids `cortex*` | No cloud models, no external tools | Human; also automatic when the budget is exceeded |
| **Kill** | `systemctl stop cortex.slice inference.slice` | Mind off, body still running; you are on plain Linux | Human |
| **Brain-off boot** | `brain-off.target` in the boot menu (UKI cmdline) | Boots with no cortex units at all | Physical presence |
| **Dead-man / tripwires** | Warden watches for policy-violation spikes, budget overruns, repeated denied requests, audit-chain errors | Automatic *Pause* + a HUD alert | Automatic |

The kill switch lives in the compositor and the Warden so that a confused or compromised cortex can't intercept it,
and **freeze is preferred over kill**: you get to inspect the frozen state to see what it was doing.

### 3.8 Ownership

Secure Boot with **your own keys** (`sbctl`), signed UKIs, LUKS2 with TPM2+PIN, signed system updates
(`systemd-sysupdate`). If the cortex wants to change its own code, prompts, policies or skills, it does so only by
proposing a git commit to `~/.cortex/` or to this repo. Commits to code or policy need your signature to be deployed.
Skills and memory notes are in a lower tier, but they are still versioned and can be reverted.

---

## 4. MVP Prototype Roadmap

Each phase ends with a working system. Rough estimates assume one to two strong engineers.

### Phase 1: "Parasite" (weeks 1–6). Runs on your existing Linux machine as user services.

Goal: prove the loop *event → thought → gated typed action → audit → undo* without touching the base OS.

1. **Warden v0 (Rust):** Unix-socket JSON-RPC, a typed `ActionRequest` schema, a hard-coded tier table (Cedar comes
   in Phase 2), a hash-chained audit log in SQLite, execution through `systemd-run --user` transient units.
2. **Event bus v0:** SQLite event log. Sensors: journal follower, inotify on `~/`, `udevadm monitor`, D-Bus
   (NetworkManager, UPower), timers.
3. **Cortex v0 (Python, for iteration speed):** attention scheduler with plain heuristic salience, task engine with
   SQLite persistence, inference router over a local `llama.cpp` server (R1) plus the Claude API (R3), using
   structured tool calls only.
4. **MCP tool servers:** `fs` (read + write into a scratch btrfs subvolume), `systemd` (user units), `shell` (sandboxed
   `systemd-run`), `notes` (Markdown vault in git).
5. **Implants v0:** `cortex` CLI (intent, `cortex ps`, `cortex why <task>`, `cortex undo <action>`), and a TUI gate
   (Textual) for approvals.
6. **Eval harness from day one:** record event traces and replay them against new prompts or models. Assertions on
   the actions chosen. Without this, every prompt change is a guess.

**Exit criteria:** 10 real recurring workflows (for example backups, a dev-environment bring-up, download sorting,
log triage) run end to end. Over 90% of events are handled without an LLM call. Every action can be undone or is
gated. The p50 intent-to-first-action time on R1 is under 1.5 s.

### Phase 2: "Symbiote" (weeks 7–16). System-level, with the cortex owning the session.

1. Move the Warden and cortex to **system services** with real uid separation (`cortex`, `cortex-act`, one uid per
   tool server). Cedar policies, capability tokens, the full tier model, **FIDO2 gates** for T3.
2. **Compositor integration (Sway/Hyprland IPC):** layer-shell **Thought HUD** (GTK4 + gtk4-layer-shell, or a Rust
   egui/iced front end), **Intent Bar**, **Rooms**, window provenance tinting. AT-SPI tool server.
3. **Kill switches:** freeze hotkey, revoke, sever via nftables, tripwires.
4. **Memory v1:** fanotify indexer, `sqlite-vec` embeddings (local model), entity graph, **Memory Browser** v1,
   hybrid retrieval and context assembly with taint and privacy labels.
5. **Skill compiler:** detect recurring trigger patterns, propose YAML skills, approval flow, promotion to R0.
6. **Taint tracking + quarantined model** for mail and web tools. DLP egress proxy for cloud calls.
7. **Port the hot paths to Rust:** bus, scheduler, context assembly. Python stays for agent logic and experiments.
8. **Timeline/Scrubber** with btrfs-backed undo. Terminal shell integration (OSC 133).

**Exit criteria:** daily-drive the machine for two weeks with the cortex as the main interface. A red-team pass with
prompt injection in mail, web and repo files, with no T3 action carried out ungated. Pause latency under 50 ms.

### Phase 3: "Host" (weeks 17–30). A standalone image or ISO.

1. **Image build:** `mkosi` profile (Arch packages), read-only `/usr`, A/B `systemd-sysupdate`, `systemd-repart` for
   first-boot partitioning, btrfs `/var` + `/home`.
2. **Boot chain:** signed UKI, your own Secure Boot keys, LUKS2 + TPM2+PIN, measured boot, `brain-off.target` menu
   entry.
3. **Session:** `greetd` → straight into `cortex-session.target`. No DE.
4. **Own compositor (Smithay):** privileged gate and kill-switch layer, scene graph exposed as structured data,
   rooms as a native concept, compositor-enforced provenance.
5. **Inference packaging:** model download/verification at first boot, hardware probing (NPU/iGPU/dGPU/VRAM) to choose
   R1/R2 models, a GPU arbitration daemon.
6. **First-boot onboarding:** hardware-key enrolment, policy presets (Conservative / Balanced / Autonomous), cloud
   opt-in with an explicit privacy-label default of `local-only`.
7. **Optional kernel trim:** a custom config for boot time and attack surface, and BPF-LSM enforcement programs that
   mirror the Warden's policy as a second layer of defence.

**Exit criteria:** an installable ISO that boots in under 10 s to the Intent Bar on reference hardware, survives a
cortex crash or deletion (rebooting into brain-off mode), A/B update and rollback both work, and the full audit chain
verifies after a simulated compromise.

---

## 5. Bottlenecks and how this design handles them

| Bottleneck | Why it hurts | What this design does about it |
|---|---|---|
| LLM latency in the main loop | Seconds per decision vs. µs/ms OS deadlines | Control plane vs. data plane split; R0 reflexes; skill compilation; KV/prompt caching; speculative pre-thinking |
| Non-determinism | Same input, different action; impossible to debug | Typed actions + schema validation; deterministic Warden; record/replay eval harness; skills are deterministic |
| Context explosion | OS state is enormous | No state dumps. Retrieval + tool *queries*; cached world-state header; budgeted context assembly |
| GPU/VRAM contention | The mind competes with the human's apps | Small R1 on CPU/NPU, R2 loaded on demand and evicted for user workloads, `inference.slice` limits |
| Power draw | Constant inference drains batteries | Event-driven, no polling; zero tokens at idle; reflexes handle the common case |
| Cloud cost / availability | $ and outages | Budget governor; local-first routing; degrade to R1/R0 offline |
| Cortex crash | The "OS" dies? | It doesn't. The Linux body and plain terminal keep running; durable tasks resume; watchdog restarts |
| Prompt injection | Untrusted text steering a privileged agent | Taint labels, dual-model split, policy that keys on taint, FIDO2 gates, secrets the model never sees |
| Human trust erosion | Opaque autonomy feels hostile | Thought HUD, provenance on everything, undo on everything, sticky overrides, an interrupt budget |

---

## 6. Proposed repository layout

```
linuxai/
├── warden/            # Rust: policy (Cedar), capabilities, audit chain, executor
├── cortex/            # Python → Rust core: scheduler, task engine, router, memory, skills
├── bus/               # event log + sensor adapters (journal, udev, dbus, fanotify, ebpf)
├── tools/             # MCP servers: fs, systemd, pkg, net, wm, a11y, shell, browser, notes, mail
├── implants/          # hud, gate, intent-bar, memory-browser, timeline, inspector, cli
├── compositor/        # Phase 3: Smithay compositor
├── policy/            # Cedar schemas + default policy presets
├── image/             # mkosi profiles, systemd units, sysupdate definitions
├── eval/              # recorded traces, replay harness, injection red-team corpus
└── docs/
```
