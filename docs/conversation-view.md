# The Conversation view

The Session tab holds two views of a running session — Conversation and
Terminal — switched by the segmented control at its top right or by
⌘J (Ctrl+Shift+J on Windows/Linux). Which one you land on is a remembered
preference that defaults to Conversation; a row that can only offer one view
(no `claude_session_id` yet → Terminal, no tmux pane → Conversation; a row
that is both lands on Conversation) shows that one without touching the
preference, so stepping off it returns you to where you were.

The Conversation view shows a session's Claude Code transcript as turns and
lets you drive the session from there, without the terminal. It works for
every tmux-backed session; background (`bg:`) and external rows are
read-only, since they have no REPL to type into — for these rows Conversation
is the only view, since they have no tmux pane to show a Terminal for.

## Reading

- **Turns.** Each turn is your prompt (relative time, long prompts clamp
  behind *Show more*) and the reply: prose rendered as Markdown (code blocks
  have *Copy*), and one muted line per tool call. Consecutive tool calls fold
  into a group (`7 tool calls · Bash, Read, Edit +2`); the running turn's
  last group stays open so calls show as they land. A call whose result was
  an error is red with a ✗, and the group label ends with a red `1 failed`.
- **Tool details.** Click a call to open its input and result, laid out by
  tool: an edit (Edit, MultiEdit, Write) as a line diff with old / new line
  numbers, `+N −M` and the file name in its header, and long unchanged runs
  folded to `⋯ N unchanged lines`; a Read as the file's numbered lines; a
  Search / Find as its list of files; Update todos as a checklist; a Run as
  `$ command` and its output. Anything else shows its raw input and result.
  On a desktop paired with a hub the detail is read through the hub
  (`session_tool_detail`).
- **Duration and reply actions.** Under each reply, one footer: how long
  the turn took (`2m 14s`) on the left, and the reply's actions on the
  right — *Copy* (every text block of the reply), *Quote*, then, apart
  from those, *Retry*, *Fork here* and *Rewind here*. The row rests dimmed
  and comes up when you point at the turn or tab into it.
- **Live indicator.** Under the last turn: a pulsing row with the REPL's own
  spinner text (`Cooking… 3s · ↓ 306 tokens`) while Claude works, *Sent,
  waiting for Claude…* right after you send, and an amber *Claude is waiting
  for you in the terminal* banner with an *Open terminal* button when the
  pane shows a permission or question dialog.
- **Cards.** A task report (a `FLEET_TASK_DONE_…` line and its JSON) and
  a `fleet-ui` block (a tutorial, a guide, a callout, facts, choices or a
  form) are drawn as cards, not as code. A card that acts only fills the
  composer. The format is [Chat blocks](chat-blocks.md).
- **File paths.** `src/lib/foo.ts:42` in a reply is a link: it opens the
  Files tab on that file, at that line.
- **↓ N new.** When you have scrolled up and more content lands, the button
  at the bottom right counts it; click it to jump back down.
- **Load older.** The tab reads the last ten turns. *Load older* under the
  notice at the top fetches ten more each time, up to a hundred.

## Prompting

- **Composer.** Type at the bottom; Enter sends, Shift+Enter breaks a line.
  What you send goes through the same path as the *Send prompt* dialog
  (tmux `send-keys` into the REPL), so it is exactly what the terminal would
  have received. A half-typed prompt survives switching tabs.
- **Slash commands.** Type `/` for Claude Code's built-in commands with a
  one-line description each; a prefix narrows the list, arrows move, Tab or
  Enter completes, Enter on the exact name sends.
- **Chips.** Quick actions above the box: Clear, Compact, Status, Continue,
  Go on, Review by default, drawn in the order you set. A click fills the box
  so you can read or edit before Enter; a chip with *Send* ticked (marked ↵)
  sends on a click instead, and Shift+click does the other one on either
  kind. While the session is waiting on an answer — a permission or choice
  prompt, or any stuck screen — no chip sends: the click only fills the box
  and the note under it says why, so a prompt is never typed into that
  menu; answer it in the terminal, then press Send. Edit, reorder (↑/↓) and
  tick *Send* under *Settings → Conversation composer*; the list is the
  fleet's, shared with the phone, and if another device saved it first the
  editor shows that list and asks you to make your change again. A session
  stuck on *press Enter* gets a ⏎ chip that sends a bare Enter.
- **Recall.** ArrowUp in an empty box brings back earlier prompts, newest
  first; ArrowDown walks forward again.
- **Context meter.** `ctx 83%` next to the box shows the context window in
  use; from 70 % the Compact chip is outlined.

## How it stays cheap

The transcript is read every 5 s while the view is open and the session is
unknown, working or blocked, and only every 15 s (or the moment a turn
completes) when it is quiet. The live indicator probes the pane every 2 s
only while something is happening, one probe at a time, and a probe never
outranks the row's own status for more than 10 s.
