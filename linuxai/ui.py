"""Terminal implant: how the AI's activity is shown to the human, and how the human steers it."""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass

_COLOR = sys.stdout.isatty() and not os.environ.get("NO_COLOR")


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if _COLOR else text


def dim(t: str) -> str: return _c("2", t)
def bold(t: str) -> str: return _c("1", t)
def cyan(t: str) -> str: return _c("36", t)
def yellow(t: str) -> str: return _c("33", t)
def red(t: str) -> str: return _c("31", t)
def green(t: str) -> str: return _c("32", t)
def magenta(t: str) -> str: return _c("35", t)


def out(text: str = "", end: str = "\n") -> None:
    sys.stdout.write(text + end)
    sys.stdout.flush()


def ask(prompt: str) -> str:
    """Read one line from the human. EOF counts as an empty answer."""
    try:
        return input(prompt)
    except EOFError:
        return ""


@dataclass
class Decision:
    action: str          # run | skip | redirect
    text: str = ""       # edited command, or the human's message when redirecting
    always: str = ""     # prefix the human chose to always allow


HELP = (
    "  Enter/y  run it            n  skip\n"
    "  e        edit, then run    a  always allow commands like this\n"
    "  or type anything else to tell the AI what to do instead"
)


def confirm(kind: str, summary: str, editable: str | None = None, suggest_prefix: str = "") -> Decision:
    """The human-as-guardrail gate. Returns what the human decided."""
    while True:
        ans = ask(yellow(f"  run {kind}? [Y/n/e/a/?] ")).strip()
        low = ans.lower()
        if low in ("", "y", "yes"):
            return Decision("run")
        if low in ("n", "no"):
            why = ask(dim("  why (optional, sent to the AI): ")).strip()
            return Decision("skip", why)
        if low == "?":
            out(HELP)
            continue
        if low == "e" and editable is not None:
            new = edit_line(editable)
            return Decision("run", new) if new.strip() else Decision("skip", "human cleared the command")
        if low == "a" and suggest_prefix:
            pre = ask(f"  always allow commands starting with [{suggest_prefix}]: ").strip() or suggest_prefix
            return Decision("run", always=pre)
        if low in ("e", "a"):
            out(dim("  (not available for this action)"))
            continue
        return Decision("redirect", ans)


def edit_line(initial: str) -> str:
    """Let the human edit a command in place (readline pre-fill when available)."""
    try:
        import readline

        readline.set_startup_hook(lambda: readline.insert_text(initial))
        try:
            return ask("  edit> ")
        finally:
            readline.set_startup_hook()
    except ImportError:
        out(dim(f"  current: {initial}"))
        return ask("  new command> ") or initial
