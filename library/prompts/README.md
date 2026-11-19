# LinuxAI prompt library

Reusable prompts, one markdown file each. Read them, copy them, edit them; the OS keeps track of which ones
help. Each file starts with a short header:

    ---
    title: what it does
    use: the kind of work (matches playbook categories, e.g. freight/negotiate)
    model: any | local | claude   (local = written to work well on a small local model)
    tags: comma, separated
    ---

Then the prompt, with {{slots}} to fill in. The AI can suggest new prompts or changes; you approve them.

## What's here

A prompt for most things you'd do on a computer. Prompts marked *(small local model)* are written to run
well on a cheap on-device model; the rest suit any model. The AI adds to this as new kinds of work come up
(with your OK).

- **accounts** (4): Cancel a subscription or account; Update my details everywhere; Check whether something is a scam; Review an account's security
- **calendar** (5): Plan my day; Find a time that works; Sort out a scheduling conflict; Put something on the calendar; Weekly review
- **code** (9): Automate a repeated task; Explain code; Fix a bug; Write a pattern (regex); Review code; Write a script; Write a shell command; Write a database query; Write tests
- **data** (3): Find duplicate records; Turn text into a table; Spot unusual entries
- **decisions** (4): Check before acting; Decide with weighted criteria; Handle something for the first time; Weigh pros and cons
- **documents** (9): Pull action items out of a document; Compare two versions of a document; Fill in a form from what we know; Write a proposal or quote; Review a contract or agreement; Write step-by-step instructions; Summarize a document; Make a reusable template; Write a report
- **email** (9): Ask someone for missing information; Politely sort out a number that doesn't add up; Decline politely and keep the door open; Follow up when someone hasn't answered; Negotiate a price or terms; Remind someone about a payment; Propose a meeting time; Triage new email; Decide whether to keep a mailing list
- **files** (5): Plan backups; Find a file; Free up disk space; Name a file from its contents (small local model); Organize a folder
- **finance** (9): Plan a monthly budget; Categorize a transaction (small local model); Compare quotes or offers; Put together an expense report; Create an invoice; Check a bill; Audit subscriptions; Make a tax prep checklist; Flag unusual charges
- **freight** (12): Fill in a broker's carrier packet; Check out a broker before booking; Counter a broker's offer; Claim detention pay; Plan the week's dispatch; Find loads that fit; Check hours of service for a run; Invoice a broker; What does this lane pay right now?; Work out whether a load pays; Chase a late broker payment; Check a rate confirmation against what was agreed
- **images** (12): Compare pictures; Describe a picture; Make a diagram; Edit a photo by description; Make a flyer; Logo concepts; Clean up a photo; Clean product photo; Remove a picture's background; Size a picture for where it's going; Explain what's on a screen; Make a social media graphic
- **learning** (4): Explain a concept; Learn a new program; Quiz me; Teach me step by step
- **legal** (2): Write a dispute letter; Read the terms for me
- **local** (13): Sort an email into a category (small local model); Pull out money amounts (small local model); Pull out dates and times (small local model); Is this urgent? (small local model); Detect the language (small local model); One-line summary (small local model); Pull specific fields out of text (small local model); Pick a category (small local model); How does the sender feel? (small local model); One-sentence summary (small local model); Spam or scam check (small local model); Give a text a title (small local model); Answer yes or no (small local model)
- **media** (5): Cut a clip; Convert a media file; Write show notes; Summarize a video or recording; Transcribe audio
- **meetings** (4): Write a meeting agenda; Follow up after a meeting; Turn meeting notes into a summary; Prepare for a meeting
- **messages** (6): Write an apology; Prepare for a phone call; Prepare a difficult conversation; Send an update to a group; Reply to a text message; Leave a voicemail
- **personal** (6): Prepare for an appointment; Find a gift; Fix something at home; Plan meals; Plan a move; Cook with what's on hand
- **research** (10): Look into a company or person; Is this claim true?; Compare options and recommend one; Research a topic in depth; Find good sources; Find a local business or service; Check whether a price is fair; Research a product before buying; Answer a quick question; Look up a rule or regulation
- **shopping** (4): Find the best price; Reorder supplies; Return or refund something; Summarize product reviews
- **spreadsheets** (5): Analyze a table of data; Design a spreadsheet; Clean up messy data; Write a spreadsheet formula; Reconcile two lists
- **summaries** (2): My day in a few lines; Summarize a long document for a decision
- **system** (10): Make the battery last longer; Explain an error message; Install software; Plan routine maintenance; Fix a network problem; Set up a device; Check the machine's security; Fix a failed service; Find out why the computer is slow; Update the system safely
- **tasks** (8): Ask the human a good question; Break a big task into steps; Make a checklist; Estimate how long something takes; Plan a trip; Prioritize a list; Look back on a finished task; Write a status update
- **travel** (4): Compare places to stay; Make a packing list; Plan a route; Check weather along a route
- **web** (8): Book something online; Buy something online safely; Download and file something; Pull listings off a page; Fill in a web form; Find something on a page; Hand a page to the human; Watch a page for changes
- **work** (3): Answer a customer; Write a job post; Price a job
- **writing** (14): Write an announcement; Write a short bio; Change the tone of a text; Write a complaint that gets results; Write a cover letter; Draft an email; Outline a piece of writing; Proofread; Reply to a message; Rewrite something to be clearer; Shorten a text; Write a social media post; Write a thank-you note; Translate
