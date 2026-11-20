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

## Watching a page instead of rereading it

To keep an eye on a page (a load board, listings, a status page), the AI sets a **watch** once. The browser then
checks the page on its own, in a background tab, and tells the core only about **new rows that match**. The AI
isn't involved, and spends nothing, until something does.

```json
{"op": "watch", "name": "dal-atl", "url": "https://loadboard.example/search?lane=DAL-ATL", "every": "10m",
 "items": "tr.load", "id_field": "ref",
 "fields": {"ref": "td.ref", "lane": "td.lane", "rate": {"selector": "td.rate", "number": true}},
 "match": {"lane": ["has", "Atlanta"], "rate": [">=", 2500]}, "note": "DAL to ATL at $2,500+"}
```

- **Rows** are picked out with a CSS selector for one row, and **fields** within it (a selector, a regex, or a number
  parsed from text). Without `items`, each line of the page is a row. Hidden rows aren't rows, as with `read`.
- **Match** uses the same form as `find` in the action language.
- **The first look** happens right away: it returns what matches now and a sample row, so the AI can check the
  fields came out right. Everything on the page then counts as seen.
- **After that**, new matching rows become a `web.matched` event carrying just those rows, at most 20. They're
  outside information, like the page. The mind gets them at once, at medium effort, and a plan can wait on them
  like any event.
- **A page that empties** (logged out, or redesigned) becomes `web.watch_problem` once, and you're told directly,
  with no model involved.
- **Watches last**: they're saved with what was seen (`browser-watches.json` next to the profile), and keep going
  after restarts. They check at most once a minute. `watches` lists them; `unwatch {name}` stops one.
- They use the shared profile, so pages that need your login work as long as you're logged in.

On a simulated day of keeping an eye on a load board, this takes 9 model requests instead of 100 (a check every 30
minutes), and finds new loads sooner (every 10 minutes). See [THINKING.md](THINKING.md).

## Requests (one JSON object per line on `$XDG_RUNTIME_DIR/linuxai-browser.sock`)

`tabs` · `open {url}` · `read {tab}` · `go {tab,url}` · `click {tab, ref|target|selector}` ·
`type {tab, ref|target|selector, text, enter?}` · `key {tab,key}` · `shot {tab}` · `give {tab,to,note}` ·
`close {tab}` · `look {tab, x?, y?}` (what's under the human's pointer, selected, plus a close-up) ·
`point {tab, quote|ref|target, note?, magnify?}` (highlight for the human) · `show {url, note?}` (pull a page up
in front of them) · `watch {url, name, every, items, fields, match, id_field, note}` · `watches` · `unwatch {name}`.
`open`, `go`, `click`, `key` and `type … enter` return the page as it is afterward, so one step is
one round trip. See [PAGES_AND_VOICE.md](PAGES_AND_VOICE.md) for how this works with voice.

## Next

- Live view of a headless browser for the Workspace screen and the phone (CDP screencast), so you can watch and
  take over from anywhere.
- Browser steps in the action language, for pages that need clicking through before the rows show.
- Downloads land in your files; uploads from them.
