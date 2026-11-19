"""Background senses. Watches the machine without spending any model tokens and leaves notes in the
inbox; the mind picks them up the next time you talk to it (or when you type /inbox)."""

from __future__ import annotations

import hashlib
import time
from datetime import datetime

from . import sysinfo
from .config import Config
from .memory import Memory


def check_once(memory: Memory, since: str | None) -> int:
    posted = 0
    for unit in sysinfo.failed_units():
        posted += memory.post_inbox("systemd", f"failed:{unit}:{datetime.now():%Y-%m-%d}",
                                    f"unit {unit} is in a failed state")
    for mount, pct in sysinfo.disk_usage():
        if pct >= 90:
            posted += memory.post_inbox("disk", f"disk:{mount}:{pct // 5}:{datetime.now():%Y-%m-%d}",
                                        f"{mount} is {pct}% full")
    for line in sysinfo.recent_errors(30, since=since):
        msg = line.split(" ", 2)[-1] if " " in line else line
        key = "journal:" + hashlib.sha1(msg.encode()).hexdigest()[:16]
        posted += memory.post_inbox("journal", key, line[:300])
    return posted


def run(interval: int = 120) -> None:
    config = Config()
    memory = Memory(config)
    since = "-10min"
    print(f"cortex observer: checking every {interval}s, writing to {config.db_path}", flush=True)
    while True:
        stamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        n = check_once(memory, since)
        if n:
            print(f"{stamp} posted {n} new observation(s)", flush=True)
        since = stamp
        time.sleep(interval)
