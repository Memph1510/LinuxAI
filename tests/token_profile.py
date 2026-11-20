#!/usr/bin/env python3
"""Exact token profile of what LinuxAI's AI sends on every request (the fixed prefix), per part and per tool.

  ANTHROPIC_API_KEY=... python3 tests/token_profile.py [--model claude-opus-5-5]

Uses the free token-counting endpoint (no model call, no cost). Prints the system prompt, the whole tool set,
each tool's own share, and what one request's fixed prefix costs uncached, written to cache, and read from cache.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

PRICES = {"claude-opus-5-5": 4.00, "claude-sonnet-5-5": 2.00, "claude-haiku-5-5": 0.10}  # $ per million input tokens
CACHE_READ = {"claude-opus-5-5": 0.20, "claude-sonnet-5-5": 0.10, "claude-haiku-5-5": 0.01}


def build(model: str):
    """The session agent's system prompt and tools, exactly as LinuxAI builds them."""
    os.environ.setdefault("LINUXAI_HOME", tempfile.mkdtemp())
    from linuxai import agent as A
    from linuxai import tools as T
    from linuxai.config import Config
    from linuxai.memory import Memory

    core_bin = ROOT / "core" / "target" / "release" / "linuxai-core"

    class Core:  # just enough of the core for the system prompt (its verb list)
        path = "/nonexistent"

        def request(self, op, **kw):
            if op == "verbs":
                return {"verbs": json.loads(subprocess.run([str(core_bin), "verbs"], capture_output=True, text=True).stdout)}
            return {"ok": True}

    cfg = Config()
    cfg.model = model
    tb = T.Toolbox(cfg, Memory(cfg), core=Core(), browser=object(), screen=None)
    ag = A.Agent(cfg, Memory(cfg), tb, client=object())
    return ag.system, ag.tools


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="claude-opus-5-5")
    args = ap.parse_args()
    import anthropic

    client = anthropic.Anthropic()
    system, tools = build(args.model)
    msg = [{"role": "user", "content": "hi"}]

    def count(**kw) -> int:
        return client.messages.count_tokens(model=args.model, messages=msg, **kw).input_tokens

    bare = count()
    with_system = count(system=system)
    full = count(system=system, tools=tools)
    print(f"model {args.model}")
    print(f"  system prompt   {with_system - bare:>7,} tokens")
    print(f"  tools ({len(tools)})      {full - with_system:>7,} tokens")
    print(f"  fixed prefix    {full - bare:>7,} tokens per request")
    per = []
    for t in tools:
        per.append((count(tools=[t]) - bare, t.get("name") or t.get("type")))
    print("  largest tools:")
    for n, name in sorted(per, reverse=True)[:15]:
        print(f"    {name:<22}{n:>6,}")
    price = PRICES.get(args.model)
    if price:
        prefix = full - bare
        print(f"  one request's fixed prefix: uncached ${prefix * price / 1e6:.4f}, "
              f"5-min cache write ${prefix * price * 1.25 / 1e6:.4f}, 1-hour write ${prefix * price * 2 / 1e6:.4f}, "
              f"cache read ${prefix * CACHE_READ[args.model] / 1e6:.5f}")
    return 0


if __name__ == "__main__":
    if not os.environ.get("ANTHROPIC_API_KEY"):
        print("needs ANTHROPIC_API_KEY (the token-counting endpoint is free)")
        sys.exit(2)
    sys.exit(main())
