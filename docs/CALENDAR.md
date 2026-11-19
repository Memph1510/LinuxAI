# Calendar

The calendar is part of the OS, not an app. It works the same way as email: what comes from outside is
information, the AI reads it and proposes, and the calendar only changes through the human or through a plan.

## What's on it

| Kind | Where it comes from | Trust | Who can change it |
|---|---|---|---|
| Your events | You (Workspace, `ctl`), or a plan with `schedule` | yours | You; the AI through a plan |
| Accepted invitations | An invitation you (or a plan) accepted | yours, linked to the invite | as above |
| Subscribed calendars | `.ics` feeds you subscribe to (school, team, holidays) | outside | Nobody: they are refreshed from the source |
| Invitations | `.ics` in an email, or imported with `--invite` | outside, pending | Only answered: accept, maybe, decline |

An invitation **never puts itself on your calendar**. It waits under "Needs you" in the Workspace, with any
clashes, until you or a plan answers it. Its description is outside text: facts to weigh, never instructions.
If the organizer cancels it, it comes off again.

## How the AI uses it

- Reads it with the `calendar` tool: `agenda`, `free` (open slots in your working day, default 8:00–18:00) and
  `invites`. Recurring events are expanded (daily, weekly by day, monthly, yearly; interval, count, until).
- Changes it only with plan verbs, which are journaled and checked like any other plan:
  `schedule(title, at, minutes?, end?, where?, notes?, rrule?)`, `reschedule(id, …)`, `unschedule(id)`,
  `answer_invite(id, answer)`. The core refuses direct changes from the AI (`calendar_add` and friends need
  `source: human`).
- An arriving invitation fires the default rule `invitation-arrived`: the AI thinks about it (who sent it,
  when, what it clashes with) and either asks you or follows its playbook.
- `calendar.soon` fires once, 30 minutes before each event (`LINUXAI_CALENDAR_LEAD` to change), and reaches you
  as a text if you're away: "LinuxAI: in 10 min: Call the dealer at phone".

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
| `calendar_add {event}`, `calendar_change {id, fields}`, `calendar_cancel {id}`, `invite_answer {id, answer}` | human only | direct changes |

Events: `calendar.added`, `calendar.changed`, `calendar.cancelled`, `calendar.invite`,
`calendar.invite_cancelled`, `calendar.feed_updated`, `calendar.soon`.

## Not yet

- Sending replies to the organizer (an iMIP REPLY email) when an invitation is answered: needs outgoing mail.
- Two-way sync with a calendar server (CalDAV). Subscriptions are read-only for now.
- Exceptions to repeating events (`EXDATE`, moved single occurrences).
