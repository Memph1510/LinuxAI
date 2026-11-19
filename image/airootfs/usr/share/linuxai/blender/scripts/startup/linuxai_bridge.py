"""LinuxAI's live bridge in the human's open Blender.

Loaded on start from BLENDER_SYSTEM_SCRIPTS (set by the LinuxAI session). It opens a Unix socket only this user
can reach, $XDG_RUNTIME_DIR/linuxai-blender-<pid>.sock, and answers one JSON request per line, on Blender's main
thread (a timer), so it never touches the scene mid-draw:

  {"op": "view"}                 what's open: the file, unsaved changes, mode, the active and selected objects
  {"op": "inspect"}              the whole scene, as the headless engine reports it
  {"op": "apply", "ops": [...]}  change the open scene (the engine's ops); one undo step, so Ctrl+Z takes it back
  {"op": "render", ...}          a preview of the open scene, written to a file

The Rust blender organ decides when a change may be made here: in the human's own scene, only when they asked
for it. Nothing here runs when Blender runs headless (jobs use the engine directly).
"""
from __future__ import annotations

import json
import os
import socket
import sys

import bpy

ENGINE_DIR = os.environ.get("LINUXAI_BLENDER_ENGINE_DIR", "/usr/share/linuxai/blender")
POLL = 0.1
_server = None


def _engine():
    if ENGINE_DIR not in sys.path:
        sys.path.insert(0, ENGINE_DIR)
    import linuxai_blender
    return linuxai_blender


def view() -> dict:
    ctx = bpy.context
    active = ctx.view_layer.objects.active
    return {"file": bpy.data.filepath or None, "unsaved_changes": bpy.data.is_dirty, "mode": ctx.mode,
            "scene": ctx.scene.name, "frame": ctx.scene.frame_current,
            "active": active.name if active else None,
            "selected": [o.name for o in ctx.view_layer.objects if o.select_get()][:100],
            "blender": bpy.app.version_string}


def handle(req: dict) -> dict:
    eng = _engine()
    op = req.get("op")
    try:
        if op == "view":
            return {"ok": True, **view()}
        if op == "inspect":
            return {"ok": True, "scene": eng.inspect(), "view": view()}
        if op == "apply":
            done = eng.run_ops(req.get("ops") or [])
            try:
                bpy.ops.ed.undo_push(message=f"LinuxAI: {req.get('why') or 'changes'}")
            except RuntimeError:
                pass
            return {"ok": True, "done": done, "view": view()}
        if op == "render":
            return {"ok": True, **eng.render(req["output"], req.get("frame"), req.get("width", 640),
                                             req.get("height", 360), req.get("samples", 16), req.get("engine", "CYCLES"))}
        return {"ok": False, "error": f"unknown op {op!r}"}
    except eng.OpError as e:
        return {"ok": False, "error": str(e), "view": view()}
    except Exception as e:  # noqa: BLE001 - report, never take Blender down
        return {"ok": False, "error": f"{type(e).__name__}: {e}"}


class Server:
    def __init__(self, path: str):
        self.path = path
        if os.path.exists(path):
            os.unlink(path)
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        old = os.umask(0o177)
        try:
            self.sock.bind(path)
        finally:
            os.umask(old)
        self.sock.listen(4)
        self.sock.setblocking(False)

    def poll(self):
        """Answer whoever is waiting; return how long until the next look (Blender timer contract)."""
        while True:
            try:
                conn, _ = self.sock.accept()
            except (BlockingIOError, OSError):
                return POLL
            with conn:
                conn.settimeout(5)
                try:
                    line = conn.makefile("r").readline()
                    reply = handle(json.loads(line)) if line.strip() else {"ok": False, "error": "empty request"}
                except (ValueError, OSError) as e:
                    reply = {"ok": False, "error": f"bad request: {e}"}
                try:
                    conn.sendall((json.dumps(reply, default=str) + "\n").encode())
                except OSError:
                    pass

    def close(self):
        self.sock.close()
        if os.path.exists(self.path):
            os.unlink(self.path)


def socket_path() -> str:
    run = os.environ.get("XDG_RUNTIME_DIR") or f"/tmp/linuxai-{os.getuid()}"
    os.makedirs(run, mode=0o700, exist_ok=True)
    return os.path.join(run, f"linuxai-blender-{os.getpid()}.sock")


def _tick():
    return _server.poll() if _server else None


def register():
    global _server
    if bpy.app.background or os.environ.get("LINUXAI_BLENDER_BRIDGE") == "0":
        return
    try:
        _server = Server(socket_path())
    except OSError as e:
        print(f"LinuxAI bridge: not started ({e})")
        return
    bpy.app.timers.register(_tick, first_interval=POLL, persistent=True)


def unregister():
    global _server
    if _server:
        if bpy.app.timers.is_registered(_tick):
            bpy.app.timers.unregister(_tick)
        _server.close()
        _server = None
