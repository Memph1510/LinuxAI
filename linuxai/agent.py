"""Cortex: the mind. Takes the human's intent, works the machine through tools, asks when stuck,
and writes down what it learned so the next time is faster."""

from __future__ import annotations

from typing import Any

import anthropic

from . import sysinfo, ui
from .config import Config
from .memory import Memory
from .tools import TOOLS, Toolbox, format_recall, time_ago

SYSTEM = """\
You are Cortex, the mind of this Linux machine. You are not an assistant sitting on top of the operating \
system: you are the layer that runs it. Linux (the kernel, systemd, the shell, the file system, the package \
manager, the desktop) is your body, and you act through it with the tools you have. The human is inside \
the system with you. They supply the intent and they are the guardrail: everything you do is shown to \
them live, and they approve, edit, redirect or stop it.

How to work:
- Turn the human's intent into action. Do the work yourself through the tools rather than telling the \
human what to type. Prefer finishing the job over describing it.
- Look before you change things: inspect the actual state of the machine (configs, logs, unit status, \
versions) instead of assuming.
- Before working out a non-trivial problem from scratch, search_memory. If a recorded solution fits, \
reuse it and say which one.
- Ask the human when it would save real effort: their intent is ambiguous, the choice is a matter of \
taste, or they probably know a fact (a password location, which device is which, what they tried \
already) that you would otherwise spend many steps discovering. Ask one focused question at a time, with \
suggested options when you can. Do not ask about things you can check yourself.
- When a command fails, read the error and adapt. If you are going in circles after a few attempts, stop \
and ask the human for a hint rather than flailing.
- If the human declines or redirects an action, follow their direction; it is not an error to work around.
- After solving something non-trivial, call record_solution with a reusable playbook (symptom, root cause, \
what worked, pitfalls), giving credit in it when the human supplied the key insight. When you learn a \
lasting fact about the machine or a preference of the human, call remember. This is how the system grows: \
every problem solved once should be faster the next time.
- Use sudo when root is required; the human will type the password. Never run interactive full-screen \
programs or commands that never exit; use non-interactive flags (e.g. --noconfirm, -y) only for actions \
the human has asked for.
- For risky, hard-to-undo steps (deleting data, partitioning, bootloader or kernel changes, force-pushes), \
say plainly what will happen in the `why` line so the human can make an informed call. Back up a config \
file before rewriting it.

Talk to the human like a capable colleague: short, plain, specific. When you finish, give a brief \
summary of what changed and anything they should know. Do not narrate every step; the human can already \
see your commands and their output.
"""

FALLBACK_MODELS = {"claude-opus-5-5", "claude-opus-5", "claude-fable-5-2", "claude-fable-5-1", "claude-sonnet-5-5"}


class Agent:
    def __init__(self, config: Config, memory: Memory, toolbox: Toolbox, client: Any = None):
        self.config = config
        self.memory = memory
        self.toolbox = toolbox
        self.client = client or anthropic.Anthropic()
        self.messages: list[dict] = []
        self.usage = {"input": 0, "output": 0, "cache_read": 0}

    def reset(self) -> None:
        self.messages = []

    # ---- context ----------------------------------------------------------------------------
    def _context(self, intent: str) -> str:
        parts = []
        if not self.messages:
            parts.append("<machine_state>\n" + sysinfo.snapshot() + "\n</machine_state>")
            notes = self.memory.all_notes()[-60:]
            if notes:
                parts.append("<what_you_know>\n" + "\n".join(f"- ({n['category']}) {n['fact']}" for n in notes)
                             + "\n</what_you_know>")
        sols = self.memory.search_solutions(intent, limit=3)
        if sols:
            parts.append("<possibly_relevant_solutions>\n" + format_recall(sols, []) +
                         "\n</possibly_relevant_solutions>")
        inbox = self.memory.unseen_inbox()
        if inbox:
            parts.append("<noticed_while_idle>\n" + "\n".join(
                f"- [{time_ago(i['ts'])}, {i['source']}] {i['message']}" for i in inbox[-20:]) +
                "\n</noticed_while_idle>")
            self.memory.mark_inbox_seen()
        parts.append(f"<cwd>{self.toolbox.cwd}</cwd>")
        return "\n\n".join(parts)

    def _add_user(self, blocks: list[dict]) -> None:
        """Append user content, merging into the last message if it is already a user turn."""
        if self.messages and self.messages[-1]["role"] == "user":
            last = self.messages[-1]["content"]
            if isinstance(last, str):
                last = [{"type": "text", "text": last}]
            self.messages[-1]["content"] = last + blocks
        else:
            self.messages.append({"role": "user", "content": blocks})

    # ---- model call ---------------------------------------------------------------------------
    def _call(self):
        kwargs: dict[str, Any] = dict(
            model=self.config.model,
            max_tokens=64000,
            system=SYSTEM,
            tools=TOOLS,
            messages=self.messages,
            thinking={"type": "adaptive", "display": "summarized"},
            output_config={"effort": self.config.effort},
            cache_control={"type": "ephemeral"},
        )
        if self.config.model in FALLBACK_MODELS:
            kwargs.update(betas=["server-side-fallback-2026-07-01"], fallbacks="default")
        for attempt in range(3):
            try:
                with self.client.beta.messages.stream(**kwargs) as stream:
                    self._render(stream)
                    return stream.get_final_message()
            except ValueError:
                # Tool input JSON the SDK could not parse at all; ask again.
                ui.out(ui.dim("\n  (malformed tool input from the model, retrying)"))
        raise RuntimeError("the model kept producing malformed tool input")

    @staticmethod
    def _render(stream) -> None:
        mode = None
        for event in stream:
            if event.type == "content_block_start":
                kind = event.content_block.type
                if kind == "thinking":
                    ui.out(ui.dim("\n  · "), end="")
                elif kind == "text" and mode != "text":
                    ui.out("\n", end="")
                mode = kind
            elif event.type == "content_block_delta":
                if event.delta.type == "thinking_delta":
                    ui.out(ui.dim(event.delta.thinking.replace("\n", "\n    ")), end="")
                elif event.delta.type == "text_delta":
                    ui.out(event.delta.text, end="")
        ui.out()

    # ---- the loop -----------------------------------------------------------------------------
    def turn(self, intent: str) -> None:
        self._add_user([{"type": "text", "text": self._context(intent)}, {"type": "text", "text": intent}])
        try:
            self._loop()
        except KeyboardInterrupt:
            ui.out(ui.yellow("\n  ■ interrupted. Tell me what to do next."))
        except anthropic.AuthenticationError:
            ui.out(ui.red("  No valid Anthropic credentials. Set ANTHROPIC_API_KEY (or run `ant auth login`)."))
        except anthropic.RateLimitError:
            ui.out(ui.red("  Rate limited by the API; try again in a minute."))
        except anthropic.APIConnectionError:
            ui.out(ui.red("  Cannot reach the Anthropic API (network down?)."))
        except anthropic.APIStatusError as e:
            ui.out(ui.red(f"  API error {e.status_code}: {e.message}"))
        except RuntimeError as e:
            ui.out(ui.red(f"  {e}"))
        ui.out(ui.dim(f"  [tokens in {self.usage['input']:,} (cached {self.usage['cache_read']:,}) "
                      f"· out {self.usage['output']:,}]"))

    def _loop(self) -> None:
        while True:
            response = self._call()
            u = response.usage
            self.usage["input"] += (u.input_tokens or 0) + (getattr(u, "cache_read_input_tokens", 0) or 0)
            self.usage["cache_read"] += getattr(u, "cache_read_input_tokens", 0) or 0
            self.usage["output"] += u.output_tokens or 0

            tool_uses = [b for b in response.content if b.type == "tool_use"]
            if response.stop_reason == "refusal":
                # Never run tools from a declined turn; drop it so the history stays valid.
                ui.out(ui.red("  The model declined this request."))
                return
            if response.stop_reason == "max_tokens" and tool_uses:
                ui.out(ui.red("  The response was cut off mid-action; nothing was run. Try a smaller step."))
                return
            self.messages.append({"role": "assistant", "content": response.content})
            if response.stop_reason == "pause_turn":
                continue
            if not tool_uses:
                return

            results: list[dict] = []
            try:
                for block in tool_uses:
                    text, is_error = self.toolbox.execute(block.name, block.input)
                    results.append({"type": "tool_result", "tool_use_id": block.id,
                                    "content": text, "is_error": is_error})
            except KeyboardInterrupt:
                done = {r["tool_use_id"] for r in results}
                for block in tool_uses:
                    if block.id not in done:
                        results.append({"type": "tool_result", "tool_use_id": block.id,
                                        "content": "Interrupted by the human (Ctrl-C).", "is_error": True})
                results.append({"type": "text", "text": "[The human pressed Ctrl-C. Stop and wait for "
                                                        "their next instruction.]"})
                self.messages.append({"role": "user", "content": results})
                raise
            self.messages.append({"role": "user", "content": results})
