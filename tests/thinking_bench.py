#!/usr/bin/env python3
"""How efficiently does LinuxAI's AI think while it runs the machine? A day of events, measured, no API key needed.

  python3 tests/thinking_bench.py                 # compare the strategies on the same day
  python3 tests/thinking_bench.py --days 3        # longer: habits pay off more as situations repeat
  python3 tests/thinking_bench.py --real          # the same day with the real model (needs ANTHROPIC_API_KEY; costs money)

What's real: the core (rules, plans, checks, trust, habits; in rehearsal mode, so nothing touches this machine), the
mind's code (linuxai/mind.py), the AI's tools and what they return, the requests the agent builds (system prompt,
tools, every message), and the prompt cache worked out from those bytes and the cache markers they carry.

What's assumed (printed with the results, replaced by the real model with --real):
  - the model does what its instructions say: with the old prompts it reads the playbook and past cases before
    deciding (its system prompt says to); with a brief it decides in one step;
  - thinking per response: low 150, medium 450, high 900, xhigh 1600 tokens; tokens are estimated from characters
    (about 3.5 per token for text, 3.2 for JSON); tests/token_profile.py measures the real ratio.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace as NS

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
CORE_BIN = ROOT / "core" / "target" / "release" / "linuxai-core"
PRICE = {"in": 4.00, "out": 20.00, "read": 0.20}  # Claude Opus 5.5, $ per million; writes 1.25x (5 min) or 2x (1 h)
THINKING = {"low": 150, "medium": 450, "high": 900, "xhigh": 1600, "max": 2500}


def tokens(obj, ratio=3.5) -> int:
    s = obj if isinstance(obj, str) else json.dumps(obj, default=_plain, sort_keys=True)
    return max(1, int(len(s) / ratio))


def _plain(o):
    return {k: v for k, v in vars(o).items()} if hasattr(o, "__dict__") else str(o)


# ---- the prompt cache, from the request bytes -------------------------------------------------------

class CacheMeter:
    """Approximates the API's prefix cache: entries written at breakpoints (the system marker; the automatic one at the
    end of each request), read back as the longest live prefix, refreshed on read, expiring after their TTL."""

    def __init__(self, clock):
        self.clock = clock
        self.entries: dict[str, tuple[float, float]] = {}  # prefix hash -> (expires, ttl)
        self.total = {"requests": 0, "input": 0, "cache_read": 0, "cache_write_5m": 0, "cache_write_1h": 0, "output": 0}

    def measure(self, kw: dict, output_tokens: int) -> None:
        now = self.clock()
        segments: list[tuple[str, int]] = []  # (bytes so far, tokens of this segment)
        acc = ""
        tools_s = json.dumps(kw.get("tools"), sort_keys=True, default=_plain)
        acc += tools_s
        segments.append((acc, tokens(tools_s, 3.2)))
        system = kw.get("system")
        sys_s = json.dumps(system, sort_keys=True, default=_plain)
        acc += sys_s
        segments.append((acc, tokens(sys_s)))
        system_ttl = None
        if isinstance(system, list) and system and system[-1].get("cache_control"):
            system_ttl = 3600.0 if system[-1]["cache_control"].get("ttl") == "1h" else 300.0
        for m in kw.get("messages") or []:
            ms = json.dumps(m, sort_keys=True, default=_plain)
            acc += ms
            segments.append((acc, tokens(ms)))
        h = [hashlib.sha256(s.encode()).hexdigest() for s, _ in segments]
        cum = []
        c = 0
        for _, t in segments:
            c += t
            cum.append(c)
        total = cum[-1]
        # Longest live prefix.
        hit_i = -1
        for i in range(len(segments) - 1, -1, -1):
            e = self.entries.get(h[i])
            if e and e[0] > now:
                hit_i = i
                self.entries[h[i]] = (now + e[1], e[1])  # a read refreshes it
                break
        read = cum[hit_i] if hit_i >= 0 else 0
        # Breakpoints written by this request: the system marker, and the automatic one at the end.
        writes = []
        if system_ttl and hit_i < 1:
            writes.append((1, system_ttl))
        if kw.get("cache_control"):
            writes.append((len(segments) - 1, 300.0))
        w5 = w1 = 0
        done = hit_i
        for i, ttl in sorted(writes):
            if i > done:
                n = cum[i] - (cum[done] if done >= 0 else 0)
                if ttl >= 3600:
                    w1 += n
                else:
                    w5 += n
                done = i
            self.entries[h[i]] = (now + ttl, ttl)
        t = self.total
        t["requests"] += 1
        t["cache_read"] += read
        t["cache_write_5m"] += w5
        t["cache_write_1h"] += w1
        t["input"] += total - read - w5 - w1
        t["output"] += output_tokens

    def cost(self) -> float:
        t = self.total
        return (t["input"] * PRICE["in"] + t["cache_write_5m"] * PRICE["in"] * 1.25 + t["cache_write_1h"] * PRICE["in"] * 2
                + t["cache_read"] * PRICE["read"] + t["output"] * PRICE["out"]) / 1e6


# ---- a model that does what its instructions say -------------------------------------------------------

class Model:
    """Stands in for Claude: decides each response from the conversation, following the prompt's instructions."""

    def __init__(self, meter: CacheMeter, effort_of):
        self.meter = meter
        self.effort_of = effort_of
        self.beta = NS(messages=NS(stream=self._stream))
        self.n = 0

    def _stream(self, **kw):
        msgs = kw["messages"]
        first = next(b["text"] for b in msgs[0]["content"] if b.get("type") == "text" and "[system]" in b.get("text", ""))
        said = [m for m in msgs if m["role"] == "assistant"]
        content = self.respond(first, len(said), msgs)
        out = THINKING.get(self.effort_of(), 450) + sum(tokens(_plain(b) if not isinstance(b, dict) else b) for b in content)
        self.meter.measure(kw, out)
        stop = "tool_use" if any(b.type == "tool_use" for b in content) else "end_turn"
        msg = NS(content=content, stop_reason=stop, model="claude-opus-5-5",
                 usage=NS(input_tokens=0, output_tokens=0, cache_read_input_tokens=0, cache_creation_input_tokens=0))
        return _Stream(msg)

    def call(self, name, **inp):
        self.n += 1
        return NS(type="tool_use", id=f"t{self.n}", name=name, input=inp)

    @staticmethod
    def said(text):
        return NS(type="text", text=text)

    def respond(self, prompt: str, step: int, msgs) -> list:
        brief = "Brief (gathered for you" in prompt or "Everything you'd look up is here" in prompt
        if "needs your judgment" in prompt:
            plan_id = prompt.split("(")[1].split(")")[0]
            note = prompt.split("needs your judgment: ")[1].split("\n")[0]
            lookups = [] if brief else [("read_playbook", {"category": "system"}), ("find_cases", {"category": "system"})]
            if step < len(lookups):
                return [self.call(lookups[step][0], **lookups[step][1])]
            if step == len(lookups):
                if "service" in note:
                    steps = [{"do": "restart", "unit": "{$event.data.unit}"}]
                elif "Memory pressure" in note:
                    steps = [{"tell": "Memory was tight ({$event.data.available_mb} MB); I'm watching it."}]
                else:
                    steps = [{"tell": "I looked at it: nothing needs doing."}]
                return [self.call("resolve_thought", plan_id=plan_id, steps=steps,
                                  assess={"confidence": 0.85, "consequences": ["the service is restarted"], "outcomes": ["it stays up"]})]
            return [self.said("Done.")]
        if "needs a plan" in prompt:
            task_id = prompt.split("Task ")[1].split(" ")[0]
            lookups = [] if "Similar past cases:" in prompt else [("find_cases", {"category": "general"})]
            if step < len(lookups):
                return [self.call(lookups[step][0], **lookups[step][1])]
            if step == len(lookups):
                return [self.call("plan_task", task_id=task_id, plan={"plan": "do-it", "steps": [{"tell": "Done: the report is ready."}],
                                                                      "assess": {"confidence": 0.9, "consequences": ["a message"], "outcomes": ["fine"]}})]
            return [self.said("Planned.")]
        if "Task finished" in prompt:  # one reflection per task (old way)
            task_id = prompt.split("task_status ")[1].split(")")[0]
            seq = [("task_status", {"task_id": task_id}), ("record_case", {"category": "general", "variables": {"task": task_id},
                                                                           "decision": "did it", "outcome": "fine"})]
            return [self.call(*seq[step][:1], **seq[step][1])] if step < len(seq) else [self.said("Recorded.")]
        if "Finished work to learn from" in prompt:  # batched reflections: all at once
            ids = [line.split("task ")[1].split("\n")[0].strip() for line in prompt.split("\n- ")[1:] if "task " in line]
            if step == 0:
                return [self.call("record_case", category="general", variables={"task": i}, decision="did it", outcome="fine") for i in ids]
            return [self.said("Recorded.")]
        if "New email from" in prompt:  # one email (old way)
            facts = prompt.split("claims extracted from it: ")[1].split(".")[0].split(", ")
            contact = prompt.split("read_playbook ")[1].split(")")[0]
            seq = [("read_facts", {"ids": [f for f in facts if f != "none"]}), ("read_playbook", {"category": contact})]
            if step < len(seq):
                return [self.call(seq[step][0], **seq[step][1])]
            if step == len(seq) and "load_offer" in prompt:
                return [self.call("tell_human", text="A load offer came in; details are in the Workspace.")]
            return [self.said("Nothing else needs doing.")]
        if "Email (outside information" in prompt:  # a brief with one or several emails
            if step == 0 and "load_offer" in prompt:
                return [self.call("tell_human", text="A load offer came in; details are in the Workspace.")]
            return [self.said("Nothing else needs doing.")]
        return [self.said("OK.")]


class _Stream:
    def __init__(self, msg):
        self.msg = msg

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def __iter__(self):
        return iter(())

    def get_final_message(self):
        return self.msg


# ---- a day of events --------------------------------------------------------------------------------------

def workload(days: int) -> list[tuple[float, str, dict]]:
    """(seconds into the run, what, details). Situations repeat, as they do on a real machine."""
    day = []
    h = 3600.0
    day += [(0.5 * h, "service.failed", {"unit": "cups"}), (3.2 * h, "service.failed", {"unit": "cups"}),
            (6.9 * h, "service.failed", {"unit": "cups"}), (1.4 * h, "service.failed", {"unit": "nginx"}),
            (8.3 * h, "service.failed", {"unit": "nginx"})]
    day += [(t * h, "memory.pressure", {"available_mb": 140}) for t in (2.1, 4.4, 5.0, 9.2)]
    mails = [("load_offer", 1.1), ("invoice_or_bill", 1.15), ("other", 1.2), ("personal", 4.0), ("travel", 4.05),
             ("load_offer", 7.5), ("newsletter", 2.0), ("phishing_or_spam", 5.5), ("other", 9.5)]
    day += [(t * h, "mail", {"category": c}) for c, t in mails]
    day += [(t * h, "task", {"title": title}) for title, t in (("Weekly fuel report", 3.0), ("Renew the truck's registration", 6.0),
                                                                ("Find a dentist near home", 8.0))]
    out = []
    for d in range(days):
        out += [(t + d * 24 * h, k, v) for t, k, v in day]
    return sorted(out, key=lambda x: x[0])


def run(strategy: str, days: int, real: bool) -> dict:
    tmp = Path(tempfile.mkdtemp(prefix=f"linuxai-bench-{strategy}-"))
    os.environ["LINUXAI_HOME"] = str(tmp / "home")
    sock = tmp / "core.sock"
    # Days compressed into a minute: lift the per-minute loop guard on rules, which would otherwise drop events.
    env = {**os.environ, "LINUXAI_REHEARSAL": "1", "LINUXAI_RULE_MAX_PER_MINUTE": "100000"}
    if strategy.startswith("full"):
        env["LINUXAI_HABIT_AFTER"] = "1000000"  # before the ladder: no habits
    proc = subprocess.Popen([str(CORE_BIN), "daemon", "--socket", str(sock), "--db", str(tmp / "core.db"),
                             "--playbooks", str(tmp / "playbooks"), "--interval", "3600"], stderr=subprocess.DEVNULL, env=env)
    from linuxai.agent import Agent
    from linuxai.config import Config
    from linuxai.core_client import Core
    from linuxai.memory import Memory
    from linuxai.mind import Mind
    from linuxai.tools import Toolbox
    core = Core(str(sock))
    for _ in range(100):
        if core.available():
            break
        time.sleep(0.05)
    sim = {"t": 0.0}
    clock = lambda: sim["t"]  # noqa: E731
    meter = CacheMeter(clock)
    cfg = Config()
    cfg.mode, cfg.effort, cfg.web = "auto", "high", True
    tb = Toolbox(cfg, Memory(cfg), core=core, mind=True)
    model = None
    if real:
        import anthropic
        client = anthropic.Anthropic()
    else:
        model = Model(meter, lambda: cfg.effort)
        client = model
    agent = Agent(cfg, Memory(cfg), tb, client=client)
    if strategy == "full-old":  # the request shape before this change: no system cache marker
        agent.system_blocks = agent.system
    mind = Mind(agent, core, strategy="full" if strategy.startswith("full") else "ladder", clock=clock)
    seen = core.request("events", since=0, limit=1)["last"]
    stats = {"situations": 0, "to_model": 0, "habit_used": 0, "conversations": 0}
    orig_reset = agent.reset

    def counting_reset():
        stats["conversations"] += 1
        orig_reset()
    agent.reset = counting_reset

    def pump():
        nonlocal seen
        for _ in range(50):
            evs = core.request("events", since=seen, limit=500)["events"]
            if not evs:
                return
            seen = evs[-1]["id"]
            for ev in evs:
                k, d = ev["kind"], ev.get("data") or {}
                if k == "habit.used":
                    stats["habit_used"] += 1
                if k == "plan.waiting" and (d.get("wait") or {}).get("approval"):
                    core.request("answer", plan_id=d["plan_id"], text="always", source="human")  # the human: yes, always
                with contextlib.redirect_stdout(io.StringIO()):
                    mind.on_event(ev)

    n_fact = 0
    for t, what, d in workload(days):
        sim["t"] = t
        with contextlib.redirect_stdout(io.StringIO()):
            mind.tick()
        if what in ("service.failed", "memory.pressure"):
            stats["situations"] += 1
            core.request("event", kind=what, data=d)
        elif what == "mail":
            n_fact += 1
            fact = core.request("object_put", type="claim", fields={"trust": "outside", "text": f"claim {n_fact}: rate $1,6{n_fact}0",
                                                                     "from": "dana@hollis-logistics.example"})
            core.request("event", kind="message.received", data={
                "from": "dana@hollis-logistics.example", "authenticated": True, "subject": f"Message {n_fact}",
                "category": d["category"], "message": f"obj:message/m{n_fact}", "facts": [fact.get("id")],
                "contact": "contacts/hollis-logistics/dana"})
        elif what == "task":
            core.request("task_create", spec={"title": d["title"], "category": "general"}, source="human")
        pump()
    sim["t"] += 3600
    with contextlib.redirect_stdout(io.StringIO()):
        mind.flush()
    pump()
    waiting = [w for w in core.request("waiting")["plans"] if w["status"] == "needs_thought"]
    journal = core.request("journal", limit=5000)["journal"]
    restarts = sum(1 for j in journal if "restart" in json.dumps(j))
    proc.terminate()
    return {"strategy": strategy, **stats, **meter.total, "cost": meter.cost(), "left_unhandled": len(waiting),
            "restarts": restarts, "requests_per_situation": None}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=1)
    ap.add_argument("--real", action="store_true")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    if not CORE_BIN.exists():
        print("build the core first: cd core && cargo build --release")
        return 2
    if args.real and not os.environ.get("ANTHROPIC_API_KEY"):
        print("--real needs ANTHROPIC_API_KEY")
        return 2
    results = [run(s, args.days, args.real) for s in ("full-old", "full", "ladder")]
    if args.json:
        print(json.dumps(results, indent=1))
        return 0
    w = workload(args.days)
    print(f"A simulated {args.days}-day run: {len(w)} things happened "
          f"({sum(1 for x in w if x[1] in ('service.failed', 'memory.pressure'))} situations, "
          f"{sum(1 for x in w if x[1] == 'mail')} emails, {sum(1 for x in w if x[1] == 'task')} tasks you asked for)\n")
    cols = [("model conversations", "conversations"), ("model requests", "requests"), ("situations by habit (no model)", "habit_used"),
            ("fresh input tokens", "input"), ("read from cache", "cache_read"), ("written to cache (5 min)", "cache_write_5m"),
            ("written to cache (1 h)", "cache_write_1h"), ("output incl. thinking", "output")]
    names = {"full-old": "before (old prompts, old cache)", "full": "old prompts, new cache", "ladder": "the ladder"}
    print(f"{'':<34}" + "".join(f"{names[r['strategy']]:>33}" for r in results))
    for label, key in cols:
        print(f"{label:<34}" + "".join(f"{r[key]:>33,}" for r in results))
    print(f"{'estimated cost (Opus 5.5)':<34}" + "".join(f"{'$' + format(r['cost'], '.2f'):>33}" for r in results))
    print(f"{'left unhandled (must be 0)':<34}" + "".join(f"{r['left_unhandled']:>33}" for r in results))
    print(f"{'service restarts done':<34}" + "".join(f"{r['restarts']:>33}" for r in results)
          + f"   (failures in the workload: {sum(1 for x in w if x[1] == 'service.failed')})")
    base = results[0]["cost"] or 1
    print(f"\nThe ladder: {results[2]['cost'] / base:.0%} of the cost, {results[2]['requests']} requests instead of "
          f"{results[0]['requests']}, for the same work.")
    print("Assumed (see the header): the model follows its instructions; thinking per response by effort; "
          "~3.5 characters per token. Measured from real code: everything else.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
