#!/usr/bin/env python3
"""LinuxAI image backend: a local model on the graphics card through ComfyUI (FLUX, Qwen-Image-Edit, SDXL...).

Set in ~/.config/linuxai/env:
  LINUXAI_IMAGE_LOCAL_CMD=python3 /usr/share/linuxai/image-backends/comfyui.py
  LINUXAI_COMFYUI_URL=http://127.0.0.1:8188
  LINUXAI_COMFYUI_WORKFLOW=~/image-workflows/flux.json     (exported from ComfyUI with "Save (API Format)")
  LINUXAI_COMFYUI_EDIT_WORKFLOW=~/image-workflows/qwen-edit.json   (optional, for changing a picture)

In the workflow, put {{prompt}}, {{width}}, {{height}}, {{seed}} and (edit workflows) {{input}} where the values go.
Reads the job as JSON on stdin; writes the picture to job["output"]. Standard library only.
"""
import json, os, random, sys, time, urllib.parse, urllib.request, uuid

job = json.load(sys.stdin)
base = os.environ.get("LINUXAI_COMFYUI_URL", "http://127.0.0.1:8188").rstrip("/")
editing = bool(job.get("inputs"))
wf_path = os.path.expanduser(os.environ.get("LINUXAI_COMFYUI_EDIT_WORKFLOW" if editing else "LINUXAI_COMFYUI_WORKFLOW", ""))
if not wf_path or not os.path.exists(wf_path):
    sys.exit(f"no ComfyUI {'edit ' if editing else ''}workflow set (LINUXAI_COMFYUI_{'EDIT_' if editing else ''}WORKFLOW)")


def upload(path: str) -> str:
    boundary = uuid.uuid4().hex
    name = os.path.basename(path)
    body = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{name}\"\r\n"
            f"Content-Type: application/octet-stream\r\n\r\n").encode() + open(path, "rb").read() + f"\r\n--{boundary}--\r\n".encode()
    req = urllib.request.Request(f"{base}/upload/image", body, {"Content-Type": f"multipart/form-data; boundary={boundary}"})
    return json.load(urllib.request.urlopen(req, timeout=60))["name"]


w, h = (job.get("size") or "1024x1024").split("x")
values = {"prompt": job["prompt"], "width": int(w), "height": int(h), "seed": random.randrange(2**31)}
if editing:
    values["input"] = upload(job["inputs"][0])


def fill(v):
    if isinstance(v, dict):
        return {k: fill(x) for k, x in v.items()}
    if isinstance(v, list):
        return [fill(x) for x in v]
    if isinstance(v, str) and v.startswith("{{") and v.endswith("}}") and v[2:-2].strip() in values:
        return values[v[2:-2].strip()]  # whole-value slot keeps its type (numbers stay numbers)
    if isinstance(v, str):
        for k, x in values.items():
            v = v.replace("{{" + k + "}}", str(x))
    return v


workflow = fill(json.load(open(wf_path)))
req = urllib.request.Request(f"{base}/prompt", json.dumps({"prompt": workflow, "client_id": uuid.uuid4().hex}).encode(),
                             {"Content-Type": "application/json"})
pid = json.load(urllib.request.urlopen(req, timeout=60))["prompt_id"]
deadline = time.time() + float(os.environ.get("LINUXAI_COMFYUI_TIMEOUT", "1200"))
while time.time() < deadline:
    hist = json.load(urllib.request.urlopen(f"{base}/history/{pid}", timeout=30)).get(pid)
    if hist:
        for out in hist.get("outputs", {}).values():
            for img in out.get("images", []):
                q = urllib.parse.urlencode({"filename": img["filename"], "subfolder": img.get("subfolder", ""), "type": img.get("type", "output")})
                open(job["output"], "wb").write(urllib.request.urlopen(f"{base}/view?{q}", timeout=60).read())
                print(json.dumps({"cost_cents": 0, "model": os.path.basename(wf_path)}))
                sys.exit(0)
        if hist.get("status", {}).get("status_str") == "error":
            sys.exit("ComfyUI reported an error running the workflow")
    time.sleep(1)
sys.exit("ComfyUI took too long")
