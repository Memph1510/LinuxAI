# Calendar

The calendar is part of the AI, not an app. There is no calendar screen: it's the AI's knowledge of your time.
You tell it things in conversation ("I've got the dentist Thursday at 11", "the kids are off school next week")
and it keeps them. It uses the calendar whenever it plans, decides or talks with you: what's coming up,
conflicts, free time, travel and preparation, and when to bring something up.

Outside content works the same way as email: it's information, never instructions.

## What's in it

| Kind | Where it comes from | Who can change it |
|---|---|---|
| Your events | You, in conversation; the AI writes them down | The AI, directly (it's its record of your time) |
| Accepted invitations | An invitation you said yes to | as above |
| Subscribed calendars | `.ics` feeds (school, team, holidays) | Nobody: refreshed from the source, read-only |
| Invitations | `.ics` in an email, or imported with `--invite` | Only answered, after the AI brings it up with you |

An invitation **never puts itself on the calendar**. The AI weighs who sent it and what it clashes with, brings
it up with you (or follows its playbook), and records your answer. Its description is outside text. If the
organizer cancels, it comes off.

## How the AI uses it

- **Every turn**, the next four days and any pending invitations are in the AI's context (`<calendar>`), so your
  schedule is part of every answer and plan without the AI having to look it up.
- **The `calendar` tool:** `agenda`, `free`, `invites` to read; `add`, `change`, `cancel`, `answer_invite` to
  keep it up to date. Recurring events are expanded (daily, weekly by day, monthly, yearly; interval, count,
  until).
- **In plans**, the verbs `schedule`, `reschedule`, `unschedule`, `answer_invite` let a bigger job include a
  calendar step (book the test drive, then put it on the calendar).
- **An arriving invitation** fires the default rule `invitation-arrived`, and the AI thinks about it.
- **`calendar.soon`** fires once, 30 minutes before each event (`LINUXAI_CALENDAR_LEAD` to change). If you're
  away, it reaches you as a text.

Times are stored as UTC seconds. `.ics` times with `TZID`, `Z`, floating and all-day dates are all understood;
times you type ("2026-11-24 14:00") are your local time.

## Setting it up

Subscriptions: put them in `/etc/linuxai/calendar.env` and the `linuxai-calendar` service starts:

```sh
LINUXAI_CALENDARS="work=https://example.com/work.ics;school=https://example.org/district.ics"
LINUXAI_CALENDAR_EVERY=900   # seconds between refreshes
```

Most calendar services (Google, Outlook, iCloud, Fastmail) give a private "secret address in iCal format".

One-off imports:

```sh
linuxai-core calendar import ~/Downloads/season.ics --name soccer   # a read-only subscribed calendar
linuxai-core calendar import invite.ics --invite                    # an invitation to answer
```

Invitations in email need nothing: the mail organ finds `text/calendar` parts and `.ics` attachments.

## Core ops

| Op | Who | What |
|---|---|---|
| `calendar {from?, to?}` | anyone | agenda, default the next 7 days |
| `calendar_free {from?, to?, minutes, day_start?, day_end?}` | anyone | open slots |
| `invites` | anyone | pending invitations |
| `calendar_import {ics, kind: feed\|invite, source}` | organs | read `.ics` in as outside information |
| `calendar_add {event}`, `calendar_change {id, fields}`, `calendar_cancel {id}`, `invite_answer {id, answer}` | the AI, the session | keeping it up to date |

Events: `calendar.added`, `calendar.changed`, `calendar.cancelled`, `calendar.invite`,
`calendar.invite_cancelled`, `calendar.feed_updated`, `calendar.soon`.

## Not yet

- Sending replies to the organizer (an iMIP REPLY email) when an invitation is answered: needs outgoing mail.
- Two-way sync with a calendar server (CalDAV). Subscriptions are read-only for now.
- Exceptions to repeating events (`EXDATE`, moved single occurrences).
