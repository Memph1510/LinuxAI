# The shared browser

One real Chromium that you and the AI use **at the same time**: your logins, your tabs, its tabs, one browser.

## How it works

`linuxai-core browser` (Rust, `core/src/browser.rs`) starts Chromium with its own profile
(`~/.local/share/linuxai/browser`, so logins persist) and talks to it over a **private pipe**
(`--remote-debugging-pipe`). There is no debugging port, so nothing else on the machine or network can attach to
it. The organ is the browser's only controller; the AI reaches it through the `browser` tool.

- **With a screen** (a Wayland or X session), Chromium opens as a normal window and that's your browser.
- **With no screen** (the live ISO today, a server, the phone later), it runs headless and works the same way;
  you'll see it through the Workspace screen and the Android app (both later).

## Using it together

Every tab has an owner.

| | You | The AI |
|---|---|---|
| Tabs you open | yours | can read them and take screenshots, can't click/type/navigate/close |
| Tabs it opens | — | its own, in a separate unfocused window so it never takes over what you're doing |
| Handing over | "take this tab" → it carries on from where you are | it gives you a tab when it needs you (a login, a captcha, a judgment on the page) with a note; reach texts you; you give it back |

The AI sees a page as cleaned text (visible text only, normalized, invisible characters removed) plus numbered
controls (links, fields, buttons), so "type into 3, click 7" is cheap and precise. It clicks and types with
real input events, as a person would. Password values are never read back.

## What a page says is information, never instructions

Page text and screenshots are labeled outside information, like email: anything the AI submits after reading a
page is labeled outside, must cite the facts it relies on, and acting on the outside world from it needs your OK.
The AI doesn't submit forms that spend money, send messages or accept terms for you without your OK.

## Why this setup (research, 2026-11)

- The common approaches are: attach an agent to your own browser over CDP (direct, but Chrome 136+ refuses
  remote debugging on the default profile because infostealers abused it); hosted cloud browsers with a live
  viewer (Browserbase and similar: production-ready, but someone else's machine and a subscription); and
  Chromium forks with an agent built in (BrowserOS, AGPL). There's no standard for a person and an agent
  co-using one browser yet; the per-tab ownership here is our own answer.
- Chromium (not Google Chrome) is unaffected by the default-profile restriction, but we use our own profile
  directory anyway, and a pipe instead of a port.
- Sources: [Chrome: changes to remote debugging switches](https://developer.chrome.com/blog/remote-debugging-port),
  [cdp-agent-kit](https://github.com/Uname58/cdp-agent-kit) (agent and human in one browser, per-tab),
  [cdp-browser-mcp](https://github.com/Tsutomu-miku/cdp-browser-mcp) (per-agent tabs on one profile),
  [Browserbase](https://www.browserbase.com/blog/what-is-a-browserbase-browser) (hosted, live view),
  [BrowserOS](https://github.com/browseros-ai/BrowserOS) (agentic Chromium fork).

## Requests (one JSON object per line on `$XDG_RUNTIME_DIR/linuxai-browser.sock`)

`tabs` · `open {url}` · `read {tab}` · `go {tab,url}` · `click {tab, ref|target|selector}` ·
`type {tab, ref|target|selector, text, enter?}` · `key {tab,key}` · `shot {tab}` · `give {tab,to,note}` ·
`close {tab}`. `open`, `go`, `click`, `key` and `type … enter` return the page as it is afterward, so one step is
one round trip.

## Next

- Live view of a headless browser for the Workspace screen and the phone (CDP screencast), so you can watch and
  take over from anywhere.
- Browser steps in the action language (so a plan can, say, check a load board every hour without the model).
- Downloads land in your files; uploads from them.
