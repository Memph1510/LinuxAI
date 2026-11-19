# Pictures

The OS makes and edits pictures itself, on whatever machine it's on. Nothing is off the table: local tools,
local models, paid services. It picks the best way for each job and tells you what it used and what it cost.

## How a job is done

| Job | How | Where |
|---|---|---|
| Precise edits: crop, resize, rotate, flip, brightness/contrast, grayscale, sharpen, blur, text on the picture, format, strip metadata | ImageMagick | Always local: exact, instant, free |
| Make a picture from a description, or change one as described ("remove the background", "make it dusk") | Best backend available, in order: | |
| | 1. a local model on the graphics card (FLUX, Qwen-Image-Edit, SDXL via ComfyUI) | free and private; needs a capable card |
| | 2. a paid service (e.g. Google's Gemini image model, "Nano Banana") | fast and good; within your monthly budget |
| | 3. a local model on the CPU (stable-diffusion.cpp) | free and private; slow |
| 3D, vector art | Blender, Inkscape (installed when needed) | local |

If one backend fails it tries the next, and says what it tried. Past the monthly budget it **asks you** before
spending (in the session right away; from behind the scenes, by text). If a free backend is available, an
over-budget job goes there instead of asking.

`linuxai-core image caps` reports what this machine can do: graphics cards and their memory, RAM, tools, which
backends are set up, and plain advice ("this card has 12 GB: it can run FLUX locally"). This is how it stays
laptop-independent: the same OS on a machine without a capable card uses the paid or CPU route; on one with a
card, the local route.

## Every picture is remembered

Each result is recorded (type `image`): the file, what made it, from what picture and with which edits or
description, the backend and model, and the cost. The photo view (with the Workspace screen) shows them and
takes your comments to refine a result. The AI looks at each result before calling a job done.

## Setting up backends (`~/.config/linuxai/env`)

Each backend is a command, so any tool or service can be plugged in. Ready-made adapters are in
`library/image-backends/` (on the OS: `/usr/share/linuxai/image-backends/`):

```
# Paid: Google's Gemini image model
LINUXAI_IMAGE_PAID_CMD=python3 /usr/share/linuxai/image-backends/gemini.py
GEMINI_API_KEY=...
LINUXAI_IMAGE_PAID_CENTS=4
LINUXAI_IMAGE_BUDGET_CENTS=1000          # $10 a month

# Local graphics card: ComfyUI with a workflow saved in API format, with {{prompt}} {{width}} {{height}} {{seed}} {{input}}
LINUXAI_IMAGE_LOCAL_CMD=python3 /usr/share/linuxai/image-backends/comfyui.py
LINUXAI_COMFYUI_URL=http://127.0.0.1:8188
LINUXAI_COMFYUI_WORKFLOW=~/image-workflows/flux.json

# CPU: stable-diffusion.cpp
LINUXAI_IMAGE_CPU_CMD=python3 /usr/share/linuxai/image-backends/sdcpp.py
LINUXAI_SD_MODEL=~/models/sd-turbo.safetensors
```

The contract for any other backend: it gets `{"prompt", "size", "inputs": [paths], "output": path}` as JSON on
stdin, writes the picture to `output`, and may print `{"cost_cents": n, "model": "..."}`. Exit 0 means done.

## Status

- Done and tested: capability detection, precise edits (real ImageMagick), routing with fallback, budget with
  asking, records, the AI's `image` tool (it sees the result). The ComfyUI adapter is tested against a stand-in
  server.
- Not yet tried against the real services: Gemini (needs a key), ComfyUI and stable-diffusion.cpp (need models,
  and a graphics card for ComfyUI). That's for the laptop.
- Next: the photo view with comments; installing ComfyUI and a model automatically when the card can run one.
