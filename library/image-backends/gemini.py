#!/usr/bin/env python3
"""LinuxAI image backend: Google's Gemini image model ("Nano Banana"), a paid service. Makes pictures from a
description and changes given pictures as described.

  LINUXAI_IMAGE_PAID_CMD=python3 /usr/share/linuxai/image-backends/gemini.py
  LINUXAI_IMAGE_PAID_CENTS=4             (estimate per picture; check current pricing)
  GEMINI_API_KEY=...                     (stored only on this machine)
  LINUXAI_GEMINI_IMAGE_MODEL=gemini-2.5-flash-image   (or a newer image model)
"""
import base64, json, mimetypes, os, sys, urllib.request

job = json.load(sys.stdin)
key = os.environ.get("GEMINI_API_KEY")
if not key:
    sys.exit("GEMINI_API_KEY isn't set")
model = os.environ.get("LINUXAI_GEMINI_IMAGE_MODEL", "gemini-2.5-flash-image")
parts = [{"text": job["prompt"] + (f" (size {job['size']})" if job.get("size") else "")}]
for p in job.get("inputs") or []:
    parts.append({"inline_data": {"mime_type": mimetypes.guess_type(p)[0] or "image/png",
                                  "data": base64.b64encode(open(p, "rb").read()).decode()}})
req = urllib.request.Request(f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent",
                             json.dumps({"contents": [{"parts": parts}]}).encode(),
                             {"Content-Type": "application/json", "x-goog-api-key": key})
resp = json.load(urllib.request.urlopen(req, timeout=300))
for part in resp.get("candidates", [{}])[0].get("content", {}).get("parts", []):
    data = part.get("inlineData") or part.get("inline_data")
    if data:
        open(job["output"], "wb").write(base64.b64decode(data["data"]))
        print(json.dumps({"cost_cents": int(os.environ.get("LINUXAI_IMAGE_PAID_CENTS", "4")), "model": model}))
        sys.exit(0)
sys.exit("no picture in the reply: " + json.dumps(resp)[:300])
