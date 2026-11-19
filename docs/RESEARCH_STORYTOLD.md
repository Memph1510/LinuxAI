# Mining the Storytold / ArtCraft repositories

The [storytold](https://github.com/storytold) GitHub organization (ArtCraft) has about 68 public repositories.
Roughly 30 are current: the ArtCraft desktop app and the **Craft App Suite**, a family of clean-room desktop
apps written from scratch in Rust, MIT or Apache-2.0. The rest are older experiments (Unreal plugins,
motion capture, voice models, a VTubing show) and test corpora.

Surveyed November 2026. Star counts change daily.

## What's there

| Group | Repositories | What they are |
|---|---|---|
| The flagship | `artcraft`, `artcraft-services`, `artcraftx` | An "IDE for artists": compose in 2D, stage scenes in 3D (props, depth, posed mannequins, cameras), then generate images and video from about 60 hosted models |
| Craft apps | `photocraft`, `lightcraft`, `vectorcraft`, `filmcraft`, `effectcraft`, `anicraft`, `soundcraft`, `pdfcraft`, `wordcraft`, `gridcraft`, `deckcraft`, `designcraft`, `figcraft`, `cadcraft`, `texturecraft`, `materialcraft` | One app per job: photo editor, RAW developer, vector, video, motion graphics, 2D animation, audio workstation, PDF, word processor, spreadsheet, slides, page layout, interface design, CAD, texture painting, materials |
| Shared | `craft-libs`, `craft-ai-integrations`, `craft-extensions`, `craft-fonts`, `craft-skills`, `craft-launcher`, `craft-wiki` | Libraries, the AI layer, plug-in store, fonts, agent skills, launcher, docs |
| Agents | `craft-anything` | Studies any app, site or game, then has a coding agent build its own version and checks it against the original |
| Tooling | `craft-pr-review-tool`, `craft-repo-status-app`, `craft-common-actions`, corpora | Their own development tools and test data |

Most Craft apps ship a headless CLI that is also an MCP server: for example `photocraft-cli run | batch | mcp`,
with "every menu item, tool and dialog" backed by a registry of 500+ commands. Linux builds come as AppImage,
Flatpak, .deb, .rpm and tarballs; an unofficial Arch package exists (`zamkara/storytold`). A third-party
plugin (`devsolux/storytold`) wires 15 of the apps into Claude Code and Codex with a single skill.

## Ideas worth taking

Ranked by how well they fit LinuxAI.

1. **Staged copy, then verified export.** The AI never edits the original. Each step is
   *inspect → stage a copy → edit → preview → save or export → reopen and verify*. Between apps, only verified
   files are passed on, never in-memory handles. If a step fails, work stops at the last verified file. This
   fits our journaled plans and is already the plan for Blender, Unity and Unreal (see
   [CREATIVE_TOOLS.md](CREATIVE_TOOLS.md)). Worth making a general rule for every organ that edits files.
2. **One engine, three surfaces.** Each capability is a GUI action, a CLI command and an MCP tool at once, from
   the same command registry. Our organs already have the socket plus CLI. Adding an MCP surface from the same
   verb registry would let outside agents use LinuxAI, under the same checks.
3. **Evidence records.** `craft-anything` returns findings with confidence, coverage and limitations. Our
   plan `assess` block covers confidence and unknowns before acting. Reporting *after* acting the same way
   ("checked: renders; not checked: animation") would make results honest and easy to read in the Workspace.
4. **Summary first, then pages.** Big results come back as a summary plus a paging tool, so the model's
   context stays small. Our tools clip results at a fixed size instead. Worth adopting for mail, events,
   search and web pages.
5. **Tool → model binding ("ToolDesk").** The human sets once which model does each tool ("remove
   background: local BEN2", "inpaint: FLUX"), and every app that offers the tool uses that choice. Our image
   organ picks a backend per request; one OS-wide table of choices, editable in the Workspace, fits better.
6. **Local models in pure Rust on the CPU.** `craft-ai-integrations` runs SAM 2.1 (selection) and BEN2 +
   ViTMatte (background removal) on `rten`, a pure-Rust CPU engine, with model files described by size,
   SHA-256 hash, source and licence. That gives us offline background removal and "select this object" in the
   image organ without Python or a GPU, plus a clean way to list and verify model downloads.
7. **Pinned, verified installs.** Downloads are checked by SHA-256 and signature (Sigstore on Linux), staged,
   probed, then promoted, and a failed step leaves the old install intact. That's how LinuxAI should install
   any outside tool or model the AI asks for.
8. **Permission tiers stated up front.** Apps declare whether they can touch only a workspace folder and its
   `outputs/`, or anything the user can, and the broad ones need an explicit acknowledgment at setup.
   Matches our trust model; worth declaring per organ.
9. **Scene first, then generate.** ArtCraft sets up layout, camera and pose before generating, which gives
   repeatable pictures. A good playbook for the image organ when the human wants a specific composition.
10. **The apps themselves.** Rather than writing our own photo, PDF or spreadsheet engines, LinuxAI could ship
    selected Craft CLIs as organs (their MCP servers wrapped by our checks). PDFCraft, GridCraft and
    PhotoCraft look the most useful. Caveat: they're young (alpha to early beta), and the ArtCraft name and
    logos are trademarks that forks must remove.

## Not for us

- The hosted model catalog and its accounts: we route paid generation through our own budget rules.
- Clean-room reimplementation of other apps (`craft-anything`): interesting, but not an OS feature.
- The older Unreal and motion-capture repositories: not maintained since 2021–2023.

## To decide later

Added to [CHECKLIST.md](CHECKLIST.md): staged-copy rule for all file-editing organs; an MCP surface for
the verb registry; evidence records after acting; summary-first results; an OS-wide tool → model table;
pure-Rust local background removal; and whether to ship any Craft apps as organs.
