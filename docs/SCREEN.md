# The screen: Hyprland and Quickshell

LinuxAI is built on **Arch Linux** (rolling release; kernel 7.3 in the current image). Linux is only the
hardware layer; everything you see is LinuxAI's.

## Layers

| Layer | What | Why |
|---|---|---|
| Window layer | **Hyprland** (Wayland compositor) | The AI can drive it completely through its control socket: open, focus, arrange, fullscreen, close; it reports focus changes and windows opening and closing; and it knows which window you're in and where your pointer is |
| Always-on layer | **Quickshell** (QML) | Native, light, reloads instantly. Above every app: the status line, the AI's questions as cards you tap to answer, the voice indicator, and the AI's pointer (an outline and a note) over whatever it's talking about. Driven by the core's event stream (`linuxai-core follow`); never blocks your clicks |
| The Workspace (the book) | **Chromium** (the shared browser), its first window, as an app window | The main screen: what needs you, the work, the selected task's steps, what's happening, and the page being worked on, live inside it |

Used as they come (pinned versions, our own configuration and our own Quickshell code), not forked. A fork only
if we need something their plugins and control socket can't do.

## The compositor organ (`linuxai-core compositor`)

Started by the Hyprland session. Talks to Hyprland's control and event sockets.

- `windows`, `active` (the window you're in and your pointer, also relative to that window), `monitors`
- `focus`, `workspace` (move a window to workspace N), `fullscreen`, `float`, `close`; only real window
  addresses reach Hyprland
- `effects {on}`: animations, blur and shadows; turned off automatically on battery and back on when plugged in
- `shot {window? | app?}`: a screenshot for the AI to look at (grim), with its origin on screen
- `point {x, y, w, h, note?, window? | app?}`: outline an area for you, over any app (screen coordinates, or inside
  a window); the always-on layer draws it
- Events to the core: `screen.focus`, `screen.window.opened`, `screen.window.closed`

## Starting the screen

`linuxai-screen` (run at login on the main console) starts Hyprland with `/etc/linuxai/hypr/hyprland.lua` (Hyprland 0.55+ is configured in Lua).
If the machine can't draw it (a VM without graphics acceleration, typically), it falls back to the text
session and writes why to `~/.local/state/linuxai/screen.log`. `LINUXAI_SCREEN=text` always uses the text
session.

Keys for now: Super+Enter a LinuxAI session, Super+Q close, Super+F fullscreen, Super+arrows move focus.

## The AI's `screen` tool

Inside the graphical session the AI has a `screen` tool: the windows that are open, the window you're in and
where your pointer is (what "this" means outside the browser), screenshots to look at, pointing at things in any
app while it talks, and arranging windows. Screenshots count as outside information, like web pages and email.

## Events the always-on layer shows

| Event | Shown as |
|---|---|
| `plan.waiting` (a question for you) | A card at the top right with the question, why the AI is unsure, what it would pick, and the answers to tap (or, for a question with no ready answers, where to answer it) |
| `voice.listening`, `voice.heard`, `voice.speaking`, `voice.done` | The voice indicator at the bottom |
| `screen.point` | An amber outline and a note over the area, for a few seconds; with `magnify`, also a lens beside it showing the area twice as large (a still capture of the screen) |

## The Workspace

The main screen is a page (`/usr/share/linuxai/workspace/index.html`) in the shared browser's first window, an app
window with no tabs or address bar. The browser organ gives that one page a private channel to the OS
(`core/src/workspace.rs`): no network server, and a call is honored only while the tab still shows the Workspace
page itself. Through it the Workspace:

- shows what needs you (questions with answer buttons, or a box to type an answer), the work (tasks), the
  selected task's steps (yours marked), and what's happening (events as they come);
- answers questions as you (it's your screen);
- shows **a web page inside the book**: a live picture of the page (Chromium's screencast), where your clicks,
  scrolling and typing go to the real page. When the AI shows you something (`browser show`), it opens there.

It can ask the core only for `tasks`, `task`, `waiting`, `answer`, `events` and `state`.
Opened on its own (without the OS) it shows example data.
