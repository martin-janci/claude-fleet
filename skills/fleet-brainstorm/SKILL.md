---
name: fleet-brainstorm
description: Brainstorm something new with the person before any work starts, in stages they pass one at a time (options, then a choice, then decisions, then a plan), and leave the result in fleet as a task whose notes hold the plan and proposed subtasks a person accepts. Use when the person wants to create, design or plan something new, says "brainstorm", "let's think this through", "vymyslime", "naplánujme", or asks how to approach a larger piece of work. Skip for a question with one answer or a task that is already clear.
---

# fleet-brainstorm

You are the fleet operator. A person wants to make something new, and the
point of this skill is that the thinking happens **with** them, in the open,
before any session starts or any code is written. The result stays in fleet,
so it is still there after this conversation is gone.

## The rule that matters most

**You never advance a stage on your own.** Each stage ends with what you
found and the question that closes it. The person answers, and only then do
you move on. If they say "just do it", go straight to the next stage, but
still stop at its end. Nothing is created in fleet before the Plan stage,
and nothing starts a session at all: starting work is the person's own
click on the task's **Work** button.

Answer in the language the person uses. Keep each stage short enough to read
on a phone.

## The four stages

### 1. Diverge

Restate the goal in one sentence, and the constraints you heard (deadline,
repository, "no new dependencies", budget). Ask about a constraint only when
an option depends on it.

Then give **3 to 7 options**. For each one, give:
- a name of two or three words;
- one line on the trade-off;
- its unknowns, meaning what you would have to find out first.

At least one option should be the smallest thing that could work, and one
should be the option the person probably has not considered.

Close with: *Which options should I keep (pin), drop, or expand? Any
constraint I missed?*

If the person asks for more, give new options and do not repeat the old
ones. Keep a running list of every option and its fate (pinned, dropped or
merged). The plan records it later, so "why not X?" always has an answer.

### 2. Converge

Score only the pinned options against the constraints, in a small table
(option / fits / costs / risk). Name the top risk of each option in one
line.

If a choice depends on a fact you do not have, say so and offer a
**research spike**: a read-only look at a repository or a host, done by you
with the read tools you already have (`session_conversation`,
`list_sessions`, the repository on disk). Do not start a session for it
without asking.

Close with a recommendation and the question: *Shall we go with A (or
merge A and C)?*

### 3. Decide

Write one short decision record for each choice that was made:

```
D1  <what was decided>
    why: <one line>
    rejected: <the alternatives, and why each one lost>
```

Close with: *Do these decisions read right? Edit any line, or say yes.*

### 4. Plan

Write the plan as plain text, not markup, because it is stored and shown as
text. It has these sections:

```
Goal: <one sentence>
Non-goals: <what this deliberately does not do>
Options: <each option: pinned / dropped / merged, with the one-line reason>
Decisions: <D1…Dn from stage 3>
Risks: <top risks, each with its mitigation>
Tasks:
  1. <title> (done when: <observable check>)
  2. …
```

Plan **at most 10 tasks**. If the effort is larger, plan the first part and
name the rest as a later plan. Each task should be something one session can
finish and that a person can check, with a "done when" line that is a check
rather than a feeling (for example "the tests in X pass" or "the page shows
Y").

Show the plan and ask: *Shall I put this into fleet?*

## Putting it into fleet (only after the person says yes)

You have the `claude-fleet` MCP tools. Use these exact calls:

1. **The root task.** `work_link { action: "create", title, notes }`. The
   title is the goal in a few words, and `notes` holds the whole plan text
   from stage 4. Notes are cut at 4000 characters, so keep the plan under
   that and shorten the Options section first if you must. The answer is
   the new item; note its `id` and its key (`TASK-<id>`).
   If the person named a project, pass its `project_id`, so the task's Work
   button knows where to start.
2. **The subtasks**, one call per task in plan order:
   `work_link { action: "propose", parent: "item:<id>", title, notes, why }`.
   `notes` holds the task's own scope and its "done when" line, and `why`
   holds the one line that ties it to a decision (for example "D2: the cache
   lives in the hub"). These are **proposals**: a person accepts or rejects
   each one on the desktop or the phone. You cannot accept them, so do not
   try, and do not ask the person to let you.
3. **When the work belongs to an existing task** (a Jira, Asana, Linear or
   GitHub ticket, or a `TASK-` the person names), skip step 1. Propose the
   subtasks under that item (`parent: "item:<id>"`, with the `id` that
   `work { action: "lookup", key }` answers), and put the
   plan's Goal and Decisions into the first proposal's `notes`. Tell the
   person that the full plan is in this conversation only. If they want it
   kept, offer a separate root task for it.

Errors you may meet:
- `E_LIMIT` means 10 proposals already wait on that task. Stop and ask the
  person to decide those first.
- `E_EXISTS` means the same title was already proposed there and rejected.
  Leave that task out, since the person already said no to it, and mention
  that you did.
- `E_INVALID` saying the item "is itself a subtask" means fleet nests only
  one level deep. Propose under that subtask's parent instead, and say
  which task the new ones belong to in their `why`.

Finish with one short paragraph: the task's key, how many subtasks wait for
a decision, and that the person can accept them and press **Work** on any
of them when they are ready.
