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

Claude itself works in text and pictures. Its voice feature in the Claude apps turns speech into text before
Claude sees it, and the API has no audio input. So LinuxAI does the same thing as a thin layer of its own:

    you speak → speech to text → the AI (with eyes on your screen) → its answer → text to speech → you hear it

- **Talking:** push to talk. In the session, `/voice on`, then Enter on an empty line starts listening and Enter
  again stops (a key on the laptop, a button on the phone later; a wake word later still). `/talk` is a single
  turn. Anything you type or say cuts off what it's saying.
- **Speech to text** runs on the machine (whisper.cpp): private, fast, works offline. `linuxai-voice-setup`
  downloads the model once. Any other program or a cloud service can be used instead (`LINUXAI_STT_CMD`).
- **Speaking:** espeak-ng on the OS today (small, robotic); Piper for a natural voice (`LINUXAI_PIPER_VOICE`); or
  any program or service (`LINUXAI_TTS_CMD`). Answers are turned into something natural to say: no formatting,
  links named rather than read out, code and tables left on the screen.
- **A spoken turn is handled differently:** the AI knows it's a transcript (and may be misheard), that "this" and
  "here" mean what's on your screen, and that it should answer in a few spoken sentences and **show rather than
  describe**.
- Voice is the human's channel, so what you say is your intent. What a page or an email says is still only
  information, whatever it says, even if it's read out loud.

## Talking with things on the screen

Voice and the screen work together. Talking about a new car:

> You: "What do you think of the Summit Trail?"
> It pulls up the listing and the comparison in front of you (**show**), highlights the price and towing
> capacity while it talks about them (**point**, with a magnifying glass for small print), and says: "It's
> $38,250, about $3,000 more than the Ridgeway, but it tows 5,000 pounds, which covers your trailer. Want me to
> check the dealer reviews?"
> You, pointing at the mileage: "And this?" → it knows what's under your pointer (**look**) and answers about that.
> "Go deeper on reliability" → it opens owner reports, recalls and reviews and keeps showing as it goes.

With an internet connection it can pull up anything you're talking about; without one, it works from what's on
the machine.

## What exists and what's next

| Piece | State |
|---|---|
| Shared Chromium, reading pages, numbered controls, acting, screenshots, handing pages back and forth | Done (`linuxai-core browser`) |
| **look**: what's under your pointer (and the row or card it's in), what you've selected, a close-up for the AI's eyes; or a point the screen passes in | Done |
| **point**: highlight what the AI is talking about on any page, with a note and an optional magnifying glass; never blocks the page | Done |
| **show**: pull a page up in front of you while talking about it; the AI keeps driving it | Done |
| Voice: push to talk, local speech to text, speaking, cutting it off, spoken turns that show rather than describe | Done (`linuxai-core voice`, `/voice on`); real microphone untried until the laptop |
| Pages inside the Workspace (the book) | With the Workspace screen |
| Natural voice (Piper) on the OS image; wake word | Next |
| Live page on the phone, talk from the phone | With the Android app |
