# LinuxAI: Scope

This is what came out of the scoping Q&A. The [BLUEPRINT](BLUEPRINT.md) has the technical depth. This
document says what we are building and in what order.

## The vision

LinuxAI is an operating system where **the AI is the system.** When you turn the computer on and log in, you are
logging into the AI.

- **No separate apps.** Email, messages, calendar, pictures and files are not programs you open. They are parts
  of the AI, different ways of looking at what it knows and different ways you communicate with it.
- **It knows your life.** Everything on the machine (mail, messages, projects, bills, plans) is one continuous
  picture, not a pile of separate, static files. So you give it *intent* and it works out the instructions.
- **An assistant, not a task runner.** "Book a flight" is enough. It knows where you usually fly, your budget, your
  airlines, the dates, and *why* you're going, because it has learned that from living alongside you.
- **The human is the intent and the guardrail.** You see what it's doing by opening the computer (its views)
  or by talking to it. When it isn't confident, it asks. When you've set a direction, it follows it.
- **It learns by conversation.** At first it knows nothing about you, so you talk a lot. Every exchange and
  every solved problem makes the next one need less talking.

### What it is not

- Not a Linux desktop with an assistant bolted on.
- Not a chatbot that waits for step-by-step instructions.
- Not a pile of separate apps that happen to share an AI.
- Not boxed in with guardrails before the capability exists. The human is the guardrail; hardening comes later.

## Decisions so far

| Topic | Decision |
|---|---|
| Name | **LinuxAI** (the OS and the AI) |
| What "OS" means | A real bootable distro. Login = logging into the AI. Built from the base up with memory, tasks and budgets in mind. |
| Foundation | **Our own system, with Linux only as the hardware-driver layer.** Everything above the kernel (no apps, memory instead of files, intent instead of commands, the views) is ours. Our system interface is designed to be kernel-independent, so Linux can be swapped out later without a redesign. |
| Base packages | Arch (in the spirit of Omarchy), used only for the kernel, drivers and low-level plumbing |
| Language | **LinuxAI gets its own action language**: what the AI writes to act, which the OS carries out exactly, and which the human can read step by step. It replaces bash and Python as the AI's way of doing things. |
| Install | Dual boot next to Windows on your laptop. **Developed in a VM first**, then dual boot once it's stable. |
| Models | Cloud models for now (Claude for the hard thinking; cheap models such as DeepSeek for bulk work). Local models later if useful. The laptop has no strong GPU. |
| Always-on listener | Either the Android tablet server you already have, or a small cloud server. Decide when we get to it. |
| Talking to it | Text-based. Our own Android messaging app, which can become your SMS app (Android allows that). |
| Email | New mailbox with your existing mail imported. Your own domain later. |
| Money | Budgets per month, week, task or category (flights, hotels). It learns normal patterns (for example the usual Verizon bill) and flags anomalies. Few purchases for now; mainly bills. |
| Visibility | You open the computer and see what it's doing through its views, or you ask it. Exactly what the views look like is to be designed together. |
| First milestone | **A functioning OS first, features later.** The first bootable version has to work as an operating system. How each feature works will take a lot of discussion. |

## Not limited to Linux

Linux is a tool here, not a constraint. What it gives us is **drivers**: decades of code for real hardware (wifi,
graphics, battery, sleep), which the AI needs, above all to stay online. It does not decide how the system works.
So the split is:

```
 ┌───────────────────────────────────────────────────────────┐
 │  LinuxAI: ours, kernel-independent                        │
 │  the AI core · memory (instead of files) · action language│
 │  events · views (instead of apps) · budgets · tasks       │
 ├───────────────────────────────────────────────────────────┤
 │  LinuxAI system interface (the contract we define)        │
 ├───────────────────────────────────────────────────────────┤
 │  Kernel + drivers: Linux today, replaceable later         │
 └───────────────────────────────────────────────────────────┘
```

A LinuxAI-native kernel stays possible as a later research track. It would only have to implement the system
interface.

## The kernel question

You said the most important thing right now is that the AI **manages everything the Linux kernel would manage.**
How that works shapes the whole build, so here it is in plain terms.

**What the kernel does.** It switches the CPU between programs thousands of times a second, hands out memory, and
talks to the hardware (wifi, disk, screen, battery) through drivers. Those decisions happen in millionths of a
second, millions of times a minute.

**Why the AI can't make each of those decisions itself.** An AI model takes somewhere between a tenth of a second
and several seconds to make one decision. If the AI made every scheduling decision, the computer would freeze. If
the model were unreachable (wifi down, API down), the computer would stop completely. Rewriting the drivers would
take years and give the AI nothing new to work with.

**What we do instead, and why it's real control rather than a workaround.** The Linux kernel is built to be
*steered*. It exposes control surfaces for almost everything it manages, and the AI becomes the thing that
sets them:

| The kernel manages | How LinuxAI controls it |
|---|---|
| **CPU scheduling** | **sched_ext** (in Linux since 6.12): the kernel's scheduling policy itself can be swapped at runtime for a small program. The AI chooses and tunes the scheduling policy based on what you're doing, and that policy runs at kernel speed. Per-workload CPU shares come from cgroups. |
| **Memory** | cgroup memory limits and priorities per workload, swap and pressure tuning. Kernel "pressure stall" signals tell the AI when the machine is struggling. |
| **Disk I/O** | I/O priorities and weights per workload. |
| **Power and heat** | CPU frequency governor, power profiles, sleep behaviour, based on battery and what you're doing. |
| **Processes and services** | The AI starts, stops and supervises every service and program through systemd, which acts as its hands. |
| **Devices** | Every plug-in, unplug, network change and error arrives as an event the AI reacts to. |
| **Network** | Connections, priorities, firewall. |

So: **the kernel executes, the AI decides.** The AI sets the policy (what matters right now, who gets
resources, what to do about this device or that failure), and the kernel carries it out at hardware speed. If the
AI is offline, the machine keeps running on the last policy it set. Nothing freezes.

This is how LinuxAI "manages the kernel" from day one, and it's the foundation the memory, task engine and budgets
get built on.

## First bootable version (v0.1): "it works as an OS"

Built and tested in a VM on Windows.

0. **The system interface and action language (the foundation).** Before code, write down the contract:
   - what a *memory object* is (replacing files);
   - what an *action* is, and the first draft of the action language;
   - what an *event* is;
   - what a *view* is (replacing apps).

   Everything else is built on it. Drafts: [SYSTEM_INTERFACE.md](SYSTEM_INTERFACE.md), [ACTION_LANGUAGE.md](ACTION_LANGUAGE.md).
1. **The image.** A LinuxAI ISO, built reproducibly from this repo (Arch packages via `archiso` or `mkosi`).
   It installs and boots in a VM.
2. **Boot into the AI.** Boot → login → the LinuxAI session. There is no Windows-style desktop. The first
   screen is the AI: a full-screen conversational view showing what it's doing.
3. **The LinuxAI core.** A system service started at boot with authority over the machine. It:
   - *senses*: processes, CPU/memory/I/O pressure, battery, temperatures, devices, network, logs, failures;
   - *decides*: what matters right now and what to do about it;
   - *acts* through the control surfaces in the table above: sched_ext policy, cgroups, power, systemd, devices,
     network.
4. **Machine management, done by the AI.** Examples of what v0.1 handles on its own:
   - gives what you're actively doing priority and holds back background work;
   - changes power and CPU policy when you unplug, and when the battery or temperature changes;
   - notices a failed service or full disk and fixes it, or tells you;
   - handles new devices and networks;
   - installs updates and applies config changes.
5. **Memory and conversation from the start.** It carries over the learning loop from the current `cortex`
   prototype: it asks when unsure, records how problems were solved, and keeps notes about the machine and you.
6. **Survives without the cloud.** If the model is unreachable, the machine still boots and runs on the last
   policies, with a plain terminal available.

**v0.1 is done when** it boots in a VM to the LinuxAI session, the core is visibly making scheduling, power and
service decisions that you can watch and question, and it keeps working (degraded) when offline.

## After v0.1 (each needs its own design discussion)

- **The parts**: email (imported mailbox), the Android messaging app, files and projects as memory,
  calendar, bills and budgets with anomaly flags.
- **The views**: what "opening the computer and seeing what it's doing" looks like (live activity, the email
  view, the calendar view, the files view), all as views into one AI rather than separate apps.
- **The always-on listener** on the Android tablet or a small server, so mail and texts are handled while the
  laptop is closed.
- **Dual boot install** on the real laptop.
- **Your own email domain**, local models, hardening.

## Open questions for later rounds

- What the main screen looks like when you log in.
- How it should introduce itself and learn about you in the first week.
- Which existing accounts it takes over first (mail provider, bank, Verizon, airlines).
- How much the tablet server does compared with the laptop.
