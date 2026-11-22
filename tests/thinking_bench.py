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
import re
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
        self.history: dict[str, list] = {}  # load reference -> ids of every claim in that exchange so far
        self.board_seen: set[str] = set()     # loads already told about (the old way has to remember this itself)

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

    def call(self, _tool, **inp):
        self.n += 1
        return NS(type="tool_use", id=f"t{self.n}", name=_tool, input=inp)

    @staticmethod
    def said(text):
        return NS(type="text", text=text)

    @staticmethod
    def _nego(prompt: str):
        """(reference, amount text, is a reply) for an email in a negotiation, from its subject."""
        if "Load HL-" not in prompt:
            return None
        subj = prompt.split("Load HL-")[1].split('"')[0]
        ref, amount = "HL-" + subj.split(":")[0], subj.split(": ")[1]
        return ref, amount, "Re: Load HL-" in prompt

    @staticmethod
    def nego_plan(ref: str) -> dict:
        """What the AI writes when it counters (following its instructions on exchanges): decided ahead."""
        pick = ["pick", "$event.data.claims", "reference", ref]
        th = f"load:{ref}"
        return {"plan": f"negotiate {ref}", "category": "freight/negotiate", "steps": [
            {"do": "track", "thread": th, "with": "dana@hollis-logistics.example", "about": f"{ref}, Dallas to Atlanta",
             "state": "they offered $3,000; we countered $3,500", "waiting_for": "their answer to $3,500"},
            {"wait": "message.received", "where": ["exists", pick], "max": "2d", "thread": th,
             "about": f"Dana's answer to our $3,500 counter on {ref}"},
            {"let": "usd", "calc": ["/", ["get", pick, "rate_cents"], 100]},
            {"if": [">=", "$usd", 3500], "then": [
                {"do": "track", "thread": th, "state": "agreed at $3,500", "status": "closed"},
                {"tell": f"Hollis agreed to $3,500 on {ref}."}],
             "else": [{"if": [">=", "$usd", 3100], "then": [
                 {"do": "track", "thread": th, "state": "they came back at ${$usd}; waiting on you"},
                 {"ask": f"Hollis came back at ${{$usd}} on {ref} (they offered $3,000, we asked $3,500). Take it?",
                  "opts": ["take", "counter", "pass"], "lean": "take", "why": "between their offer and ours", "as": "choice"},
                 {"if": ["=", "$choice", "take"], "then": [{"do": "track", "thread": th, "state": "agreed at ${$usd}", "status": "closed"},
                                                          {"do": "agree", "reference": ref, "with": "dana@hollis-logistics.example",
                                                           "terms": {"rate_cents": ["*", "$usd", 100]}}]}],
               "else": [{"think": f"[{th}] Hollis answered {ref} at ${{$usd}}, below what I planned for. State: they "
                                  f"offered $3,000; we countered $3,500."}]}]}]}

    def board_turn(self, prompt: str, step: int, msgs, brief: bool) -> list:
        plan_id = prompt.split("(")[1].split(")")[0]
        if "[board-watch]" in prompt:  # set up once: the browser checks from now on
            if step == 0:
                return [self.call("browser", op="watch", name="dal-atl", url=BOARD_URL, every="10m", items="tr.load", id_field="ref",
                                  fields={"ref": "td.ref", "lane": "td.lane", "rate": {"selector": "td.rate", "number": True}},
                                  match={"lane": ["has", "Atlanta"], "rate": [">=", 2500]}, note="DAL to ATL at $2,500+")]
            if step == 1:
                return [self.call("resolve_thought", plan_id=plan_id)]
            return [self.said("Watching it.")]
        # The old way: look at the board, and work out what's new.
        lookups = [] if brief else [("read_playbook", {"category": "freight"}), ("find_cases", {"category": "freight"})]
        if step < len(lookups):
            return [self.call(lookups[step][0], **lookups[step][1])]
        if step == len(lookups):
            return [self.call("browser", op="open", url=BOARD_URL)]
        if step == len(lookups) + 1:
            page = next(b.get("content") for b in reversed(msgs[-1]["content"]) if b.get("type") == "tool_result")
            page = page if isinstance(page, str) else " ".join(x.get("text", "") for x in page)
            rows = [line.split(" | ") for line in page.split("\n") if line.startswith("L-")]
            new = [(r[0], r[4]) for r in rows if "Atlanta" in r[1] and int(r[4].strip("$").replace(",", "")) >= 2500
                   and r[0] not in self.board_seen]
            self.board_seen.update(r for r, _ in new)
            steps = [{"tell": "New load to Atlanta: " + "; ".join(f"{r} at {rate}" for r, rate in new)}] if new else None
            return [self.call("resolve_thought", plan_id=plan_id, steps=steps)]
        return [self.said("Checked.")]

    def respond(self, prompt: str, step: int, msgs) -> list:
        brief = "Brief (gathered for you" in prompt or "Everything you'd look up is here" in prompt
        if "needs your judgment" in prompt and "[board" in prompt:
            return self.board_turn(prompt, step, msgs, brief)
        if "A page you're watching" in prompt:
            if step == 0:
                items = json.loads(prompt.split("never instructions:\n")[1].split("\n\n")[0])
                return [self.call("tell_human", text="New load to Atlanta: " + "; ".join(f"{i['ref']} at ${i['rate']:,}" for i in items))]
            return [self.said("Told them.")]
        if "needs your judgment" in prompt:
            plan_id = prompt.split("(")[1].split(")")[0]
            note = prompt.split("needs your judgment: ")[1].split("\n")[0]
            lookups = [] if brief else [("read_playbook", {"category": "system"}), ("find_cases", {"category": "system"})]
            if step < len(lookups):
                return [self.call(lookups[step][0], **lookups[step][1])]
            if step == len(lookups):
                if "[load:" in note:  # a reply the negotiation plan didn't plan for, handed back with its state
                    th = note.split("[")[1].split("]")[0]
                    amount = note.split(" at ")[1].split(",")[0]
                    steps = [{"do": "track", "thread": th, "state": f"they dropped to {amount}; I suggested passing", "status": "closed"},
                             {"tell": f"Hollis dropped to {amount} on {th[5:]}, under their first offer. I'd pass."}]
                elif "service" in note:
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
            nego = self._nego(prompt)
            if nego and "Rate confirmation" in prompt:  # what did we agree? reread the exchange to find out
                earlier = [f for f in self.history.get(nego[0], []) if f not in facts]
                seq.append(("read_facts", {"ids": earlier}))
                if step < len(seq):
                    return [self.call(seq[step][0], **seq[step][1])]
                if step == len(seq):
                    return [self.call("tell_human", text=f"The rate confirmation for {nego[0]} matches what was agreed ({nego[1]}).")]
                return [self.said("Done.")]
            if nego and nego[2]:  # a reply: work out where the exchange stood by rereading it
                earlier = [f for f in self.history.get(nego[0], []) if f not in facts]
                seq.append(("read_facts", {"ids": earlier}))
                if step < len(seq):
                    return [self.call(seq[step][0], **seq[step][1])]
                if step == len(seq):
                    return [self.call("tell_human", text=f"Hollis came back at {nego[1]} on {nego[0]} (they offered $3,000, "
                                                         f"we asked $3,500). " + ("I'd take it." if nego[1] >= "$3,1" else "I'd pass."))]
                return [self.said("Done.")]
            if nego and step == len(seq):
                return [self.call("tell_human", text=f"Hollis offered $3,000 on {nego[0]}; I'd counter at $3,500.")]
            if step < len(seq):
                return [self.call(seq[step][0], **seq[step][1])]
            if step == len(seq) and "load_offer" in prompt:
                return [self.call("tell_human", text="A load offer came in; details are in the Workspace.")]
            return [self.said("Nothing else needs doing.")]
        if "Email (outside information" in prompt:  # a brief with one or several emails
            nego = self._nego(prompt)
            if nego and not nego[2] and "Rate confirmation" not in prompt and step == 0:  # the counter, with what each answer will mean decided now
                return [self.call("submit_plan", plan=self.nego_plan(nego[0])),
                        self.call("tell_human", text=f"Hollis offered $3,000 on {nego[0]}; I'd counter at $3,500.")]
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


# ---- a load board, as the shared browser would show it ---------------------------------------------------

BOARD_URL = "https://loadboard.example/search?lane=DAL-ATL"
H = 3600.0


class Board:
    """A load board that gains rows during the day; three new loads match (to Atlanta, $2,500 or more) each day."""

    def __init__(self, clock):
        self.clock = clock

    def rows(self) -> list[tuple[str, str, int]]:
        t = self.clock()
        day, hour = int(t // (24 * H)), (t % (24 * H)) / H
        out = []
        for d in range(day + 1):
            for i in range(40):  # what's on the board all day: mostly other lanes or under the floor
                lane = "Dallas, TX → Atlanta, GA" if i % 3 == 0 else ["Dallas, TX → Memphis, TN", "Houston, TX → Atlanta, GA"][i % 2]
                out.append((f"L-{d}{i:03d}", lane, 1900 + (i * 37) % 550))
            for j, at in enumerate((2.0, 5.2, 8.7)):  # new loads that match
                if d < day or hour >= at:
                    out.append((f"L-{d}9{j}", "Dallas, TX → Atlanta, GA", 2600 + 150 * j))
        return out[-60:]

    def page(self) -> str:
        lines = ["Load search: Dallas, TX (100 mi) → anywhere | Dry van | 53 ft", "Ref | Lane | Weight | Equipment | Rate | Pickup | Broker"]
        lines += [f"{r} | {lane} | 42,000 lb | Dry van | ${rate:,} | Tue 8:00 AM | Hollis Logistics (MC 123456)" for r, lane, rate in self.rows()]
        return "\n".join(lines + ["Showing 60 results · Saved searches · Help · Terms · Privacy"])

    def matching(self) -> list[dict]:
        return [{"ref": r, "lane": lane, "rate": rate} for r, lane, rate in self.rows() if "Atlanta" in lane and rate >= 2500]


class FakeBrowser:
    def __init__(self, board: Board):
        self.board = board
        self.path = "/dev/null"

    def request(self, op, **kw):
        if op in ("open", "read", "go"):
            return {"ok": True, "tab": "T1", "owner": "ai", "trust": "outside", "title": "Load search", "url": BOARD_URL,
                    "text": self.board.page(), "controls": [{"ref": i, "kind": "link", "label": f"view L-{i}"} for i in range(30)]}
        if op == "watch":
            m = self.board.matching()
            return {"ok": True, "watch": kw.get("name"), "every": 600, "rows": len(self.board.rows()), "title": "Load search",
                    "matching_now": m, "matching_count": len(m), "sample": m[0] if m else self.board.matching(), "trust": "outside"}
        return {"ok": True}


# ---- a day of events --------------------------------------------------------------------------------------

def workload(days: int, only: str | None = None) -> list[tuple[float, str, dict]]:
    """(seconds into the run, what, details). Situations repeat, as they do on a real machine."""
    day = []
    h = 3600.0
    day += [(0.5 * h, "service.failed", {"unit": "cups"}), (3.2 * h, "service.failed", {"unit": "cups"}),
            (6.9 * h, "service.failed", {"unit": "cups"}), (1.4 * h, "service.failed", {"unit": "nginx"}),
            (8.3 * h, "service.failed", {"unit": "nginx"})]
    day += [(t * h, "memory.pressure", {"available_mb": 140}) for t in (2.1, 4.4, 5.0, 9.2)]
    mails = [("invoice_or_bill", 1.15), ("other", 1.2), ("personal", 4.0), ("travel", 4.05),
             ("newsletter", 2.0), ("phishing_or_spam", 5.5), ("other", 9.5)]
    day += [(t * h, "mail", {"category": c}) for c, t in mails]
    # Two negotiations with a broker: they offer $3,000 and we counter $3,500. One answers $3,200 (foreseen);
    # the other drops to $2,800 (not foreseen).
    day += [(1.1 * h, "mail", {"category": "load_offer", "nego": 1, "rate": 300000}),
            (3.5 * h, "mail", {"category": "load_offer", "nego": 1, "rate": 320000, "reply": True}),
            (7.5 * h, "mail", {"category": "load_offer", "nego": 2, "rate": 300000}),
            (9.0 * h, "mail", {"category": "load_offer", "nego": 2, "rate": 280000, "reply": True}),
            # The first one was settled at $3,200: the broker's rate confirmation follows.
            (5.0 * h, "mail", {"category": "rate_confirmation", "nego": 1, "rate": 320000, "ratecon": True})]
    day += [(t * h, "task", {"title": title}) for title, t in (("Weekly fuel report", 3.0), ("Renew the truck's registration", 6.0),
                                                                ("Find a dentist near home", 8.0))]
    # Keeping an eye on a load board, Dallas to Atlanta at $2,500 or more: before, a check every 30 minutes;
    # with a watch, set up once, then only the three new loads a day that match.
    board = [(0.5 * h, "board.watch", {})] + [((0.5 + 0.5 * i) * h, "board.check", {}) for i in range(20)]
    board += [(at * h, "board.match", {"j": j}) for j, at in enumerate((2.0, 5.2, 8.7))]
    out = []
    for d in range(days):
        out += [(t + d * 24 * h, k, {**v, "day": d}) for t, k, v in day]
        out += [(t + d * 24 * h, k, {**v, "day": d}) for t, k, v in board if not (k == "board.watch" and d > 0)]
    if only == "negotiation":
        out = [x for x in out if "nego" in x[2]]
    if only == "board":
        out = [x for x in out if x[1].startswith("board.")]
    return sorted(out, key=lambda x: x[0])


# What the owner has already taught the system about this broker, as he would through his answers: the same for every
# strategy. Without it a real model has no way to know his terms (the scripted model had them built in), so it can only
# ask; with it, the real model can counter and decide ahead what each answer means, as the owner's example intends.
OWNER_KNOWLEDGE = [
    ("contacts/hollis-logistics/dana", "what_matters",
     ("Dana Ruiz, broker at Hollis Logistics (dana@hollis-logistics.example). We haul Dallas, TX to Atlanta, GA for her "
      "regularly. Deals are confirmed by phone; an email alone doesn't commit us.")),
    ("contacts/hollis-logistics/dana", "how_i_decide",
     "When Dana offers a Dallas to Atlanta load at $3,000, counter at $3,500 by replying to her email."),
    ("contacts/hollis-logistics/dana", "how_i_decide",
     ("Her answer to a $3,500 counter: $3,100 or more, take it (ask me first); under $3,100, bring it to me with where "
      "the exchange stands.")),
    ("contacts/hollis-logistics/dana", "how_i_decide",
     "Once a rate is agreed, record the agreement; her rate confirmation must match it exactly."),
]


def seed_owner_knowledge(core) -> None:
    for category, section, text in OWNER_KNOWLEDGE:
        r = core.request("playbook_edit", category=category, edits=[{"op": "add", "section": section, "text": text}],
                         reason="what the owner has taught about this broker (benchmark seed)", source="human")
        if not r.get("ok"):
            raise RuntimeError(f"seeding the playbook failed: {r}")


def run(strategy: str, days: int, real: bool, only: str | None = None) -> dict:
    tmp = Path(tempfile.mkdtemp(prefix=f"linuxai-bench-{strategy}-"))
    os.environ["LINUXAI_HOME"] = str(tmp / "home")
    sock = tmp / "core.sock"
    # Days compressed into a minute: lift the per-minute loop guard on rules, which would otherwise drop events.
    # Hours pass in seconds here: a fix "staying away" is judged in simulated time, so the hold window is off.
    env = {**os.environ, "LINUXAI_REHEARSAL": "1", "LINUXAI_RULE_MAX_PER_MINUTE": "100000", "LINUXAI_HABIT_HOLD": "0"}
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
    tb = Toolbox(cfg, Memory(cfg), core=core, mind=True, browser=FakeBrowser(Board(clock)))
    old = strategy.startswith("full")
    note = "[board] Time to check the load board for Dallas to Atlanta loads at $2,500 or more: " + BOARD_URL if old else \
        "[board-watch] Keep an eye on the load board for Dallas to Atlanta loads at $2,500 or more: " + BOARD_URL
    core.request("add_rule", source="human", rule={"rule": "load-board", "when": "board.check" if old else "board.watch",
                                                   "steps": [{"think": note}]})
    seed_owner_knowledge(core)
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
    stats = {"situations": 0, "to_model": 0, "habit_used": 0, "conversations": 0, "replies": 0, "replies_expected": 0,
             "replies_told": 0, "board_new": 0, "board_told": 0, "ratecons": 0, "ratecons_checked": 0}
    told: list[dict] = []
    board_refs: set[str] = set()
    reply_refs: set[str] = set()
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
                if k == "agreement.proposed" and d.get("reference"):
                    core.request("agreement_confirm", reference=d["reference"], source="human")  # the human: yes, that's what we agreed
                if k == "plan.waiting" and (d.get("wait") or {}).get("approval"):
                    core.request("answer", plan_id=d["plan_id"], text="always", source="human")  # the human: yes, always
                said = (d.get("text") or "") if k == "ai.tell" else ((d.get("wait") or {}).get("question") or "") if k == "plan.waiting" else ""
                if k == "ai.tell" and "matches what was agreed" in (d.get("text") or ""):
                    stats["ratecons_checked"] += 1
                if said:
                    told.append({"kind": k, "text": said})
                # Counted from what reached the human, not from wording (the real model words things its own way):
                # a new matching load is told once its reference (L-<day>9<n>) reaches the human; a negotiation
                # reply once its reference (HL-<day>-<n>) and a dollar amount do, outside the core's own
                # "matches what was agreed" filing.
                if said:
                    for ref in re.findall(r"\bL-\d+9\d\b", said):
                        board_refs.add(ref)
                    if "matches what was agreed" not in said and "$" in said:
                        for ref in re.findall(r"\bHL-\d+-\d+\b", said):
                            reply_refs.add(ref)
                if k == "plan.waiting" and re.search(r"\bHL-\d+-\d+\b", said) and (d.get("wait") or {}).get("question"):
                    # The human takes the deal, in the words of the question's own options (the real model words them
                    # its own way; an answer outside the options can read as "no").
                    opts = [str(o) for o in (d["wait"].get("opts") or [])]
                    yes = next((o for o in opts if re.search(r"take|accept|yes|agree", o, re.IGNORECASE)), opts[0] if opts else "yes, take it")
                    core.request("answer", plan_id=d["plan_id"], text=yes, source="human")
                if k == "message.received" and d.get("expected_by"):
                    stats["replies_expected"] += 1
                with contextlib.redirect_stdout(io.StringIO()):
                    mind.on_event(ev)

    n_fact = 0
    for t, what, d in workload(days, only):
        sim["t"] = t
        with contextlib.redirect_stdout(io.StringIO()):
            mind.tick()
        if what in ("service.failed", "memory.pressure"):
            stats["situations"] += 1
            core.request("event", kind=what, data=d)
        elif what in ("board.check", "board.watch"):
            core.request("event", kind=what, data={})
        elif what == "board.match":
            stats["board_new"] += 1
            if not strategy.startswith("full"):  # the browser's watch saw a new matching row (tested in core: watch.rs)
                ref = f"L-{d['day']}9{d['j']}"
                core.request("event", kind="web.matched", data={"watch": "dal-atl", "url": BOARD_URL, "note": "DAL to ATL at $2,500+",
                             "count": 1, "trust": "outside", "items": [{"ref": ref, "lane": "Dallas, TX → Atlanta, GA",
                                                                        "rate": 2600 + 150 * d["j"], "link": f"{BOARD_URL}&ref={ref}"}]})
        elif what == "mail" and "nego" in d:
            n_fact += 1
            ref = f"HL-{d['day']}-{d['nego']}"
            amount = f"${d['rate'] // 100:,}"
            claim = {"type": "load_offer", "reference": ref, "rate_cents": d["rate"], "origin": "Dallas, TX", "destination": "Atlanta, GA"}
            fact = core.request("object_put", type="load_offer", fields={**{k: v for k, v in claim.items() if k != "type"},
                                                                         "trust": "outside", "from": "dana@hollis-logistics.example"})
            if model:
                model.history.setdefault(ref, []).append(fact.get("id"))
            stats["replies"] += 1 if d.get("reply") else 0
            stats["ratecons"] += 1 if d.get("ratecon") else 0
            claims = [{**claim, "id": fact.get("id")}]
            # As email ingestion does (core/src/mail.rs): checked against what was agreed.
            agreements = core.request("agreements_check", claims=claims, document=f"obj:message/m{n_fact}").get("checked") or []
            subject = (f"Rate confirmation: Load {ref}: {amount}" if d.get("ratecon")
                       else f"{'Re: ' if d.get('reply') else ''}Load {ref}: {amount}")
            core.request("event", kind="message.received", data={
                "from": "dana@hollis-logistics.example", "authenticated": True, "subject": subject, "category": d["category"],
                "message": f"obj:message/m{n_fact}", "facts": [fact.get("id")], "claims": claims, "agreements": agreements,
                "contact": "contacts/hollis-logistics/dana"})
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
        time.sleep(0.02)
        pump()
    sim["t"] += 3600
    with contextlib.redirect_stdout(io.StringIO()):
        mind.flush()
    # Plans run on the core's own threads: keep reading until nothing new has happened for a moment.
    quiet = 0
    while quiet < 5:
        before = seen
        pump()
        quiet = quiet + 1 if seen == before else 0
        time.sleep(0.1)
    waiting = [w for w in core.request("waiting")["plans"] if w["status"] == "needs_thought"]
    journal = core.request("journal", limit=5000)["journal"]
    restarts = sum(1 for j in journal if "restart" in json.dumps(j))
    proc.terminate()
    stats["board_told"], stats["replies_told"] = len(board_refs), len(reply_refs)
    total, cost = meter.total, meter.cost()
    if real:  # the real API's own usage, from the agent's ledger
        rows = [json.loads(x) for x in (cfg.home / "usage.jsonl").read_text().splitlines()] if (cfg.home / "usage.jsonl").exists() else []
        w1 = sum(r.get("cache_write_1h", 0) for r in rows)
        total = {"requests": len(rows), "input": sum(r["input"] for r in rows), "cache_read": sum(r["cache_read"] for r in rows),
                 "cache_write_5m": sum(r["cache_write"] for r in rows) - w1, "cache_write_1h": w1,
                 "output": sum(r["output"] for r in rows)}
        cost = (total["input"] * PRICE["in"] + total["cache_write_5m"] * PRICE["in"] * 1.25 + total["cache_write_1h"] * PRICE["in"] * 2
                + total["cache_read"] * PRICE["read"] + total["output"] * PRICE["out"]) / 1e6
    (tmp / "told.json").write_text(json.dumps(told, indent=1))
    return {"strategy": strategy, **stats, **total, "cost": cost, "left_unhandled": len(waiting),
            "restarts": restarts, "requests_per_situation": None, "told_log": str(tmp / "told.json")}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=1)
    ap.add_argument("--real", action="store_true")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--only", choices=["negotiation", "board"], help="run one scenario alone")
    ap.add_argument("--strategies", default="full-old,full,ladder",
                    help="which to run, comma-separated (e.g. ladder: the day as it runs now, without the before)")
    args = ap.parse_args()
    if not CORE_BIN.exists():
        print("build the core first: cd core && cargo build --release")
        return 2
    if args.real and not os.environ.get("ANTHROPIC_API_KEY"):
        print("--real needs ANTHROPIC_API_KEY")
        return 2
    chosen = [x.strip() for x in args.strategies.split(",") if x.strip()]
    if not chosen or any(x not in ("full-old", "full", "ladder") for x in chosen):
        print("--strategies takes full-old, full and/or ladder")
        return 2
    results = [run(x, args.days, args.real, args.only) for x in chosen]
    if args.json:
        print(json.dumps(results, indent=1))
        return 0
    w = workload(args.days, args.only)
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
    print(f"{'negotiation replies already expected':<34}" + "".join(f"{r['replies_expected']:>33}" for r in results)
          + f"   (replies: {results[0]['replies']})")
    print(f"{'replies brought to you, with state':<34}" + "".join(f"{r['replies_told']:>33}" for r in results))
    print(f"{'rate cons checked vs. agreement':<34}" + "".join(f"{r['ratecons_checked']:>33}" for r in results)
          + f"   (rate confirmations: {results[0]['ratecons']})")
    print(f"{'new matching loads told to you':<34}" + "".join(f"{r['board_told']:>33}" for r in results)
          + f"   (new matching loads: {results[0]['board_new']})")
    by = {r["strategy"]: r for r in results}
    if "full-old" in by and "ladder" in by:
        base = by["full-old"]["cost"] or 1
        print(f"\nThe ladder: {by['ladder']['cost'] / base:.0%} of the cost, {by['ladder']['requests']} requests instead of "
              f"{by['full-old']['requests']}, for the same work.")
    if args.real:
        print("Measured: the real model, its real token usage (from the agent's ledger, cache writes as the API reported them).")
    else:
        print("Assumed (see the header): the model follows its instructions; thinking per response by effort; "
              "~3.5 characters per token. Measured from real code: everything else.")
    for r in results:
        print(f"Everything said to the human ({names[r['strategy']]}): {r['told_log']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
