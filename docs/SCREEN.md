# The screen: Hyprland and Quickshell

LinuxAI is built on **Arch Linux** (rolling release; kernel 7.3 in the current image). Linux is only the
hardware layer; everything you see is LinuxAI's.

## Layers

| Layer | What | Why |
|---|---|---|
| Window layer | **Hyprland** (Wayland compositor) | The AI can drive it completely through its control socket: open, focus, arrange, fullscreen, close; it reports focus changes and windows opening and closing; and it knows which window you're in and where your pointer is |
| Always-on layer | **Quickshell** (QML) | Native, light, reloads instantly: the status line now; next the voice indicator, the AI's questions, and its pointer, highlight and magnifier over any app |
| Pages | **Chromium** (the shared browser), on screen under Hyprland | The Workspace pages and web pages, one engine, which the AI already drives |

Used as they come (pinned versions, our own configuration and our own Quickshell code), not forked. A fork only
if we need something their plugins and control socket can't do.

## The compositor organ (`linuxai-core compositor`)

Started by the Hyprland session. Talks to Hyprland's control and event sockets.

- `windows`, `active` (the window you're in and your pointer, also relative to that window), `monitors`
- `focus`, `workspace` (move a window to workspace N), `fullscreen`, `float`, `close`; only real window
  addresses reach Hyprland
- `effects {on}`: animations, blur and shadows; turned off automatically on battery and back on when plugged in
- Events to the core: `screen.focus`, `screen.window.opened`, `screen.window.closed`

## Starting the screen

`linuxai-screen` (run at login on the main console) starts Hyprland with `/etc/linuxai/hypr/hyprland.lua` (Hyprland 0.55+ is configured in Lua).
If the machine can't draw it (a VM without graphics acceleration, typically), it falls back to the text
session and writes why to `~/.local/state/linuxai/screen.log`. `LINUXAI_SCREEN=text` always uses the text
session.

Keys for now: Super+Enter a LinuxAI session, Super+Q close, Super+F fullscreen, Super+arrows move focus.
