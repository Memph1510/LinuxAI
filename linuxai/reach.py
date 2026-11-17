"""Reach: when the AI is undecided it doesn't sit and wait; it reaches the human.

Watches the LinuxAI core and sends a message whenever a plan needs the human (a question, an approval) or a
follow-up is due, and passes on what the AI wants the human to know. No AI model is involved, so it works
without an API key.

Channels:
  - the LinuxAI session (always: it shows the same events live)
  - LINUXAI_NOTIFY_CMD: any shell command that reads the message on stdin. This plugs in an SMS gateway, a
    push service or a chat bot, e.g.  LINUXAI_NOTIFY_CMD='curl -s -d @- https://ntfy.sh/<your-topic>'
    (Later: the LinuxAI Android app, which can also carry your reply back.)
"""

from __future__ import annotations

import os
import subprocess
import time
from datetime import datetime


def _when(ts: float | None) -> str:
    if not ts:
        return ""
    d = datetime.fromtimestamp(ts)
    today = datetime.now().date()
    day = "" if d.date() == today else ("tomorrow " if (d.date() - today).days == 1 else d.strftime("%a %b %-d "))
    return day + d.strftime("%-I:%M%p").lower()


def message_for(event: dict) -> str | None:
    """The text sent to the human for a core event, or None if it isn't worth a message."""
    kind, data = event.get("kind", ""), event.get("data") or {}
    wait = data.get("wait") or {}
    if kind in ("plan.waiting", "plan.reminder") and wait.get("question"):
        lines = [("Reminder: " if kind == "plan.reminder" else "") + f"LinuxAI needs you: {wait['question']}"]
        if wait.get("why"):
            lines.append(f"Why I'm unsure: {wait['why']}")
        if wait.get("lean"):
            lines.append(f"I'd lean: {wait['lean']}")
        if wait.get("opts"):
            lines.append("Reply: " + " / ".join(map(str, wait["opts"])))
        if wait.get("fallback") and wait.get("until"):
            lines.append(f"If I don't hear back by {_when(wait['until'])}, I'll go with: {wait['fallback']}")
        elif wait.get("fallback"):
            lines.append(f"If I don't hear back, I'll go with: {wait['fallback']}")
        lines.append(f"[{data.get('plan_id', '')}]")
        return "\n".join(lines)
    if kind == "ai.tell" and data.get("text"):
        return f"LinuxAI: {data['text']}"
    if kind == "plan.failed":
        return f"LinuxAI: '{data.get('plan')}' failed: {data.get('error')}. I'll look into it."
    return None


def send(text: str, cmd: str | None = None) -> bool:
    cmd = cmd if cmd is not None else os.environ.get("LINUXAI_NOTIFY_CMD", "")
    if not cmd:
        return False
    try:
        subprocess.run(["sh", "-c", cmd], input=text, text=True, timeout=30, capture_output=True, check=True)
        return True
    except (subprocess.SubprocessError, OSError):
        return False


def run() -> None:
    from .core_client import Core

    core = Core()
    while not core.available():
        time.sleep(3)
    cmd = os.environ.get("LINUXAI_NOTIFY_CMD", "")
    print(f"linuxai reach: connected; channel: {'command' if cmd else 'session only (set LINUXAI_NOTIFY_CMD)'}",
          flush=True)
    # Anything already waiting when we start gets one message, so nothing is left sitting unseen.
    for w in core.request("waiting").get("plans", []):
        text = message_for({"kind": "plan.waiting", "data": w})
        if text:
            send(text, cmd)
    for ev in core.follow():
        text = message_for(ev)
        if text and not send(text, cmd) and cmd:
            print("linuxai reach: could not send a message through LINUXAI_NOTIFY_CMD", flush=True)
