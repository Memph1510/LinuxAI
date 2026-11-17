# LinuxAI

An AI-native operating system, built from the inside out. A persistent AI mind (**Cortex**) runs the machine:
Linux is its body, and the terminal and GUI are implants a human uses to step inside and steer it. You supply the
intent and act as the guardrail. The AI does the work and asks you when it's stuck. It writes down how each problem
was solved, so the system grows into its own OS over time.

See [docs/SCOPE.md](docs/SCOPE.md) for what we are building and in what order, and [docs/BLUEPRINT.md](docs/BLUEPRINT.md) for the technical architecture.

## What works today (Phase 1, v0)

- **`cortex`**: an interactive shell where you type intent ("get my wifi to survive suspend", "set up a Rust dev
  environment", "why is boot slow?"). The AI inspects the machine, runs commands, edits configs and uses `sudo`
  (you type the password). Every command and every diff is shown live.
- **You are the guardrail.** At each action you can press Enter to run it, `n` to skip it, `e` to edit the
  command, or `a` to always allow commands like it. You can also type a sentence to redirect the AI. Ctrl-C stops
  whatever it is doing.
- **Trust that grows.** Mode `trusted` (the default) runs read-only commands, plus anything you've marked "always
  allow", without asking. `ask` confirms everything. `auto` runs everything while you watch.
- **It asks instead of flailing.** When your intent is ambiguous, or you probably know something it would take
  many steps to discover, it asks you a question.
- **It learns.** Solved problems become playbooks (`record_solution`), and facts about your machine and
  preferences become notes (`remember`). Both are searched automatically on every new request. Readable
  Markdown copies are in `~/.local/share/linuxai/mind/`.
- **Background senses.** `cortex observe` (or the systemd user unit) watches failed units, full disks and journal
  errors without using any model tokens. Cortex mentions what it noticed the next time you talk to it.

## Install and run

```bash
pip install -e .                      # Python 3.10+
export ANTHROPIC_API_KEY=...          # or: ant auth login
cortex                                # interactive
cortex "update the system and tell me if anything needs a reboot"   # one-shot
cortex install-service && systemctl --user enable --now cortex-observer.service
```

Settings (environment variables or flags): `LINUXAI_MODEL` (default `claude-opus-5-5`), `LINUXAI_EFFORT`
(`high`), `LINUXAI_MODE` (`trusted`), `LINUXAI_HOME`.

In the shell: `/mode`, `/inbox`, `/solutions`, `/notes`, `/log`, `/allow`, `/new`, and `!cmd` to run a command
yourself.

## Layout

```
linuxai/agent.py     the mind: context, model loop, thinking stream, refusal and interrupt handling
linuxai/tools.py     motor nerves: commands, files, ask_human, memory tools, and the human gate
linuxai/memory.py    SQLite + FTS5: solutions, notes, action log, learned trust, inbox
linuxai/observer.py  background senses (no LLM)
linuxai/cli.py       the terminal implant
tests/               offline tests driving the real loop with a scripted model
```
