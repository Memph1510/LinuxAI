# Blender

The AI does 3D work in Blender as part of the OS. It builds scenes from a description, fixes and tidies
files, sets up materials, lights and cameras, renders, and exports for games and printing. It works with you in
the Blender you have open, and it treats your files the way a careful assistant would.

Blender 5.3 from Arch's `extra` repository. The organ is `core/src/blender.rs`. The work inside Blender is
`/usr/share/linuxai/blender/linuxai_blender.py`, written in Blender's own Python, which is the only way to drive
Blender.

## How your work stays safe

- **Staged copies.** The AI never saves over a file. A change to `kitchen.blend` goes to `kitchen.ai-1.blend`
  next to it, so texture paths and linked files keep working. The original is opened read-only, in memory.
- **Replacing the original asks you.** `replace` puts a staged copy in place of its original only after you
  say yes, and keeps the original as `kitchen.before-ai-<time>.blend`.
- **Checked results.** After each change the organ:
  1. reopens the new file;
  2. compares it with the original: what was added, removed or changed, down to locations, materials and
     modifiers;
  3. renders a small Cycles preview, which the AI looks at;
  4. checks the preview isn't blank or black.

  It reports what it checked and what it didn't, for example "other camera angles" or "the animation, only one
  frame was rendered".
- **Files from others can't run code.** Every job runs with auto-run scripts off and without your add-ons. Scripts
  inside a file are listed for the AI, never run. There's a test for this.
- **Python the AI writes** (the `python` op, for whatever the built-in ops can't do) can do anything you can, so
  you see the code and say yes first. When you're not there, it doesn't happen.

## Your open Blender (the live bridge)

When you start Blender in LinuxAI, it loads a small bridge from `BLENDER_SYSTEM_SCRIPTS`, which the session sets.
The bridge listens on a socket only your user can open: `$XDG_RUNTIME_DIR/linuxai-blender-<pid>.sock`. It
answers on Blender's main thread, so it never touches the scene mid-draw.

- `live_view` gives the open file, unsaved changes, the mode, and the active and selected objects. When you
  say "this", the AI looks here.
- `live_inspect` gives the whole scene. `live_render` gives a preview of it.
- `live_apply` changes your open scene, only when you asked. It confirms with you first, and the change is one
  undo step, so Ctrl+Z takes it back.

## What the AI can do

These are `ops` in `new`, `apply` and `live_apply`:

| Area | Ops |
|---|---|
| Objects | `add` (cube, plane, sphere, ico_sphere, cylinder, cone, torus, monkey, circle, grid, empty, text), `delete`, `transform` (absolute or relative, or exact dimensions), `rename`, `parent`, `collection` |
| Looks | `material` (colour, metallic, roughness, alpha, emission, transmission), `world`, `shade` smooth/flat |
| Light and camera | `light` (point, sun, spot, area; aimed with `look_at`), `camera` (`look_at`, lens) |
| Geometry | `modifier` (bevel, subdivision, mirror, array, solidify, decimate, remesh, boolean, weld, …), `apply_transforms`, `recalc_normals` |
| Animation | `keyframe`, `frame` |
| Bringing things in | `import` (glTF/GLB, FBX, OBJ, STL, PLY, USD); `image_plane` (a picture, for example from the image organ, as a textured plane) |
| Anything else | `python` (with your OK) |

Other jobs:

- `inspect`: everything about a file, including objects, materials, missing textures and unapplied scale.
- `render`: Cycles, EEVEE or Workbench; on the CPU unless `LINUXAI_BLENDER_GPU=1`.
- `export`: GLB, glTF, FBX, OBJ, STL, PLY or USD.

New scenes, renders, previews and exports go to `~/blender/`. Each one is recorded in the core's memory
(type `creation`).

## Settings

| Variable | Default |
|---|---|
| `LINUXAI_BLENDER` | `blender` on the PATH |
| `LINUXAI_BLENDER_ENGINE` | `/usr/share/linuxai/blender/linuxai_blender.py` |
| `LINUXAI_BLENDER_DIR` | `~/blender` |
| `LINUXAI_BLENDER_GPU` | unset: render on the CPU |
| `LINUXAI_BLENDER_BRIDGE` | `0` turns the live bridge off |

## Size

Blender and what it needs add about 2.3 GB installed: Blender itself, Intel's GPU runtime, the denoiser,
OpenVDB, USD and MaterialX. It's in the image for now. Installing it the first time it's needed is the
alternative (see the checklist).

## Not yet

- Long renders and animations as background tasks with progress in the Workspace (a render now waits up to an
  hour).
- Plan verbs, so the mind can include Blender steps in a plan with the core's approval, rather than only in the
  session.
- Texturing and UV unwrapping ops, geometry nodes, and sculpting help.
- Unity and Unreal (see [CREATIVE_TOOLS.md](CREATIVE_TOOLS.md)).
