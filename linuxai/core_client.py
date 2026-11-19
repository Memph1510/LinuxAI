"""Client for the LinuxAI core (linuxai-core) over its local socket: JSON, one object per line."""

from __future__ import annotations

import json
import os
import socket
from pathlib import Path
from typing import Any, Iterator


def default_socket() -> str:
    if os.environ.get("LINUXAI_CORE_SOCKET"):
        return os.environ["LINUXAI_CORE_SOCKET"]
    if Path("/run/linuxai/core.sock").exists():
        return "/run/linuxai/core.sock"
    return str(Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "linuxai-core.sock")


def browser_socket() -> str:
    """Where the shared browser organ (linuxai-core browser) listens."""
    if os.environ.get("LINUXAI_BROWSER_SOCKET"):
        return os.environ["LINUXAI_BROWSER_SOCKET"]
    return str(Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "linuxai-browser.sock")


def compositor_socket() -> str:
    """Where the screen organ (linuxai-core compositor) listens, inside the Hyprland session."""
    if os.environ.get("LINUXAI_COMPOSITOR_SOCKET"):
        return os.environ["LINUXAI_COMPOSITOR_SOCKET"]
    return str(Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "linuxai-compositor.sock")


class CoreError(Exception):
    pass


class Core:
    def __init__(self, path: str | None = None, timeout: float = 600):
        self.path = path or default_socket()
        self.timeout = timeout

    def available(self) -> bool:
        try:
            return bool(self.request("ping").get("ok"))
        except (OSError, CoreError):
            return False

    def request(self, op: str, **fields: Any) -> dict:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(self.timeout)
            s.connect(self.path)
            s.sendall((json.dumps({"op": op, **fields}) + "\n").encode())
            buf = b""
            while not buf.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf += chunk
        if not buf:
            raise CoreError("no response from linuxai-core")
        return json.loads(buf)

    def follow(self) -> Iterator[dict]:
        """Yield every event from now on (blocks)."""
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.connect(self.path)
        s.sendall(b'{"op":"follow"}\n')
        f = s.makefile("r", encoding="utf-8")
        f.readline()  # {"ok": true, "following": true}
        for line in f:
            if line.strip():
                yield json.loads(line)
