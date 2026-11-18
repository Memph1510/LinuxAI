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
