"""Offline tests: a scripted fake model drives the real agent loop, tools, and memory."""

from __future__ import annotations

import os
import tempfile
from pathlib import Path
from types import SimpleNamespace as NS

import pytest

from linuxai import ui
from linuxai.agent import Agent
from linuxai.config import Config
from linuxai.memory import Memory
from linuxai.tools import Toolbox, is_read_only, matches_allow, suggest_prefix


@pytest.fixture()
def env(tmp_path, monkeypatch):
    monkeypatch.setenv("LINUXAI_HOME", str(tmp_path / "home"))
    cfg = Config()
    cfg.mode = "auto"
    mem = Memory(cfg)
    tb = Toolbox(cfg, mem, asker=lambda q, o: "use the blue one")
    tb.cwd = str(tmp_path)
    return cfg, mem, tb, tmp_path


# ---- read-only classifier ------------------------------------------------------------------------
@pytest.mark.parametrize("cmd", [
    "ls -la /etc", "systemctl status sshd", "journalctl -u nginx -n 50 --no-pager",
    "cat /etc/os-release | grep NAME", "git log --oneline -5", "df -h 2>/dev/null",
    "ip addr", "pacman -Qi linux", "sed -n 1,5p file", "find . -name '*.py'",
])
def test_read_only(cmd):
    assert is_read_only(cmd)


@pytest.mark.parametrize("cmd", [
    "rm -rf /tmp/x", "sudo systemctl restart nginx", "echo hi > /etc/motd", "systemctl restart x",
    "cat a | tee b", "ls $(rm x)", "find . -delete", "sed -i s/a/b/ f", "git push", "pacman -S vim",
])
def test_not_read_only(cmd):
    assert not is_read_only(cmd)


def test_allow_prefixes():
    assert suggest_prefix("sudo pacman -Syu --noconfirm") == "sudo pacman"
    assert suggest_prefix("git pull origin main") == "git pull"
    assert matches_allow("git pull origin main", ["git pull"])
    assert matches_allow("git status && git pull", ["git pull"])
    assert not matches_allow("git pull && rm -rf x", ["git pull"])
    assert not matches_allow("git pullx", ["git pull"])


# ---- tools ---------------------------------------------------------------------------------------
def test_command_keeps_cwd_and_exit_code(env):
    cfg, mem, tb, tmp = env
    (tmp / "sub").mkdir()
    out = tb.t_run_command("cd sub && pwd", "test")
    assert "exit code: 0" in out and str(tmp / "sub") in out
    assert tb.cwd == str(tmp / "sub")
    out = tb.t_run_command("false", "test")
    assert "exit code: 1" in out
    assert mem.recent_actions()[0]["exit_code"] == 1


def test_edit_file_and_decline(env, monkeypatch):
    cfg, mem, tb, tmp = env
    f = tmp / "conf.ini"
    f.write_text("a=1\nb=2\n")
    assert "Wrote" in tb.t_edit_file(str(f), "b=2", "b=3", "why")
    assert f.read_text() == "a=1\nb=3\n"
    cfg.mode = "ask"
    monkeypatch.setattr(ui, "confirm", lambda *a, **k: ui.Decision("redirect", "set b=4 instead"))
    out = tb.t_edit_file(str(f), "b=3", "b=9", "why")
    assert "set b=4 instead" in out and f.read_text() == "a=1\nb=3\n"


def test_trusted_mode_learns(env, monkeypatch):
    cfg, mem, tb, tmp = env
    cfg.mode = "trusted"
    calls = []
    monkeypatch.setattr(ui, "confirm", lambda *a, **k: calls.append(1) or ui.Decision("run", always="touch"))
    tb.t_run_command("touch x1", "t")
    tb.t_run_command("touch x2", "t")      # learned, should not ask again
    tb.t_run_command("ls", "t")            # read-only, never asks
    assert len(calls) == 1 and (tmp / "x2").exists()


def test_memory_roundtrip(env):
    cfg, mem, tb, tmp = env
    tb.t_record_solution("Fix wifi dropping after suspend", "iwlwifi drops after resume",
                         "Disable power save in NetworkManager", ["sudo tee ..."], "wifi suspend iwlwifi")
    tb.t_remember("prefers fish shell", "preferences")
    hits = tb.t_search_memory("wifi keeps dropping")
    assert "solution #1" in hits
    assert (cfg.mind_dir / "notes.md").read_text().count("fish") == 1
    assert list((cfg.mind_dir / "solutions").glob("0001-*.md"))


# ---- agent loop with a scripted model ------------------------------------------------------------
def _msg(blocks, stop):
    return NS(content=blocks, stop_reason=stop,
              usage=NS(input_tokens=10, output_tokens=5, cache_read_input_tokens=0))


def _tool(id_, name, inp):
    return NS(type="tool_use", id=id_, name=name, input=inp)


class FakeStream:
    def __init__(self, msg):
        self.msg = msg

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def __iter__(self):
        for b in self.msg.content:
            if b.type == "text":
                yield NS(type="content_block_start", content_block=NS(type="text"))
                yield NS(type="content_block_delta", delta=NS(type="text_delta", text=b.text))

    def get_final_message(self):
        return self.msg


class FakeClient:
    def __init__(self, script):
        self.script = list(script)
        self.requests = []
        self.beta = NS(messages=NS(stream=self._stream))

    def _stream(self, **kw):
        self.requests.append({**kw, "messages": list(kw["messages"])})
        return FakeStream(self.script.pop(0))


def test_agent_loop(env):
    cfg, mem, tb, tmp = env
    client = FakeClient([
        _msg([NS(type="text", text="Checking."),
              _tool("t1", "ask_human", {"question": "Which drive?", "options": ["red", "blue"]}),
              _tool("t2", "run_command", {"command": "echo hello", "why": "test"})], "tool_use"),
        _msg([_tool("t3", "record_solution", {"title": "Say hello", "problem": "need greeting",
                                              "solution": "echo it", "commands": ["echo hello"]})], "tool_use"),
        _msg([NS(type="text", text="Done.")], "end_turn"),
    ])
    agent = Agent(cfg, mem, tb, client=client)
    agent.turn("greet me")
    assert not client.script
    # tool results were sent back in a single user message, in order
    results = client.requests[1]["messages"][-1]["content"]
    assert [r["tool_use_id"] for r in results] == ["t1", "t2"]
    assert "blue one" in results[0]["content"] and "hello" in results[1]["content"]
    # first request carried machine state + the intent; fallbacks are on for the default model
    first = client.requests[0]
    assert "<machine_state>" in first["messages"][0]["content"][0]["text"]
    assert first["fallbacks"] == "default" and first["thinking"]["type"] == "adaptive"
    assert mem.list_solutions()[0]["title"] == "Say hello"
    # next turn recalls the learned solution automatically
    client.script.append(_msg([NS(type="text", text="ok")], "end_turn"))
    agent.turn("say hello again")
    assert "possibly_relevant_solutions" in client.requests[-1]["messages"][-1]["content"][0]["text"]


def test_refusal_runs_nothing(env):
    cfg, mem, tb, tmp = env
    client = FakeClient([_msg([_tool("t1", "run_command", {"command": "touch boom", "why": "x"})], "refusal")])
    agent = Agent(cfg, mem, tb, client=client)
    agent.turn("do it")
    assert not (tmp / "boom").exists()
    assert agent.messages[-1]["role"] == "user"


# ---- integration with the real LinuxAI core -------------------------------------------------------
import shutil
import subprocess
import time

CORE_BIN = Path(__file__).resolve().parents[1] / "core" / "target" / "release" / "linuxai-core"


@pytest.fixture()
def live_core(tmp_path):
    if not CORE_BIN.exists():
        pytest.skip("build core first: cargo build --release")
    sock = tmp_path / "core.sock"
    proc = subprocess.Popen([str(CORE_BIN), "daemon", "--socket", str(sock), "--db", str(tmp_path / "core.db"),
                             "--playbooks", str(tmp_path / "playbooks"), "--interval", "60"], stderr=subprocess.DEVNULL)
    from linuxai.core_client import Core
    core = Core(str(sock))
    for _ in range(50):
        if core.available():
            break
        time.sleep(0.1)
    yield core
    proc.terminate()


def test_ai_plan_through_core_needs_trust_first(env, live_core):
    from linuxai.cli import route_answer
    cfg, mem, tb, tmp = env
    tb.core = live_core
    plan = {"plan": "show-kernel", "why": "human asked",
            "assess": {"confidence": 0.9, "consequences": ["prints the kernel version"], "outcomes": ["read-only either way"]},
            "steps": [{"do": "run", "program": "uname", "args": ["-r"], "as": "k"},
                      {"tell": "kernel {$k.output}"}]}
    client = FakeClient([_msg([_tool("t1", "submit_plan", {"plan": plan})], "tool_use"),
                         _msg([NS(type="text", text="Asked you first.")], "end_turn")])
    agent = Agent(cfg, mem, tb, client=client)
    assert "LinuxAI action language" in agent.system
    agent.turn("what kernel am I on")
    result = client.requests[1]["messages"][-1]["content"][0]["content"]
    assert "is waiting for the human" in result and "always" in result
    # The human answers "always" in the session: goes straight to the plan, no model involved.
    waiting = live_core.request("waiting")["plans"]
    pending = {waiting[0]["plan_id"]: waiting[0]["wait"]}
    assert route_answer(live_core, pending, "always")
    tells = [e for e in live_core.request("events")["events"] if e["kind"] == "ai.tell"]
    assert tells and tells[-1]["data"]["text"].startswith("kernel ")
    # Trust learned: the same kind of plan now runs immediately.
    r = live_core.request("submit", plan=plan, source="ai")
    assert r["result"]["status"] == "done"


def test_mind_resolves_a_thought(env, live_core):
    cfg, mem, tb, tmp = env
    tb.core, tb.mind = live_core, True
    live_core.request("submit", source="human",
                      plan={"plan": "odd", "steps": [{"think": "something odd happened"}]})
    w = live_core.request("waiting")["plans"][0]
    assert w["status"] == "needs_thought"
    client = FakeClient([
        _msg([_tool("t1", "resolve_thought", {"plan_id": w["plan_id"],
                                              "steps": [{"tell": "Looked into it; all fine."}]})], "tool_use"),
        _msg([NS(type="text", text="done")], "end_turn")])
    Agent(cfg, mem, tb, client=client).turn("[system] resolve it")
    assert live_core.request("waiting")["plans"] == []
    # Behind the scenes, changes must go through plans, not raw commands.
    assert "read-only" in tb.t_run_command("touch /tmp/x", "t")


def test_reach_messages_explain_why(tmp_path):
    from linuxai.reach import message_for, send
    ev = {"kind": "plan.waiting", "data": {"plan_id": "p1", "plan": "freight", "wait": {
        "question": "Take $1,780 ($2.28/mi)?", "opts": ["take", "hold"], "why": "Further under your floor than before.",
        "lean": "take", "fallback": "hold", "until": None}}}
    text = message_for(ev)
    assert "LinuxAI needs you: Take $1,780" in text and "Why I'm unsure: Further under" in text
    assert "I'd lean: take" in text and "Reply: take / hold" in text and "I'll go with: hold" in text
    assert message_for({"kind": "plan.reminder", "data": ev["data"]}).startswith("Reminder: ")
    assert message_for({"kind": "action.done", "data": {}}) is None
    out = tmp_path / "sms.txt"
    assert send(text, f"cat > {out}") and out.read_text() == text


def test_undecided_plan_reaches_human_via_live_core(env, live_core, tmp_path):
    from linuxai.reach import message_for
    plan = {"plan": "q", "steps": [{"ask": "Pay Verizon $112.40?", "why": "37% above usual", "lean": "ask about the new charge first",
                                    "opts": ["pay", "hold"]}]}
    live_core.request("submit", plan=plan, source="human")
    w = live_core.request("waiting")["plans"][0]
    text = message_for({"kind": "plan.waiting", "data": w})
    assert "Why I'm unsure: 37% above usual" in text


def test_core_survives_kill_9_mid_plan(tmp_path):
    """Kill the real daemon in the middle of a plan; on restart it finishes the plan."""
    if not CORE_BIN.exists():
        pytest.skip("build core first")
    from linuxai.core_client import Core
    sock, db = str(tmp_path / "s"), str(tmp_path / "core.db")

    def start():
        p = subprocess.Popen([str(CORE_BIN), "daemon", "--socket", sock, "--db", db, "--interval", "0.3"],
                             stderr=subprocess.DEVNULL)
        c = Core(sock, timeout=10)
        for _ in range(50):
            if c.available():
                return p, c
            time.sleep(0.1)
        raise RuntimeError("core did not start")

    proc, core = start()
    plan = {"plan": "slow-job", "steps": [
        {"do": "run", "program": "sh", "args": ["-c", f"echo started >> {tmp_path}/ran; sleep 3"]},
        {"do": "run", "program": "sh", "args": ["-c", f"echo second >> {tmp_path}/ran"]},
        {"tell": "slow job finished"}]}
    import threading
    threading.Thread(target=lambda: _ignore(lambda: core.request("submit", plan=plan, source="human")), daemon=True).start()
    time.sleep(1.0)
    proc.kill()          # no chance to clean up
    proc.wait()
    proc, core = start()  # restart: recovery happens before it serves
    try:
        for _ in range(100):
            evs = core.request("events", since=0, limit=10000)["events"]
            if any(e["kind"] == "ai.tell" and e["data"].get("text") == "slow job finished" for e in evs):
                break
            time.sleep(0.2)
        else:
            raise AssertionError("plan was not finished after restart")
        assert any(e["kind"] == "plan.resumed" for e in evs)
        assert (tmp_path / "ran").read_text().splitlines()[-1] == "second"
    finally:
        proc.terminate()


def _ignore(f):
    try:
        f()
    except Exception:  # noqa: BLE001 - the daemon is killed mid-request on purpose
        pass


def test_ai_plans_a_task_from_its_playbook_then_learns(env, live_core, tmp_path):
    cfg, mem, tb, tmp = env
    tb.core, tb.mind = live_core, True
    live_core.request("playbook_put", category="freight/negotiate", reason="your floor rule",
                      text="# Negotiating\n## How I decide\n- Floor $2.40/mi.\n")
    r = live_core.request("task_create", source="ai", spec={
        "title": "Negotiate the Hollis load", "category": "freight/negotiate/hollis",
        "context": {"broker": "@hollis", "offer_per_mile": 2.10, "weather": "clear"}})
    task_id = r["task"]
    assert r["tree"]["status"] == "needs_plan"
    client = FakeClient([
        _msg([_tool("t1", "read_playbook", {"category": "freight/negotiate/hollis"}),
              _tool("t2", "find_cases", {"category": "freight/negotiate"})], "tool_use"),
        _msg([_tool("t3", "plan_task", {"task_id": task_id, "plan": {"steps": [
            {"ask": "Hollis offers $2.10/mi. Counter at $2.50?", "opts": ["yes", "no"], "why": "Below your $2.40 floor",
             "lean": "yes", "as": "go"},
            {"tell": "Countering Hollis at $2.50/mi"}]}})], "tool_use"),
        _msg([NS(type="text", text="Asked the human.")], "end_turn"),
        # reflection after the task finishes
        _msg([_tool("t4", "record_case", {"category": "freight/negotiate", "variables": {"broker": "@hollis", "weather": "clear",
                                                                                         "offer_per_mile": 2.10},
                                          "decision": "countered at $2.50", "outcome": "human approved"}),
              _tool("t5", "update_playbook", {"category": "freight/negotiate/hollis", "reason": "first Hollis negotiation",
                                              "text": "# Hollis\n## What matters\n- Opens around $2.10/mi; counter at $2.50.\n"})], "tool_use"),
        _msg([NS(type="text", text="Learned.")], "end_turn"),
    ])
    agent = Agent(cfg, mem, tb, client=client)
    agent.turn(f"[system] Task {task_id} needs a plan")
    # The playbook reached the model, and the task now waits on the human's decision.
    first_results = client.requests[1]["messages"][-1]["content"]
    assert "Floor $2.40/mi" in first_results[0]["content"]
    tree = live_core.request("task", task=task_id)["tree"]
    assert tree["status"] == "waiting"
    plan_id = tree["plan_id"]
    assert live_core.request("answer", plan_id=plan_id, text="yes")["result"]["status"] == "done"
    assert live_core.request("task", task=task_id)["tree"]["status"] == "done"
    agent.reset()
    agent.turn("[system] Task finished: reflect")
    chain = live_core.request("playbook", category="freight/negotiate/hollis")["chain"]
    assert [p["category"] for p in chain] == ["freight/negotiate", "freight/negotiate/hollis"]
    cases = live_core.request("find", spec={"type": "case", "where": {"broker": "@hollis"}})["found"]
    assert cases and cases[0]["decision"] == "countered at $2.50"



def test_reach_tells_the_human_about_blockers():
    from linuxai.reach import message_for
    text = message_for({"kind": "task.failed", "data": {"title": "Book the hotel", "error": "sold out"}})
    assert "blocked on 'Book the hotel': sold out" in text


def test_unassessed_ai_plan_is_sent_back_with_instructions(env, live_core):
    cfg, mem, tb, tmp = env
    tb.core = live_core
    out = tb.t_submit_plan({"plan": "x", "steps": [{"do": "run", "program": "true"}]})
    assert "rejected" in out and "assess" in out
