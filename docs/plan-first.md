# Plan first

A checkbox on **New session** for work you want planned before any code
changes. The agent researches, writes a plan file, and stops. You read the
plan and say go. There is no phase engine and nothing advances by itself.

## Starting

Tick **Plan first** in New session (an agent session started in the
foreground on this hub; not a shell, a background run, or another fleet).
The agent is told to:

1. research how things work today, describing and not proposing;
2. write the plan to `docs/plans/<date>-<short-slug>.md`: the goal, what it
   found, the steps as small vertical slices that each end with a check, and
   its open questions for you;
3. stop, say "Plan ready" with the path, and wait.

With a ticket and **Brief Claude with the ticket** on, the paragraph rides
under the brief in the same first prompt. Otherwise it is sent as the
session's first prompt (`queue_prompt`), and your task is the next message.

## Plan ready

When the session's turn ends and its changes hold a new Markdown file
directly in `docs/plans/` (or `docs/superpowers/plans/`), the conversation
shows a **Plan ready** card with the file's path:

- **Read plan** shows the file rendered, in place.
- **Approve and start** sends one prompt saying the plan is approved.
- **Ask for changes…** puts the file's name in the composer for you to say
  what to change; the work does not start.
- **×** hides the card for that plan.

The card asks once per plan file. A second plan file in the same session
asks again. Someone who may not write to the session sees the card with its
answers off.

## What is kept, and where

Which sessions were started with Plan first is remembered **on this device**
(`localStorage`, the newest 200). Another device shows no card; the agent
still waits there, in words. The plan itself is a file in the branch, reviewed
and committed with the code. The hub stores nothing new and no tool changed.

## Measure

Kept if it is used: tasks started with it per month, and how many plans are
approved without a request for changes. Under three uses in a month, remove
the checkbox (HumanLayer plan v2, Batch 3).
