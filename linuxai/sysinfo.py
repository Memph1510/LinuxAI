"""Cheap snapshot of the machine's state, fed to the AI as context. No LLM involved."""

from __future__ import annotations

import os
import platform
import shutil
import socket
import subprocess


def _run(cmd: list[str], timeout: int = 5) -> str:
    if not shutil.which(cmd[0]):
        return ""
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout).stdout.strip()
    except (subprocess.SubprocessError, OSError):
        return ""


def os_release() -> str:
    try:
        with open("/etc/os-release") as f:
            info = dict(line.rstrip().split("=", 1) for line in f if "=" in line)
        return info.get("PRETTY_NAME", "").strip('"')
    except OSError:
        return platform.platform()


def failed_units() -> list[str]:
    out = _run(["systemctl", "--failed", "--no-legend", "--plain"])
    units = [line.split()[0] for line in out.splitlines() if line.strip()]
    out = _run(["systemctl", "--user", "--failed", "--no-legend", "--plain"])
    units += [f"(user) {line.split()[0]}" for line in out.splitlines() if line.strip()]
    return units


def recent_errors(n: int = 15, since: str | None = None) -> list[str]:
    cmd = ["journalctl", "-p", "err", "--no-pager", "-o", "short-iso", "-n", str(n)]
    cmd += ["--since", since] if since else ["-b"]
    return [line for line in _run(cmd).splitlines() if line and not line.startswith("--")]


def disk_usage() -> list[tuple[str, int]]:
    rows = []
    for line in _run(["df", "-P", "-x", "tmpfs", "-x", "devtmpfs", "-x", "overlay"]).splitlines()[1:]:
        parts = line.split()
        if len(parts) >= 6 and parts[4].endswith("%"):
            rows.append((parts[5], int(parts[4].rstrip("%"))))
    return rows


def snapshot() -> str:
    lines = [
        f"host: {socket.gethostname()}  user: {os.environ.get('USER', '?')}  os: {os_release()}",
        f"kernel: {platform.release()}  arch: {platform.machine()}",
    ]
    if up := _run(["uptime", "-p"]):
        lines.append(f"uptime: {up}")
    if mem := _run(["free", "-h"]):
        lines.append("memory:\n" + mem)
    if disks := disk_usage():
        lines.append("disk: " + ", ".join(f"{m} {p}%" for m, p in disks))
    pkg = [m for m in ("pacman", "apt", "dnf", "zypper", "nix", "flatpak") if shutil.which(m)]
    if pkg:
        lines.append("package managers: " + ", ".join(pkg))
    if os.environ.get("WAYLAND_DISPLAY") or os.environ.get("DISPLAY"):
        lines.append(f"session: {os.environ.get('XDG_CURRENT_DESKTOP') or os.environ.get('XDG_SESSION_TYPE') or 'graphical'}")
    failed = failed_units()
    lines.append("failed units: " + (", ".join(failed) if failed else "none"))
    errs = recent_errors(8)
    if errs:
        lines.append("recent journal errors (this boot):\n" + "\n".join(errs))
    return "\n".join(lines)
