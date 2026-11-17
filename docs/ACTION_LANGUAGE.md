# LinuxAI Action Language: Draft 1

> Status: **first draft for discussion.** Syntax, keywords and the name can all change.
> Working name: **the action language**. Naming it is an open question.
> Companion document: [SYSTEM_INTERFACE.md](SYSTEM_INTERFACE.md).

## In plain terms

This is the language the AI uses to *do* things. Most of it will be written by the AI and read by you, so it is
designed for three things:

1. **You can read it.** A plan reads close to English, and the system can show any plan as plain sentences.
2. **The system can check it before it runs.** Do the things it mentions exist, does it fit your budget and your
   standing directions, which steps cost money or reach the outside world?
3. **It does exactly what it says.** There are no hidden side effects. Only verbs change anything, and every
   change is recorded.

It is deliberately small. The AI's intelligence goes into *choosing* the plan; the language just has to say the
plan clearly and safely.

## A first look

```
plan "Pay the Verizon bill"
  because "bill arrived by email; due Nov 28"
  confidence 0.9

  let bill = find bill from @verizon where status = unpaid, newest
  let usual = average amount of (find bill from @verizon, last 6)

  if bill.amount > usual by more than 20%:
    ask "Verizon is ${bill.amount}, about {percent(bill.amount, usual)} above the usual ${usual}. Pay it?"
      options "pay", "hold", "I'll look"
    expect answer is "pay" otherwise stop

  pay bill from budget:bills
  remember "Paid Verizon ${bill.amount} on {today}"
```

Shown to you as:

> **Pay the Verizon bill** (because the bill arrived by email; due Nov 28)
> 1. Find the newest unpaid Verizon bill.
> 2. Compare it with the average of the last 6 Verizon bills.
> 3. If it is more than 20% above that, ask you before paying.
> 4. Pay it from the bills budget. 💳 *costs money: $112.40 · within mandate "Bills"*
> 5. Remember the payment.

---

## 1. Values

| Kind | Examples |
|---|---|
| text | `"Sam's wedding"` |
| number | `3`, `0.8`, `20%` |
| money | `$84.12`, `€40`, `$900` (currency is part of the value; adding $ to € is an error) |
| time | `today`, `tomorrow 9am`, `2026-06-14`, `next Friday`, `in 3 days`, `6 months ago` |
| duration | `3 days`, `2h`, `45 minutes` |
| yes/no | `yes`, `no` |
| references | `@verizon` (handle), `#denver-trip` (tag or project), `budget:bills`, `obj:bill/01JDQ8K3...` |
| lists | `["aisle", "window"]` |
| records | `{ from: DEN, to: AUS, date: 2027-06-12 }` |

Times are resolved against your time zone and calendar when the plan is checked, so `next Friday` becomes a
concrete date you can see before anything runs.

## 2. Finding things

`find` queries the memory:

```
find bill from @verizon where status = unpaid, newest
find message where channel = email and from = @landlord, last 10
find appointment between today and next Sunday
find anything about "the Denver wedding"            -- search by meaning
find people linked to #denver-trip
```

- `where` filters, `, newest` / `, oldest` / `, last N` / `, sorted by <field>` shape the result.
- A `find` that should return one thing but returns none or several is an error at check time, unless the plan
  handles it (`if none:` / `pick one`). The AI can't act on a guess without saying so.

Simple calculations: `count`, `sum`, `average`, `min`, `max`, `percent`, and the usual `+ - * /` and comparisons.

## 3. Doing things: verbs

A verb is an action an organ provides. The general shape is:

```
<verb> <thing> [preposition value]...
```

```
pay bill from budget:bills
send message to @mom saying "Landed, see you at 6"
reply to message saying "Thanks, Tuesday works"
book flight { from: DEN, to: AUS, date: 2027-06-12, seat: aisle } within budget:travel
schedule appointment "Dentist" at next Tuesday 3pm for 1h
move document "lease.pdf" to #apartment
set power profile to "saver"
set cpu policy to "efficiency"
limit program @steam memory to 4GB
restart service @networkmanager
run program "git" with ["pull"] in #linuxai          -- the programs organ: escape hatch to ordinary software
```

Each verb has a declared signature (what it takes, what it returns, what kind of effect it has, whether it can be
undone). The checker uses that to validate the call and to show you the plain-language version.

## 4. Talking with you

```
tell "Booked: Southwest 1432, Jun 12, 8:05am, aisle, $214."

ask "Which dates for Denver?" options "Jun 11–15", "Jun 12–14"
let dates = answer

ask "Anything I should know about this trip?"           -- free-form answer
```

- `ask` **pauses the plan durably**. It waits through hours, days and reboots for your reply, by text or on screen,
  then continues from that exact point.
- `tell` sends a message and continues.
- `answer` holds the reply. The plan can use it like any other value.

## 5. Control

```
let x = ...                     -- name a value

if <condition>:                 -- branches
  ...
else:
  ...

for each m in find message where needs_reply:
  ...

expect <condition> otherwise stop                -- a check; if it fails, stop and report
expect <condition> otherwise ask "…"             -- or ask you what to do

try:
  book hotel ...
otherwise:                                       -- what to do if a step fails
  tell "That hotel is full; holding two alternatives."

wait for payment.settled about bill up to 3 days
wait until 2026-11-27 9am
```

There are no general-purpose loops that can run forever, and no user-defined functions in draft 1. A plan always
has a clear, finite shape the checker can reason about. (Reusable pieces come from rules and solutions, below.)

## 6. Plans

Every multi-step action is a `plan` with a header:

```
plan "Book flights for Sam's wedding"
  because "wedding Jun 14 in Denver (invite email Nov 2); you said 'book it'"
  confidence 0.75
  budget budget:travel up to $600
  ...
```

- **because** is required. Every plan says why it exists, in one line, so the journal reads as a story.
- **confidence** is the AI's own estimate. Below your threshold, the checker turns the first external or
  money-costing step into an `ask`.
- **budget** reserves an amount up front, so several plans can't overspend the same budget at once.

## 7. Rules: what the system learns to do on its own

A rule is a plan that runs when an event happens:

```
when power.unplugged:
  set power profile to "saver"
  set cpu policy to "efficiency"

when bill.arrived from @verizon:
  run plan "Pay the Verizon bill"

when message.received where from = @mom and mentions "call":
  tell "Mom asked you to call."
```

- Rules run **without the AI model**: instantly, offline, at no cost.
- The AI proposes rules after it has handled the same situation more than once. You accept, edit or decline
  them. Accepted rules are listed, readable and switchable at any time.

## 8. Mandates

Your standing directions, in the same language (see SYSTEM_INTERFACE.md §4.3):

```
mandate "Bills"
  allow pay for bill from @verizon, @xfinity, @state-farm
  within budget:bills
  when amount is within 20% of usual

mandate "Machine"
  allow set, limit, restart, install for host        -- the AI runs the computer
```

When a plan is checked, every effectful step must be covered by a mandate. Any step that isn't becomes a question
to you.

## 9. Remembering

```
remember "Paid Verizon $112.40 on 2026-11-20"
believe "Prefers aisle seats" confidence 0.85 because find booking where seat = aisle
learn solution "Wifi drops after suspend" steps ...
```

These create `note`, `preference`/`belief` and `solution` objects linked to the plan that produced them.

## 10. How a plan runs

1. **Parse** into a structure (stored as data, so plans can be saved, replayed, compared and turned into rules).
2. **Check:**
   - every reference resolves, every value has the right type, and every verb exists with valid arguments;
   - times are resolved to concrete values;
   - money is estimated and charged against budgets;
   - every effectful step is covered by a mandate;
   - confidence is compared with your threshold.
   The result is either *ready* or a list of problems. The AI fixes its own mistakes here, before anything runs.
3. **Run** step by step. External steps carry idempotency keys. `ask` and `wait` pause the plan durably.
4. **Journal** each step: intent, inputs, result, cost, and undo where available.
5. **Finish** with a `plan.finished` or `plan.failed` event, and a one-line summary to the activity view.

## 11. Grammar sketch (draft 1)

```
plan        := "plan" TEXT header* block
header      := "because" TEXT | "confidence" NUMBER | "budget" REF ("up to" MONEY)?
rule        := "when" EVENT_PATTERN ("where" cond)? ":" block
mandate     := "mandate" TEXT ("allow" verb_list "for" target_list)+ ("within" REF)? ("when" cond)?
block       := INDENT stmt+ DEDENT
stmt        := let | if | for | expect | try | wait | ask | tell | verb_call | remember | believe | learn
let         := "let" NAME "=" expr
if          := "if" cond ":" block ("else:" block)?
for         := "for each" NAME "in" expr ":" block
expect      := "expect" cond "otherwise" ("stop" | ask)
try         := "try:" block "otherwise:" block
wait        := "wait for" EVENT_PATTERN ("about" expr)? ("up to" DURATION)? | "wait until" TIME
ask         := "ask" TEXT ("options" TEXT ("," TEXT)*)?
tell        := "tell" TEXT
verb_call   := VERB expr? (PREP expr)* record?
expr        := find | REF | literal | NAME ("." NAME)* | expr OP expr | FUNC "(" args ")" | "answer"
find        := "find" TYPE ("from" REF)? ("where" cond)? ("," shape)*
```

Text in `"…"` can embed values with `{name}` and money with `${name}`.

## 12. Questions to settle together

1. **Name.** What should the language be called?
2. **Feel.** Is the English-like style right, or would you prefer something shorter and more symbolic?
3. **Should you write it too?** Draft 1 assumes the AI writes and you read, edit and approve. Do you also want to
   write mandates and rules by hand?
4. **Escape hatch.** The `run program` verb lets the AI fall back to ordinary software and shell commands for
   anything the language doesn't cover yet. Keep it unlimited while the language grows, or restrict it?
5. **First verbs.** For v0.1 (the OS working as an OS), the first verbs are the host verbs: power, CPU policy,
   memory limits, services, devices, network, install and update. Anything you want on day one beyond those?
