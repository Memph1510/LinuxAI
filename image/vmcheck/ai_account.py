"""VM checks for the independent review's C1 (issue #22): the AI has its own account, can't answer for the human, and
its commands run in a sandbox. Run as root; each step that is the AI's runs as linuxai-ai.
Usage: python3 ai_account.py account|socket|sandbox|organs|mind|core"""
import json
import os
import re
import subprocess
import sys
import time

AI = "linuxai-ai"
PY = "/opt/linuxai/lib"


def sh(cmd: list, user: str | None = None, timeout: int = 60, env: dict | None = None) -> str:
    if user:
        cmd = ["runuser", "-u", user, "--"] + (["env"] + [f"{k}={v}" for k, v in (env or {}).items()] if env else []) + cmd
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, check=False)
    return (r.stdout + r.stderr).strip()


def ctl(req: dict, user: str | None = None) -> dict:
    out = sh(["linuxai-core", "ctl", json.dumps(req)], user)
    try:
        return json.loads(out)
    except json.JSONDecodeError:
        return {"raw": out}


mode = sys.argv[1]
if mode == "account":
    groups = sh(["id", "-nG", AI]).split()
    sudo = sh(["sudo", "-l", "-U", AI])
    print(f"ai account: exists {bool(groups)}; reaches the core {'linuxai-core' in groups}; "
          f"not the human's {'linuxai' not in groups and 'wheel' not in groups}; no sudo {'not allowed' in sudo}")

elif mode == "socket":
    assess = {"confidence": 0.95, "consequences": ["changes a machine setting"],
              "outcomes": [{"if": "it misbehaves", "then": "set it back"}]}
    plan = {"plan": "c1-check", "assess": assess, "steps": [{"do": "set", "what": "power.profile", "to": "power-saver"}]}
    r = ctl({"op": "submit", "plan": plan, "source": "ai"}, AI)
    pid = (r.get("result") or {}).get("plan_id", "")
    a = ctl({"op": "answer", "plan_id": pid, "text": "yes, always", "source": "human"}, AI)
    st = (ctl({"op": "plan", "plan_id": pid}).get("plan") or {}).get("status")
    standing = [m for m in ctl({"op": "mandates"}).get("mandates", []) if "power" in json.dumps(m)]
    again = (ctl({"op": "submit", "plan": plan, "source": "human"}, AI).get("result") or {}).get("status")
    ctl({"op": "answer", "plan_id": pid, "text": "no", "source": "human"})  # the human tidies up
    print(f"the AI answering as the human: asked first {bool(pid)}; still waiting {st == 'waiting:answer'}; "
          f"no standing permission {not standing}; claiming to be the human still asks {again == 'waiting:answer'}; ({a.get('error', '')})")

elif mode == "sandbox":
    probe = ("import subprocess; from linuxai.tools import sandbox_argv, sandbox_hidden, sandbox_ready;"
             "print('ready', sandbox_ready()[0]);"
             "box = sandbox_argv('/', sandbox_hidden());"
             "print(subprocess.run(box + ['sh', '-c', 'echo sockets $(ls -A /run/linuxai /run/linuxai-organs 2>/dev/null | grep -c sock);"
             " (timeout 5 bash -c \"echo > /dev/tcp/1.1.1.1/53\" 2>/dev/null && echo connect yes) || echo connect no;"
             " (timeout 5 getent hosts archlinux.org >/dev/null && echo lookup yes) || echo lookup no;"
             " grep NoNewPrivs /proc/self/status; echo readonly $(touch /etc/x 2>&1 | grep -c Read-only);"
             " echo services $(systemctl is-active linuxai-core)'], capture_output=True, text=True, check=False).stdout)")
    out = sh(["python3", "-c", probe], AI, env={"PYTHONPATH": PY, "HOME": "/tmp"})
    nnp = re.search(r"NoNewPrivs:\s+1", out) is not None
    print(f"sandbox: works {'ready True' in out}; core and organ sockets hidden {'sockets 0' in out}; "
          f"no network {'connect no' in out}; no name lookups {'lookup no' in out}; no new privileges {nnp}; "
          f"read-only {'readonly 1' in out}; still sees services {'services active' in out}")

elif mode == "organs":
    st = os.stat("/run/linuxai-organs")
    socks = sorted(os.listdir("/run/linuxai-organs"))
    probe = ("import json, socket; s = socket.socket(socket.AF_UNIX); s.connect('/run/linuxai-organs/linuxai-browser.sock');"
             "s.sendall(b'{\"op\": \"tabs\"}\\n'); print(json.loads(s.makefile().readline()).get('ok'))")
    reach = sh(["python3", "-c", probe], AI) if "linuxai-browser.sock" in socks else "no browser socket"
    print(f"organs folder: setgid {bool(st.st_mode & 0o2000)}; sockets {','.join(socks)}; the AI's account reaches the browser {reach == 'True'}")

elif mode == "mind":
    key = "/home/linuxai/.config/linuxai/env"
    made = not os.path.exists(key)
    if made:  # a stand-in key: the mind only starts once a key exists; nothing here calls the API
        os.makedirs(os.path.dirname(key), exist_ok=True)
        with open(key, "w") as f:
            f.write("ANTHROPIC_API_KEY=sk-ant-vmcheck-not-real\n")
        os.chmod(key, 0o600)
        sh(["chown", "-R", "linuxai:linuxai", "/home/linuxai/.config"])
    sh(["systemctl", "start", "linuxai-mind.service"])
    pid, user = "0", ""
    for _ in range(60):
        pid = sh(["systemctl", "show", "-p", "MainPID", "--value", "linuxai-mind.service"])
        if pid not in ("", "0") and "connected to the core" in sh(["journalctl", "-u", "linuxai-mind", "-b", "--no-pager"]):
            user = sh(["ps", "-o", "user=", "-p", pid])
            break
        time.sleep(2)
    readable = sh(["sh", "-c", f"test -r {key} && echo yes || echo no"], AI)
    sh(["systemctl", "stop", "linuxai-mind.service"])
    if made:
        os.remove(key)
    print(f"the mind: runs as {user or 'nothing'}; connected to the core {bool(user)}; its account can read the key file {readable}")

elif mode == "core":
    # Review M4: the core and the mail organ under their own accounts; what needs root goes through systemd.
    core_user = sh(["ps", "-o", "user=", "-p", sh(["systemctl", "show", "-p", "MainPID", "--value", "linuxai-core"])])
    mail_user = sh(["systemctl", "show", "-p", "User", "--value", "linuxai-mail"])
    plan = {"plan": "m4-check", "steps": [{"do": "run", "program": "id", "args": ["-u"], "as": "r"}, {"tell": "uid {$r.output}"}]}
    r = ctl({"op": "submit", "plan": plan, "source": "human"})
    told = [e["data"].get("text", "") for e in ctl({"op": "events", "since": 0, "limit": 2000}).get("events", []) if e["kind"] == "ai.tell"]
    root_action = any(t == "uid 0" for t in told)
    print(f"core runs as {core_user}; mail runs as {mail_user}; an approved action still runs as root {root_action} "
          f"({(r.get('result') or {}).get('status')}; {(r.get('result') or {}).get('error', '')})")
