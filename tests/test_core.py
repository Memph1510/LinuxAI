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
