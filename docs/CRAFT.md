# The Craft apps

LinuxAI's AI finishes tasks with files using the ArtCraft **Craft apps**: complete editing engines written in
Rust (MIT or Apache-2.0). It drives each one over MCP. You give it a photo, a PDF, a spreadsheet or a video
with a task, and it uses the same engine a person would use in the app.

| App | For |
|---|---|
| photocraft | Photo editing, with 1,000+ commands: layers, selections, filters, adjustments, retouching. **Select Subject and Remove Background run on this machine**, with no cloud and no account. |
| lightcraft | RAW photo development, non-destructive adjustments, presets, batch export |
| vectorcraft | Vector art: shapes, paths, text, logos, icons, SVG |
| filmcraft | Video editing: cut, trim, arrange, titles, transitions, audio, export |
| effectcraft | Motion graphics and compositing |
| soundcraft | Audio: edit, mix, effects, clean-up, export |
| pdfcraft | PDF: read and find text, comments, forms, rotate/move/delete/insert pages, combine, split, protect, metadata |
| wordcraft | Documents (DOCX and more), styles, tables, PDF export |
| gridcraft | Spreadsheets: workbooks, formulas, charts, XLSX/CSV |
| deckcraft | Slides (PPTX), layouts, notes |
| designcraft | Page layout: flyers, brochures, posters, multi-page print |
| figcraft | Interface design and prototyping |
| cadcraft | CAD drawings |
| texturecraft | 3D texture painting |
| materialcraft | 3D materials (node-based) |

## How a task goes

1. `open {app, files}`. The first time an app is needed, it's installed (see below). A session starts on
   **copies** of the files (`/work/in/…`), and the app may write only to `/work/out`.
2. The AI looks at what's there, using the app's inspect and preview tools. It sees pictures the app returns.
3. It makes the change with the app's own tools and commands.
4. It saves into `/work/out` and looks at the result.
5. `keep` copies a result next to the original as `name.ai-N.ext`, and checks the copy is identical.
6. `replace` puts a kept result in place of the original, but only after you say yes. The original is kept as
   `name.before-ai-<time>.ext`.
7. `close`, or 20 idle minutes, removes the session's copies and outputs.

## Safety

- **Verified installs.** The manifest (`/usr/share/linuxai/craft/apps.json`, reviewed in this repository) pins
  each app's version and SHA-256. An install must match that hash and carry a good GPG signature from the
  **ArtCraft Linux Release Signing Key**, primary fingerprint `2B76 741B 414A AD6C D33E 6312 13A0 05C0 B9D7 C3ED`.
  Only the CLI is kept, and it must answer MCP in the sandbox before it's put in place. A failure at any step
  leaves nothing behind. New versions come only through a reviewed change to the manifest.
- **A sandbox for every app** (bubblewrap). The app has no network and sees only the system's programs, itself,
  read-only copies of its inputs, and its output folder. Your home folder, other files and the network aren't
  there. The boot test checks this.
  - **Four apps rely on the sandbox alone**: FilmCraft, LightCraft, GridCraft and CADCraft have no folder limits
    of their own.
  - **The other eleven** also enforce their own read and write folders.
- **Originals are never touched.** Apps work on copies, results are kept beside the originals, and replacing an
  original asks you.
- **Documents are outside information.** Text inside a file (a PDF's words, layer names, metadata) can try to
  steer an AI. Everything after a craft result is labeled outside, so plans built on it get the usual checks.

## Pieces

| Piece | What |
|---|---|
| `core/src/craft.rs` | the organ: verified installs, MCP client, sandboxed sessions, keep and replace |
| `linuxai-craft.service` | a user service holding sessions; skips root |
| `linuxai-core craft do '<json>'` | one request to it; `craft apps` lists the apps |
| the `craft` AI tool | `apps`, `open`, `tool_info`, `call`, `keep`, `replace`, `close` |

Installed apps live in `~/.local/share/linuxai/craft/apps/<app>-<version>/`. Results from a session with no files
of yours go to `~/craft/<app>/`.

## Tested

- **Unit tests** (a stand-in app, a real GPG key and signature):
  - a tampered download is refused;
  - a signature from the wrong key is refused;
  - a good install keeps only the CLI;
  - a session works on copies and writes only to its output folder;
  - a kept result is placed and checked;
  - replace asks and keeps a backup;
  - closing a session cleans it up.
- **The real apps in Arch with bubblewrap:**
  - **PdfCraft:** installed through the full chain (17 tools). It read an invoice's text, rendered page 1,
    and saved a copy that was kept as `invoice.ai-1.pdf`, with the original unchanged.
  - **PhotoCraft:** installed through the full chain (1,009 commands). It removed a photo's background offline
    in under a second.
  - **The sandbox:** paths outside the session were refused, `/work/in` was read-only, and there was no
    network.
