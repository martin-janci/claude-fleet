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
  an error is red with a ✗, and the group label ends with `· 1 failed`.
- **Duration.** A finished turn shows how long it took (`2m 14s`) under its
  reply.
- **Live indicator.** Under the last turn: a pulsing row with the REPL's own
  spinner text (`Cooking… 3s · ↓ 306 tokens`) while Claude works, *Sent,
  waiting for Claude…* right after you send, and an amber *Claude is waiting
  for you in the terminal* banner with an *Open terminal* button when the
  pane shows a permission or question dialog.
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
  Review by default. A click fills the box so you can read or edit before
  Enter; Shift+click sends at once. Edit them under *Settings → Conversation
  composer*. A session stuck on *press Enter* gets a ⏎ chip that sends a bare
  Enter.
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
