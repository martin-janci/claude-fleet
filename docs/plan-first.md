# Plan first

A checkbox on **New session** for work you want planned before any code
changes. The agent researches, writes a plan file, and stops. You read the
plan and say go. There is no phase engine and nothing advances by itself.

## Starting

Tick **Plan first** in New session (an agent session started in the
foreground on this hub; not a shell, a background run, or another fleet).
The agent is told to:

1. research how things work today, describing and not proposing;
2. write the plan to `docs/plans/<date>-<short-slug>.md`, uncommitted: the
   goal, what it found, the steps as small vertical slices that each end
   with a check, and its open questions for you;
3. stop, say "Plan ready" with the path, and wait; and say it again each
   time it changes the plan.

With a ticket and **Brief Claude with the ticket** on, the paragraph rides
under the brief in the same first prompt. Otherwise it is sent as the
session's first prompt (`queue_prompt`), and your task is the next message.
A ticket started in several repositories tells every session it starts. When
the prompt cannot be sent, a toast names the session and the reason; the
session is started all the same.

The phone's New session has the same tick under **First message**: the
paragraph goes out under what you typed there, as one prompt, on a plain
start and on a ticket start alike.

## Plan ready

The conversation shows a **Plan ready** card when all of this holds:

- the agent's turn has ended;
- since the last prompt it said "Plan ready" (or "the plan is ready") and
  named the file;
- that file is among the session's changes: a new or edited Markdown file
  directly in `docs/plans/` (or `docs/superpowers/plans/`).

The card carries the file's path and:

- **Read plan** shows the file rendered, in place (on the phone: in Files).
- **Approve and start** sends one prompt saying the plan is approved.
- **Ask for changes…** puts the file's name in the composer for you to say
  what to change; the work does not start.
- **×** (**Hide** on the phone) hides the card.

The card is about what the agent said last. The next prompt takes it away,
whoever sent it and from wherever: Approve on the card, "looks good, go
ahead" typed by hand, or a colleague's answer on another device. When the
agent changes the plan and says "Plan ready" again, the card is back.
Someone who may not write to the session sees the card with its answers off.

Because the agent's own words are the signal, the card shows on every
device and for a session started anywhere: with the checkbox, by Control, or
by typing the same instructions yourself. An agent that does not say "Plan
ready" gets no card and still waits, in words.

The session's changes are read when the agent's turn ends, once for what it
said. If that read fails (the host or the hub does not answer), a card that
is showing stays, and one that was not showing yet appears the next time the
session is opened. A session on another fleet has no card in this window:
its own hub's window, and a phone paired with that hub, show it.

## What is kept, and where

Only that a card was answered (approved or hidden), **on this device**
(`localStorage`, the newest 200 sessions; the phone keeps its own newest
100 and drops them when it pairs with another hub), so it does not come
back while the approval waits in the queue. The plan itself is a file in
the branch, reviewed and committed with the code. The hub stores nothing
new and no tool changed.

## Measure

Kept if it is used: tasks started with it per month, and how many plans are
approved without a request for changes. Under three uses in a month, remove
the checkbox (HumanLayer plan v2, Batch 3).
