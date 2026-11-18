#!/usr/bin/env python3
"""LinuxAI image backend: stable-diffusion.cpp (`sd`), a local model on the CPU, or the GPU with a Vulkan/CUDA build.
No graphics card needed: slow but free and private.

  LINUXAI_IMAGE_CPU_CMD=python3 /usr/share/linuxai/image-backends/sdcpp.py
  LINUXAI_SD_MODEL=~/models/sd-turbo.safetensors   (a small/turbo model keeps CPU times reasonable)
  LINUXAI_SD_STEPS=4                                 (optional)
"""
import json, os, shutil, subprocess, sys

job = json.load(sys.stdin)
sd = os.environ.get("LINUXAI_SD_BIN") or shutil.which("sd")
model = os.path.expanduser(os.environ.get("LINUXAI_SD_MODEL", ""))
if not sd or not os.path.exists(model):
    sys.exit("stable-diffusion.cpp (sd) or LINUXAI_SD_MODEL isn't set up")
w, h = (job.get("size") or "512x512").split("x")
cmd = [sd, "-m", model, "-p", job["prompt"], "-W", w, "-H", h, "-o", job["output"],
       "--steps", os.environ.get("LINUXAI_SD_STEPS", "20")]
if job.get("inputs"):
    cmd += ["-i", job["inputs"][0]]  # change the given picture as described
r = subprocess.run(cmd, capture_output=True, text=True)
if r.returncode != 0:
    sys.exit(r.stderr[-500:])
print(json.dumps({"cost_cents": 0, "model": os.path.basename(model)}))
