"""Paths and settings. Everything can be overridden with environment variables."""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from pathlib import Path


def _home() -> Path:
    if os.environ.get("LINUXAI_HOME"):
        return Path(os.environ["LINUXAI_HOME"]).expanduser()
    xdg = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(xdg) / "linuxai"


@dataclass
class Config:
    home: Path = field(default_factory=_home)
    model: str = field(default_factory=lambda: os.environ.get("LINUXAI_MODEL", "claude-opus-5-5"))
    # low | medium | high | xhigh | max. Opus 5.5 defaults to medium; running a machine is agentic work.
    effort: str = field(default_factory=lambda: os.environ.get("LINUXAI_EFFORT", "high"))
    # ask: confirm every change | trusted: auto-run read-only + learned commands | auto: run everything
    mode: str = field(default_factory=lambda: os.environ.get("LINUXAI_MODE", "trusted"))
    # Web search and fetch for research (server-side tools). LINUXAI_WEB=0 turns them off.
    web: bool = field(default_factory=lambda: os.environ.get("LINUXAI_WEB", "1") != "0")
    # Raw email is kept here for the human; the AI's tools refuse to read it (it only ever sees extracted facts).
    mail_dir: Path = field(default_factory=lambda: Path(os.environ["LINUXAI_MAIL_DIR"]).expanduser()
                           if os.environ.get("LINUXAI_MAIL_DIR") else _home() / "mail")
    command_timeout: int = field(default_factory=lambda: int(os.environ.get("LINUXAI_TIMEOUT", "600")))
    max_output_chars: int = 8000

    @property
    def db_path(self) -> Path:
        return self.home / "mind.db"

    @property
    def mind_dir(self) -> Path:
        """Human-readable Markdown view of what the AI has learned."""
        return self.home / "mind"

    def ensure(self) -> None:
        (self.mind_dir / "solutions").mkdir(parents=True, exist_ok=True)
