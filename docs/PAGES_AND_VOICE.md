# Pages, not a browser; talking about what's on the screen

The goal: the web isn't a separate program you open. It's part of the AI, the same as your email, tasks and
pictures. **A web page is just another page in the book.**

## Pages in the book

The Workspace is one surface (the "book"). Its pages are the things you and the AI are working on: a task, an
email thread, a picture, a decision, and web pages. A web page appears **inside the work it belongs to**:

- "Find a load out of Dallas" → the task page shows the load board the AI is searching, live, next to the loads
  it has found so far and its reasoning.
- The AI needs you to log in → that page comes to the front of the book with one line: "Log in here, then say done."
- You open a page yourself ("show me the weather on I-40") → it's a page in the book like any other; the AI is
  already looking at it with you.

There's no tab bar, no address bar you have to manage, no separate window. Underneath, it's the shared Chromium
from [BROWSER.md](BROWSER.md). It's the same engine that draws the Workspace, so a web page sits in the book as
naturally as any other page. Tab ownership stays, invisibly: what you're on is yours, and the AI works on its own
pages until you hand one over by saying so.

## Shared attention

The AI always knows what you're looking at: which page, where you've scrolled, what you've selected or are
pointing at. So you can speak the way you would to someone sitting next to you:

- "What's this?" / "Is this a good rate?" (pointing at a load) → it answers about *that*.
- "Fill this in with the usual." → it fills the form you're on while you watch, and stops before submitting.
- "Not that one, the one below." → it follows.
- "Keep an eye on this and tell me if it drops under 2.10." → it becomes a watch task.

While it talks about something, it highlights it on the page. While it acts, you see what it's doing, and you can
interrupt at any time ("wait", "stop", "go back"): your voice wins.

## Voice

- **Talking:** push to talk first (a key on the laptop, a button on the phone), with a wake word later. Local
  speech-to-text (whisper.cpp) by default, so it's private, fast and works offline. A cloud model is used only when
  the local one isn't good enough on that machine.
- **Answering:** short spoken answers (local text-to-speech, e.g. Piper) for quick things, the page itself for
  everything else (it shows instead of reading out). You choose voice, text, or both.
- **Same on the phone:** the page you're on streams to the Android app; you talk to it there exactly the same way.
- Voice is the human's channel, so what you say is your intent. What a page or an email says is still only
  information, whatever it says, even if it's read out loud.

## What exists and what's next

| Piece | State |
|---|---|
| Shared Chromium, reading pages, numbered controls, acting, screenshots, handing pages back and forth | Done (`linuxai-core browser`) |
| Pages inside the Workspace (the book) | With the Workspace screen |
| "What the human is looking at" (page, scroll, selection, pointer) as a signal the AI always has | Next for the browser organ |
| Highlighting what the AI is talking about, live view of its actions | Next for the browser organ |
| Voice organ: push to talk, local speech-to-text and text-to-speech, interrupting | To build |
| Live page on the phone | With the Android app |
