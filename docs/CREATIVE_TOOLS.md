# Blender, Unity and Unreal

How LinuxAI works with the big creative applications. **Blender is built** ([BLENDER.md](BLENDER.md)); Unity and Unreal are still plans.

## The shape: one "app organ" pattern for all of them

Each of these programs can be driven by code, and each already has community or official bridges for AI
agents (MCP servers). LinuxAI should not just plug the AI into those bridges and let it type into the human's
open project. Treat each application the way the OS treats the browser and pictures:

1. **The AI works on staged copies.** It never edits the human's file in place. It copies the
   `.blend` file or the project scene into a work folder, runs its changes headless, and saves the result
   next to the original (`scene.blend` → `scene.ai-1.blend`). Replacing the original is a plan step with
   `effect: Destructive`, so the human approves it the first time and trust grows from there.
2. **Every result is verified and looked at.** After a change, the organ renders a preview (a frame, a
   viewport shot, or a build log) and returns it with a short report: what changed, what was checked, what
   wasn't. The AI looks at the preview before it says "done". (The ArtCraft apps call this "inspect, stage,
   edit, preview, export, reopen and verify". It's the same idea as our image organ.)
3. **The live window is a page in the Workspace.** When the human has the app open, the compositor organ
   already lets the AI see the window, point at things and magnify them. A live bridge (an add-on inside the
   app) lets the AI read the open scene ("what's selected", "what's this node") and, with the human watching,
   make a change they asked for. Changes through the live bridge go through a plan like everything else.
4. **Outside content stays outside.** Downloaded assets, add-ons, marketplace packages and project files from
   other people are information. Scripts inside them never run because the AI was asked to open the file:
   Blender's "auto-run Python scripts" stays off.
5. **Long jobs are tasks.** Renders, bakes, light builds and game builds run as background tasks with
   progress in the Workspace and a `reach` text when they finish or fail. They respect power policy: no
   overnight render on battery.

Core pieces to add, the same for each app: verbs (`blender`, `unity`, `unreal`; effect Local on staged
copies), an organ process per app that owns the headless runs and the live bridge, a read-only AI tool to
inspect scenes, a preview returned as an image, and playbooks per category (`3d/modeling`, `3d/render`,
`game/build`).

## The rule for every organ that edits files

Adopted from the ArtCraft apps ([RESEARCH_STORYTOLD.md](RESEARCH_STORYTOLD.md)), and already how Blender and the
picture organ work. Any organ that changes the human's files follows it:

1. **Inspect** the file first.
2. **Stage a copy:** the change goes to a new file next to the original (`name.ai-N.ext`). The original is never
   written.
3. **Edit** the copy with the application's own engine.
4. **Preview and verify:** reopen the result, compare it with the original, and look at it. Report what was
   checked and what wasn't.
5. **Replace only with the human's OK**, keeping the original as a backup.

Between organs, pass verified files, never in-memory handles. If a step fails, stop at the last verified file.

## Blender: first, and fully (built: see [BLENDER.md](BLENDER.md))

Blender is the best place to start: free, open source, in Arch's `extra` repository (`pacman -S blender`), and
almost every action is available from Python (`bpy`).

- **Headless:** `blender -b scene.blend --python-expr "…"` (or `--python job.py`) runs a script with no
  window, then saves, renders (`-f 1`, `-a`) or exports (glTF, FBX, OBJ, USD). The organ writes a small,
  checked job script per request rather than passing raw code; a raw-`bpy` escape hatch exists for the AI but
  runs only on a staged copy.
- **Live bridge:** a Blender add-on that opens a Unix socket inside the human's running Blender (the
  community MCP servers use a TCP port on localhost; a socket only the human's user can open is safer). The
  organ asks it for the scene, the selection and the viewport, and sends it operations.
- **What the AI can do for the human:** set up scenes from a description, batch-rename and tidy collections,
  fix scale and normals, set up materials, light and cameras, run renders and compare them, convert and
  export for game engines, explain what a node setup does, and turn pictures from the image organ into
  textures or reference planes.
- **Size:** about 400 MB installed. Ship it in the image, or install on first use? (Decide later.)
- **First test:** a boot check that opens a generated `.blend` headless, adds an object, saves a staged copy,
  renders a 320×240 frame with Cycles on the CPU, and checks the picture isn't blank.

## Unity: second, through Unity's own CLI

- Unity's editor is not open source and isn't in Arch's repositories. It installs with Unity's own tools and
  needs a Unity account and licence, which only the human can do (a human step in the setup task).
- Unity released an official standalone `unity` CLI in July 2026. It manages editors, projects and sign-in,
  and turns a running editor into a local command server that agents can drive, including an MCP mode
  (`unity mcp configure <client>`). It needs Unity 6 or later and the Unity Pipeline package in the project.
  Unity's earlier in-editor MCP server is deprecated in favour of this CLI.
- Plan: the `unity` organ wraps the CLI. Headless work (builds, tests, asset imports, `-batchmode`) runs on a
  copy of the project or on a git branch; the live editor connection is the live bridge. Game projects are
  usually git repositories, so the natural staged copy is a branch plus a worktree, and "replace the
  original" becomes "merge".
- Community MCP servers also exist (CoderGamester's mcp-unity, CoplayDev's MCP for Unity) for older Unity
  versions.

## Unreal: last, optional

- Unreal runs on Linux, but the Linux editor comes from Epic (a prebuilt download or a build from source after
  linking a GitHub account to Epic). It needs a lot of disk space (well over 100 GB with a project), a strong
  GPU and a lot of memory. Most laptops running LinuxAI won't want it.
- Drive it with Unreal's built-in Python remote execution, enabled in Project Settings, plus the Remote Control
  API. The community `unreal-engine-mcp` server uses only those, with no C++ plugin, and lists Linux as
  supported but less tested. UE 5.7 also ships an experimental in-editor AI assistant.
- Plan: same organ shape as Unity (git worktree as the staged copy, builds and cooks as background tasks).
  Only build this when someone actually uses Unreal on LinuxAI.

## Order

1. Blender organ: headless jobs, staged copies, preview, playbooks, then the live bridge. Boot test.
2. A "3D asset" path between organs: image → texture or reference plane, Blender → glTF/FBX for games.
3. Unity organ on the official CLI, with a human setup task for the account and licence.
4. Unreal organ, when wanted.

## To decide later

- Ship Blender in the image, or install it the first time it's needed?
- Should the live bridges ever change the human's open scene while they're working in it, or only after
  they say so each time?
- Unity and Unreal accounts and licences: a setup task the AI walks the human through, or left to the human?
