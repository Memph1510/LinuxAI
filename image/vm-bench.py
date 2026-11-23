#!/usr/bin/env python3
"""Run the thinking benchmark with the real model inside the LinuxAI image (handoff LX-API-4b).

  ANTHROPIC_API_KEY=... python3 image/vm-bench.py out/linuxai-*.iso [--strategies ladder] [--out DIR]

Boots the ISO in QEMU (as image/test-boot.py does) and runs tests/thinking_bench.py --real in the booted OS, so the
AI's own checks (systemctl, journal) see a real systemd machine. The workload's failing services exist there: cups and
nginx are created as units that really fail. The benchmark's core runs in rehearsal mode, so the AI's restarts are
recorded, not performed. The key reaches the VM over the serial console, read by the shell without display; it is
never written to a file or a log (leave SERIAL_LOG unset). It costs real API money (the ladder alone: about $2).
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
spec = importlib.util.spec_from_file_location("test_boot", HERE / "test-boot.py")
tb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tb)

FAILING_UNITS = ("cups", "nginx")


def status(out: str) -> str:
    """The HTTP status a command printed on a line of its own (the shell can add lines after it)."""
    codes = re.findall(r"^\s*(\d{3})\s*$", out, re.M)
    return codes[-1] if codes else out.strip()[-60:]


def status_len(out: str) -> int:
    """The number a command printed on the last line of its own."""
    nums = re.findall(r"^\s*(\d+)\s*$", out, re.M)
    return int(nums[-1]) if nums else -1


def fetch(con, path: str) -> str:
    """A file out of the VM: one base64 line, then a newline, so the end marker always follows on its own line."""
    import base64
    out = con.run(f"base64 -w0 {path}; echo", 300)
    b64 = max(re.findall(r"[A-Za-z0-9+/=]{4,}", out), key=len, default="")
    return base64.b64decode(b64).decode()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("iso")
    ap.add_argument("--strategies", default="ladder")
    ap.add_argument("--out", default=str(Path(tempfile.gettempdir()) / "linuxai-vm-bench"))
    ap.add_argument("--boot-timeout", type=float, default=1500)
    ap.add_argument("--check-only", action="store_true",
                    help="boot, check the API and the key's path with whatever key is given, then stop: no model calls")
    ap.add_argument("--ca", help="extra CA certificates (PEM) the VM should trust, for a network that re-terminates TLS")
    args = ap.parse_args()
    key = os.environ.get("ANTHROPIC_API_KEY", "")
    if not key:
        print("needs ANTHROPIC_API_KEY (it is passed to the VM over the serial console, never written down)")
        return 2
    if os.environ.get("SERIAL_LOG"):
        print("unset SERIAL_LOG: the console carries the key")
        return 2
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp())
    ser, mon = str(tmp / "serial.sock"), str(tmp / "mon.sock")
    accel = "kvm" if os.path.exists("/dev/kvm") else "tcg,thread=multi"
    qemu = subprocess.Popen([
        "qemu-system-x86_64", "-accel", accel, "-cpu", "SandyBridge", "-m", "4096", "-smp", str(min(os.cpu_count() or 2, 4)),
        "-cdrom", args.iso, "-boot", "d", "-display", "none", "-vga", "std",
        "-serial", f"unix:{ser},server,nowait", "-monitor", f"unix:{mon},server,nowait",
        "-nic", "user,model=virtio-net-pci"], stderr=subprocess.DEVNULL)
    try:
        con = tb.Serial(ser)
        t0 = time.time()
        con.read_until(r"login:", args.boot_timeout)
        print(f"booted in {time.time() - t0:.0f}s", flush=True)
        con.login()
        con.run("export TERM=dumb PS1='# '; unset REPORTTIME; stty -echo cols 200", 30)
        con.run("timeout 240 systemctl is-system-running --wait", 300)
        # The workload's failing services, for real: units that fail when started.
        for unit in FAILING_UNITS:
            con.run(f"printf '[Unit]\\nDescription=benchmark stand-in for {unit}\\n[Service]\\nExecStart=/usr/bin/false\\n'"
                    f" > /etc/systemd/system/{unit}.service", 30)
        con.run("systemctl daemon-reload; " + "; ".join(f"systemctl start {u}" for u in FAILING_UNITS) + "; true", 60)
        state = con.run("systemctl is-failed " + " ".join(FAILING_UNITS), 30)
        print(f"failing units: {' '.join(state.split())}", flush=True)
        con.run("mkdir -p /tmp/bench/tests", 30)
        tb.copy_in(con, ROOT / "tests" / "thinking_bench.py", "/tmp/bench/tests/thinking_bench.py")
        # Use the address this host uses for the API (some hosts pin it in /etc/hosts and refuse others).
        api_ip = socket.gethostbyname("api.anthropic.com")
        con.run(f"echo '{api_ip} api.anthropic.com' >> /etc/hosts", 30)
        if args.ca:
            # Trusted by this shell only: the VM's own store is combined with the extra CAs in a copy.
            tb.copy_in(con, Path(args.ca), "/tmp/bench/extra-ca.pem")
            con.run("cat /etc/ssl/certs/ca-certificates.crt /tmp/bench/extra-ca.pem > /tmp/bench/ca.pem;"
                    " export SSL_CERT_FILE=/tmp/bench/ca.pem CURL_CA_BUNDLE=/tmp/bench/ca.pem", 30)
        # The AI layer the image ships (and its packages), as the image's own cortex launcher sets it.
        con.run("export PYTHONPATH=/opt/linuxai/lib", 30)
        reach = con.run("curl -s -o /dev/null -w '%{http_code}\\n' https://api.anthropic.com/v1/models", 60)
        code = status(reach)
        # The benchmark's own client, with a dummy key: a 401 means it reached the API and trusted its certificate.
        probe = ("import anthropic\ntry:\n    anthropic.Anthropic(api_key=\"probe\", max_retries=0).models.list()\n    print(200)\n"
                 "except anthropic.APIStatusError as e:\n    print(e.status_code)\nexcept Exception as e:\n    print(type(e).__name__, e, getattr(e, \"__cause__\", None))\n")
        (tmp / "probe.py").write_text(probe)
        tb.copy_in(con, tmp / "probe.py", "/tmp/bench/probe.py")
        py = con.run("python3 /tmp/bench/probe.py 2>&1 | tail -8", 600)  # the first import compiles the SDK (slow without KVM)
        py_code = status(py)
        print(f"API reachable from the VM: curl HTTP {code}, python HTTP {py_code}", flush=True)
        if code not in ("200", "401") or py_code not in ("200", "401"):
            print(f"the VM can't reach the API; stopping before any spend. The client said:\n{py.strip()}")
            return 1
        # The key: zsh's line editor redraws a typed line even with echo off, so it is turned off first, and history
        # with it. The key lives only in this shell's environment; anything saved from the console is scrubbed of it.
        con.run("unset HISTFILE; setopt HIST_IGNORE_SPACE; unsetopt zle", 30)
        con.send(f" export ANTHROPIC_API_KEY='{key}'\n")
        seen = con.run("echo ${#ANTHROPIC_API_KEY}", 60)  # everything the console printed since, up to a marker
        shown = key in seen or key in con.buf
        print(f"key set in the VM shell: {'yes' if status_len(seen) == len(key) else 'NO'}", flush=True)
        print(f"key shown on the console: {'YES' if shown else 'no'}", flush=True)
        if shown:
            return 1
        if args.check_only:  # the console path without spending: a stand-in file comes back intact
            con.run("printf '[{\"kind\": \"ai.tell\", \"text\": \"check\"}]' > /tmp/bench/told.json", 30)
            print(f"told.json comes back: {fetch(con, '/tmp/bench/told.json')!r}", flush=True)
            con.run("unset ANTHROPIC_API_KEY", 30)
            return 0
        report = con.run(f"cd /tmp/bench && LINUXAI_CORE_BIN=/usr/local/bin/linuxai-core python3 tests/thinking_bench.py --real"
                         f" --strategies {args.strategies} 2>&1 | tail -40", 7200)
        report = report.replace(key, "<key>")
        (out / "report.txt").write_text(report)
        print(report, flush=True)
        told = re.search(r"(/tmp/linuxai-bench-\w+-\w+/told\.json)", report)
        if told:
            (out / "told.json").write_text(fetch(con, told.group(1)).replace(key, "<key>"))
            print(f"messages to the human: {out / 'told.json'}", flush=True)
        con.run("unset ANTHROPIC_API_KEY", 30)
    finally:
        qemu.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
