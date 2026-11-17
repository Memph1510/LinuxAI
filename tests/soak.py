#!/usr/bin/env python3
"""Endurance (soak) run for the LinuxAI core: days of activity compressed into minutes.

  python3 tests/soak.py --minutes 30 [--core core/target/release/linuxai-core] [--out DIR]

Runs the real daemon with a heavy mixed workload and crashes it on purpose:
  - a storm of events (power changes, service failures, organ events, the human talking)
  - macro tasks with human steps, answered by a simulated human after random delays (yes / no / always / an option)
  - AI plans that must pass the second-look gate (assessments, unknowns, first-time situations)
  - long-running actions alongside everything else
  - kill -9 and restart every few minutes
  - pruning on a compressed schedule (minutes instead of days)
Samples memory, threads, open files, database size and response times; checks invariants at the end.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from linuxai.core_client import Core, CoreError  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
ASSESS = {"confidence": 0.9, "consequences": ["runs a harmless command"], "outcomes": [{"if": "it fails", "then": "try again later"}]}


class Daemon:
    def __init__(self, binary: str, work: Path, prune_every: float):
        self.binary, self.work, self.prune_every = binary, work, prune_every
        self.sock = str(work / "core.sock")
        self.proc: subprocess.Popen | None = None
        self.restarts = 0

    def start(self) -> None:
        env = dict(os.environ, LINUXAI_KEEP_EVENTS_DAYS=str(120 / 86400), LINUXAI_KEEP_PLANS_DAYS=str(60 / 86400), LINUXAI_KEEP_TASKS_DAYS=str(180 / 86400),
                   LINUXAI_KEEP_JOURNAL_DAYS=str(600 / 86400), LINUXAI_KEEP_PLAYBOOK_VERSIONS="5",
                   LINUXAI_PRUNE_EVERY_SECS=str(self.prune_every))
        self.proc = subprocess.Popen([self.binary, "daemon", "--socket", self.sock, "--db", str(self.work / "core.db"),
                                      "--playbooks", str(self.work / "playbooks"), "--interval", "0.5"],
                                     stderr=open(self.work / "daemon.log", "a"), env=env)
        c = Core(self.sock, timeout=30)
        for _ in range(200):
            if c.available():
                return
            time.sleep(0.05)
        raise RuntimeError("daemon did not come up")

    def kill9(self) -> None:
        if self.proc:
            self.proc.kill()
            self.proc.wait()
        self.restarts += 1

    def proc_stat(self) -> dict:
        pid = self.proc.pid if self.proc else 0
        try:
            st = Path(f"/proc/{pid}/status").read_text()
            get = lambda k: int(next(l for l in st.splitlines() if l.startswith(k)).split()[1])  # noqa: E731
            return {"rss_kb": get("VmRSS:"), "threads": get("Threads:"), "fds": len(os.listdir(f"/proc/{pid}/fd"))}
        except (OSError, StopIteration):
            return {}


class Soak:
    def __init__(self, args):
        self.args = args
        self.work = Path(args.out or tempfile.mkdtemp(prefix="linuxai-soak-"))
        self.work.mkdir(parents=True, exist_ok=True)
        self.d = Daemon(args.core, self.work, prune_every=30)
        self.core = Core(self.d.sock, timeout=30)
        self.stop = threading.Event()
        self.down = threading.Event()        # set while the daemon is being restarted
        self.lock = threading.Lock()
        self.errors: list[str] = []
        self.latency: list[float] = []
        self.samples: list[dict] = []
        self.counts = {"events": 0, "tasks": 0, "ai_plans": 0, "answers": 0, "long_actions": 0}

    # -- helpers --
    def call(self, op: str, **kw):
        if self.down.is_set():
            return None
        t = time.perf_counter()
        try:
            r = self.core.request(op, **kw)
        except (OSError, CoreError, json.JSONDecodeError) as e:
            if not self.down.is_set():
                with self.lock:
                    self.errors.append(f"{op}: {e}")
            return None
        if op in ("ping", "state", "waiting", "stats"):
            with self.lock:
                self.latency.append(time.perf_counter() - t)
        return r

    def loop(self, fn, every: float) -> threading.Thread:
        def run():
            while not self.stop.is_set():
                try:
                    fn()
                except Exception as e:  # noqa: BLE001 - recorded, never fatal
                    if not self.down.is_set():
                        with self.lock:
                            self.errors.append(f"{fn.__name__}: {e!r}")
                self.stop.wait(every * random.uniform(0.5, 1.5))
        t = threading.Thread(target=run, daemon=True)
        t.start()
        return t

    # -- workload --
    def storm(self):
        kind = random.choice(["power.unplugged", "power.plugged", "test.tick", "organ.mail", "human.said", "service.failed"])
        data = {"unit": "soak.service"} if kind == "service.failed" else {"n": random.randint(0, 1000)}
        if kind == "human.said":
            self.call("say", text="soak")
        else:
            self.call("event", kind=kind, data=data)
        self.counts["events"] += 1

    def macro_task(self):
        n = self.counts["tasks"]
        spec = {"title": f"Soak load #{n}", "category": random.choice(["freight/negotiate", "travel/book", "bills/pay"]),
                "context": {"broker": random.choice(["@hollis", "@acme"]), "weather": random.choice(["clear", "snow"])},
                "subtasks": [
                    {"key": "a", "title": "Look around", "plan": {"steps": [{"let": "x", "value": n}, {"tell": "looked {$x}"}]}},
                    {"key": "b", "title": "Which one?", "kind": "decision", "after": ["a"], "ask": {"opts": ["A", "B"], "why": "soak"}},
                    {"key": "c", "title": "Sign it", "kind": "action", "after": ["b"]},
                    {"key": "d", "title": "Wrap up", "after": ["c"], "plan": {"steps": [{"do": "run", "program": "true"}]}}]}
        if self.call("task_create", spec=spec, source="human"):
            self.counts["tasks"] += 1

    def ai_plan(self):
        cat = random.choice(["freight/negotiate", "garden/water", None])
        plan = {"plan": f"ai-{self.counts['ai_plans']}", "steps": [{"do": "run", "program": "true"}],
                "assess": dict(ASSESS, **({"unknowns": ["soak unknown"]} if random.random() < 0.3 else {}),
                               **({"research": ["looked"]} if random.random() < 0.5 else {}))}
        if cat:
            plan["category"] = cat
        if self.call("submit", plan=plan, source="ai"):
            self.counts["ai_plans"] += 1

    def long_action(self):
        def go():
            self.call("submit", plan={"plan": "long", "steps": [{"do": "run", "program": "sleep", "args": ["2"]}]}, source="human")
        threading.Thread(target=go, daemon=True).start()
        self.counts["long_actions"] += 1

    def human(self):
        """A simulated human: answers what's waiting, after a while, in different ways."""
        r = self.call("waiting")
        for p in (r or {}).get("plans", [])[:20]:
            w = p.get("wait") or {}
            if p["status"] == "needs_thought":
                self.call("resolve", plan_id=p["plan_id"])  # no steps: end it
                continue
            if not w.get("question") or random.random() < 0.3:
                continue  # not every question gets an answer right away
            if w.get("approval"):
                text = random.choice(["yes", "yes", "always", "no, not now"])
            elif w.get("opts"):
                text = random.choice(w["opts"])
            else:
                text = "looks fine"
            if self.call("answer", plan_id=p["plan_id"], text=text):
                self.counts["answers"] += 1

    def playbook(self):
        cat = random.choice(["freight/negotiate", "travel/book"])
        self.call("playbook_put", category=cat, text=f"# {cat}\n- version {random.random()}\n", reason="soak")
        self.call("object_put", type="case", fields={"category": cat, "decision": "soak", "rate": random.uniform(2, 3)})

    def sample(self):
        s = {"t": round(time.time() - self.t0, 1), **self.d.proc_stat()}
        r = self.call("stats")
        if r:
            s.update(r["stats"])
            s["active_plans"] = r["active_plans"]
        db, wal = self.work / "core.db", self.work / "core.db-wal"
        s["db_file"] = db.stat().st_size if db.exists() else 0
        s["wal_file"] = wal.stat().st_size if wal.exists() else 0
        self.samples.append(s)
        print(json.dumps(s), flush=True)

    # -- run --
    def run(self) -> int:
        self.d.start()
        self.t0 = time.time()
        end = self.t0 + self.args.minutes * 60
        threads = [self.loop(self.storm, 0.02), self.loop(self.macro_task, 0.4), self.loop(self.ai_plan, 0.5),
                   self.loop(self.long_action, 3), self.loop(self.human, 0.3), self.loop(self.playbook, 2),
                   self.loop(self.sample, 10), self.loop(lambda: self.call("ping"), 0.2)]
        next_crash = time.time() + self.args.crash_every * 60
        while time.time() < end:
            time.sleep(1)
            if time.time() >= next_crash:
                self.down.set()
                self.d.kill9()
                time.sleep(0.5)
                self.d.start()
                self.down.clear()
                print(f"# kill -9 and restart #{self.d.restarts}", flush=True)
                next_crash = time.time() + self.args.crash_every * 60
        # Wind down: stop new work, let the simulated human finish answering, let everything settle.
        self.stop.set()
        quiet_end = time.time() + 60
        self.stop = threading.Event()
        humans = [self.loop(self.human, 0.2)]
        while time.time() < quiet_end:
            time.sleep(1)
            w = (self.call("waiting") or {}).get("plans", [])
            if not w:
                break
        self.stop.set()
        time.sleep(3)
        self.sample()
        report = self.check()
        (self.work / "report.json").write_text(json.dumps(report, indent=2))
        (self.work / "samples.json").write_text(json.dumps(self.samples))
        print(json.dumps(report, indent=2))
        self.d.proc.terminate()
        return 0 if report["pass"] else 1

    def check(self) -> dict:
        db = sqlite3.connect(self.work / "core.db")
        q = lambda sql: db.execute(sql).fetchall()  # noqa: E731
        running_plans = q("SELECT id, state FROM plans WHERE status='running'")
        stuck_tasks = q("SELECT id, status FROM tasks WHERE status IN ('running','pending') AND parent IS NULL")
        dup_actions = q("SELECT step_key, COUNT(*) c FROM journal WHERE status='done' GROUP BY step_key HAVING c > 1")
        middle = [s["rss_kb"] for s in self.samples if "rss_kb" in s][len(self.samples) // 4: 3 * len(self.samples) // 4] or [0]
        last = next((s for s in reversed(self.samples) if "rss_kb" in s), {})
        lat = sorted(self.latency) or [0]
        p = lambda q_: lat[min(len(lat) - 1, int(len(lat) * q_))]  # noqa: E731
        checks = {
            "no plan left running": len(running_plans) == 0,
            "no task tree left stuck (finished, or waiting on the human)": len(stuck_tasks) == 0,
            "no action done twice": len(dup_actions) == 0,
            "memory flat (end within 50% of the middle of the run)": last.get("rss_kb", 0) <= 1.5 * statistics.median(middle) + 4096,
            "threads back down after the load": last.get("threads", 999) <= 32,
            "file handles bounded": last.get("fds", 999) <= 64,
            "database size levels off (end within 50% of the middle of the run)":
                last.get("db_bytes", 0) <= 1.5 * statistics.median([s["db_bytes"] for s in self.samples if s.get("db_bytes")][len(self.samples) // 4: 3 * len(self.samples) // 4] or [1]) + 1_000_000,
            "p99 response under 250ms": p(0.99) < 0.25,
            "pruning ran (digests exist)": (q("SELECT COUNT(*) FROM objects WHERE type='digest'") or [[0]])[0][0] > 0,
            "events kept bounded by pruning": (q("SELECT COUNT(*) FROM events WHERE ts < strftime('%s','now') - 400") or [[1]])[0][0] == 0,
            "no unexpected errors": len(self.errors) == 0,
        }
        return {"pass": all(checks.values()), "checks": checks, "minutes": self.args.minutes, "restarts": self.d.restarts,
                "workload": self.counts, "latency_ms": {"p50": round(p(0.5) * 1000, 2), "p99": round(p(0.99) * 1000, 2),
                                                        "max": round(lat[-1] * 1000, 2), "samples": len(lat)},
                "end": last, "rss_kb_middle_median": statistics.median(middle),
                "stuck_tasks": stuck_tasks[:5], "running_plans": [r[0] for r in running_plans][:5],
                "dup_actions": dup_actions[:5], "errors": self.errors[:20], "work_dir": str(self.work)}


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--minutes", type=float, default=30)
    ap.add_argument("--crash-every", type=float, default=4, help="minutes between kill -9 restarts")
    ap.add_argument("--core", default=str(ROOT / "core" / "target" / "release" / "linuxai-core"))
    ap.add_argument("--out")
    sys.exit(Soak(ap.parse_args()).run())
