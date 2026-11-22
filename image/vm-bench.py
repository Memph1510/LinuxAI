#!/usr/bin/env python3
"""Run the thinking benchmark with the real model inside the LinuxAI image (handoff LX-API-4b).

  ANTHROPIC_API_KEY=... python3 image/vm-bench.py out/linuxai-*.iso [--strategies ladder] [--out DIR]

Boots the ISO in QEMU (as image/test-boot.py does) and runs tests/thinking_bench.py --real in the booted OS, so the
AI's own checks (systemctl, journal) see a real systemd machine. The workload's failing services exist there: cups and
nginx are created as units that really fail. The benchmark's core runs in rehearsal mode, so the AI's restarts are
recorded, not performed. The key reaches the VM over the serial console with echo off and is never written to a file
or a log (leave SERIAL_LOG unset). It costs real API money (the ladder alone: about $2).
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import re
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


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("iso")
    ap.add_argument("--strategies", default="ladder")
    ap.add_argument("--out", default=str(Path(tempfile.gettempdir()) / "linuxai-vm-bench"))
    ap.add_argument("--boot-timeout", type=float, default=1500)
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
        con.run("export TERM=dumb PS1='# '; stty -echo cols 200", 30)
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
        reach = con.run("curl -s -o /dev/null -w '%{http_code}' https://api.anthropic.com/v1/models", 60)
        print(f"API reachable from the VM: HTTP {reach.strip()[-3:]}", flush=True)
        # The key: typed with echo off, kept only in this shell's environment.
        con.send(f"export ANTHROPIC_API_KEY='{key}'\n")
        time.sleep(1)
        report = con.run(f"cd /tmp/bench && LINUXAI_CORE_BIN=/usr/local/bin/linuxai-core python3 tests/thinking_bench.py --real"
                         f" --strategies {args.strategies} 2>&1 | tail -40", 7200)
        if key in report:
            report = report.replace(key, "<key>")
        (out / "report.txt").write_text(report)
        print(report, flush=True)
        told = re.search(r"(/tmp/linuxai-bench-ladder-\w+/told\.json)", report)
        if told:
            told_text = con.run(f"cat {told.group(1)}", 120)
            (out / "told.json").write_text(told_text.replace(key, "<key>"))
            print(f"messages to the human: {out / 'told.json'}", flush=True)
        con.run("unset ANTHROPIC_API_KEY", 30)
    finally:
        qemu.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
