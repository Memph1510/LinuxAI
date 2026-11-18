# LinuxAI

An operating system where the AI is the system. You turn the computer on, log in, and you're inside the AI. You
talk to it in plain English, it runs the machine, and you're the intent and the guardrail. Linux is used only as
the hardware-driver layer; everything above it is LinuxAI.

- **[docs/SCOPE.md](docs/SCOPE.md)**: what we're building and in what order
- **[docs/CHECKLIST.md](docs/CHECKLIST.md)**: what's done and what's next (core first, then install, the Workspace screen, the Android app)
- **[docs/SYSTEM_INTERFACE.md](docs/SYSTEM_INTERFACE.md)**: objects, events, actions and views (the foundation)
- **[docs/TASKS.md](docs/TASKS.md)**: tasks of any size, the human steps (action, evaluation, critique, decision), playbooks and cases
- **[docs/MAIL.md](docs/MAIL.md)**: email as information, never instructions (the first outside-world organ)
- **[docs/BROWSER.md](docs/BROWSER.md)**: the browser you and the AI use at the same time
- **[docs/IMAGES.md](docs/IMAGES.md)**: making and editing pictures on any machine (local tools, local models, paid services)
- **[docs/RESEARCH_SELF_LEARNING.md](docs/RESEARCH_SELF_LEARNING.md)**: what self-learning harnesses (DeepSeek Harness, ACE, Memento-Skills, Recuris, Hermes…) do, and what LinuxAI takes from them
- **[docs/ACTION_LANGUAGE.md](docs/ACTION_LANGUAGE.md)**: the language the AI uses to run the machine
- **[docs/VIEWS.md](docs/VIEWS.md)**: the Workspace screen (tasks, decisions, reasoning, comments that change things); prototype in `prototypes/`
- **[image/README.md](image/README.md)**: the bootable ISO, and how to try it in a VM on Windows
- [docs/BLUEPRINT.md](docs/BLUEPRINT.md): the original technical blueprint

## v0.1: it works as an OS

| Part | Where | What it does |
|---|---|---|
| **LinuxAI core** | `core/` (Rust) | The system service that runs the machine. It senses power, battery, CPU/memory/I/O pressure, temperature, failed services and the scheduler, and turns changes into events. It controls the machine through the kernel's steering surfaces: power profiles, the sched_ext scheduler policy, cgroup resource shares via systemd, services and sysctl. It runs plans in the **LinuxAI action language**: it checks them first, runs them at machine speed, records every step, and lets plans pause for days waiting for your answer and resume after a reboot. Built-in **rules** handle routine situations with no AI model (unplugged → save power). |
| **The AI** | `linuxai/` (Python) | Only the parts that talk to Claude: the mind, your session, and filling in the claims form for email (`cortex extract`). It plans whole jobs at once in the action language, asks when unsure, and learns. Everything else is the Rust core: the **email organ** (`linuxai-core mail`), the **shared browser** (`linuxai-core browser`), **pictures** (`linuxai-core image`) and **reach** (`linuxai-core reach`) included. |
| **The session** | `cortex session` | Your screen when you log in: talk to it, see what the system is doing as it happens, answer its questions. |
| **The mind** | `cortex mind` | The AI working behind the scenes on what the rules hand it ("this service failed, find out why"). |
| **The image** | `image/` | An Arch-based live ISO that boots into the LinuxAI session. Built by GitHub Actions on every push. |

## Running it

**As an OS:** download the ISO from the latest GitHub Actions run (or `./image/build.sh`) and boot it in a VM.
See [image/README.md](image/README.md).

**On an existing Linux machine (development):**

```bash
cd core && cargo build --release && ./target/release/linuxai-core daemon &   # the core
pip install -e .                                                             # the AI layer
export ANTHROPIC_API_KEY=...
cortex session                                                                # step inside
```

Without the core running, `cortex` falls back to the earlier prototype: a terminal agent that runs commands with
your approval.

## Tests

```bash
cd core && cargo test          # engine, trust, durable plans, rules, senses
pytest -q                      # AI layer, plus integration with a live core
python3 image/test-boot.py out/linuxai-*.iso   # boots the ISO in QEMU and checks it works as an OS
```
