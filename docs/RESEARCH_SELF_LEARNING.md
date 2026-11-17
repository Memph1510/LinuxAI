# Self-learning harnesses: what exists, and what LinuxAI can take from it

Researched 2026-11-17. Sources are at the bottom. Benchmark figures are as reported by the authors or secondary
sources and were not re-verified.

## The big picture

Almost every serious self-learning system now works the way LinuxAI already does: **the model stays frozen,
and the knowledge around it evolves** (playbooks, skills, memory, the harness itself). Nobody retrains the
model to learn from experience. That confirms our direction (playbooks + cases + lessons). The projects below
show how to do it *well*, and which mistakes to avoid.

## What each one does

| Project | What it is | The idea worth taking |
|---|---|---|
| **DeepSeek Harness** (`dsh`, DeepSeek, Aug 2026, MIT, preview) | A plugin-based agent runtime. The model adapter, tool registry, session state and agent loop are all swappable plugins. Append-only event log of every message, tool call, reasoning step and sub-agent dispatch, for replay and evaluation | **Not self-learning.** It confirms our design (organs ≈ plugins, event log + journal ≈ its trajectory log). Worth taking: *replay a recorded trajectory to evaluate a change* |
| **ACE: Agentic Context Engineering** (Stanford & SambaNova, ICLR 2026; `ace-framework` package) | Context as an **evolving playbook**. Three roles: a *Generator* does the task, a *Reflector* critiques the trace and extracts lessons, a *Curator* turns them into small **delta entries** merged into the playbook by simple non-AI logic | **Delta updates, not rewrites.** Rewriting a whole playbook each time causes *context collapse*. In one reported case an 18k-token playbook was rewritten down to 122 tokens and accuracy fell below having no playbook at all. Also: separate the doing from the reflecting |
| **Memento-Skills** (ICML 2026, open source) | A library of **markdown skill files** (spec + instructions + helper code). A learned router picks the most *behaviorally* relevant skill. After failures, the system patches the skill | **Validate before saving:** every automated change is tested on a synthetic case before it enters the library. **Choose by what worked, not by similar wording** (80% vs 50% success). Reported GAIA 52% → 66% |
| **Recuris** (arXiv 2608.24876, Aug 2026) | A fixed Meta-Agent reads execution traces, works out **which component** caused a failure, and proposes a **localized** edit. A fixed **validation gate** keeps the edit only if it passes; otherwise the old memory stays. Working memory tracks goals as pending / done / blocked, with evidence | **Attribute, then make a local fix, then gate it.** **The harness fetches the right skill before every state-changing action**, instead of leaving it to the model (83.6% vs 65.6%). **A goal is "done" only when the evidence supports it.** +15 to +18 points on several benchmarks; bigger gains on longer tasks |
| **Evo-Harness**, **SkillHone**, **Living-Harness** (2026 papers) | Variations: compile reflections into a structured skill harness; keep a persistent record of *why* each skill was revised and what evaluation supported it; update memory after deployment | **Every change carries its reason and its evidence** |
| **Hermes Agent** (Nous Research, MIT) | Markdown memory files (`memory.md`, `user.md`); writes a `SKILL.md` after complex tasks (agentskills.io standard); full-text search over past sessions (SQLite FTS5) | **Periodic self-review:** every ~10 turns it asks itself whether something belongs in memory or a skill. **A portable skill format** |

## What we should take, in priority order

1. **Delta updates to playbooks (ACE).** Today the AI rewrites a whole playbook to change one point. That's the
   context-collapse risk. Change to small entries (add / revise / retire one point). Each entry has an id, its
   evidence (case ids or your comment), and a count of how often it helped or hurt. The markdown file stays
   readable and editable by you; it's generated from the entries.
2. **Validation gate before a lesson is kept (Memento-Skills, Recuris).** Before a playbook change is saved,
   replay the recent cases in that category against it: "with this rule, would past decisions have come out the
   same, or better?" If it would have made things worse, the change is held for your review instead of saved.
3. **Localized fixes from failure attribution (Recuris).** When a task fails, the reflection names *which*
   part was wrong (a playbook point, a missing variable, the plan, a wrong fact) and edits only that.
4. **The harness fetches knowledge, not the model (Recuris).** The core already gives playbooks to the AI when
   a task needs a plan. Extend that to every plan that changes something: the playbook points in scope are
   attached automatically, and the assessment has to say which ones it followed or deliberately didn't.
5. **"Done" needs evidence (Recuris).** A task step counts as done only with evidence (a result, a confirmation,
   a check), not just because the plan ran.
6. **Choose cases by what worked (Memento-Skills).** Rank similar cases by outcome as well as matching
   variables.
7. **Periodic self-review (Hermes).** At quiet times, the mind reviews recent activity for lessons it missed.
8. **Portable format (Hermes / agentskills.io).** Consider making playbooks compatible with the `SKILL.md`
   standard, so knowledge can move between tools.

What we should **not** take: any of these as a framework dependency. They're built around a chat agent. LinuxAI
is an OS with its own core, task layer and event log, so we take the ideas, not the code.

## Sources

- InfoQ: [The Open-Sourcing of DeepSeek Harness](https://www.infoq.com/news/2026/08/deep-seek-harness/)
- The New Stack: [DeepSeek open sources an agent harness where everything is a plugin](https://thenewstack.io/deepseek-harness-open-source-plugins/)
- [DeepSeek Harness](https://deepseek.com/en/harness/)
- ACE paper: [Agentic Context Engineering (arXiv 2510.04618)](https://huggingface.co/papers/2510.04618); [review](https://www.themoonlight.io/review/agentic-context-engineering-evolving-contexts-for-self-improving-language-models); [ace-framework on PyPI](https://pypi.org/project/ace-framework/0.8.7/)
- VentureBeat: [New framework lets AI agents rewrite their own skills without retraining](https://venturebeat.com/orchestration/new-framework-lets-ai-agents-rewrite-their-own-skills-without-retraining-the) (Memento-Skills)
- [Recuris (arXiv 2608.24876)](https://www.alphaxiv.org/abs/2608.24876)
- [Evo-Harness (arXiv 2608.15071)](https://www.alphaxiv.org/abs/2608.15071); [SkillHone (arXiv 2606.08671)](https://arxiv.org/pdf/2606.08671); [Living-Harness (arXiv 2607.26598)](https://www.alphaxiv.org/abs/2607.26598.md)
- Hermes Agent: [docs](https://hermes-agent.nousresearch.com/docs); [Better Stack guide](https://betterstack.com/community/guides/ai/hermes-agent/)
