# Review notes

Notes on lines of a session's changes, written **before anything is
pushed** and sent to the session's agent as one message. Once a pull
request is open, review happens on GitHub; this is the step before that.

## Writing notes

Open a changed file in the session's Files view and look at its Diff.
Click a line number to write a note on that line: an added or unchanged
line takes the new side, a removed line the old side. **Save note** (or
⌘↵ / Ctrl+↵) keeps it as a draft; Esc drops what you typed.

Drafts stay **on this device**, in the browser storage of this window, per
session. Nothing reaches the hub until you send. Editing a draft and
clearing its text deletes it.

## Sending

The bar above the diff counts the drafts: **Send N notes to Claude** (or the
session's agent) types every draft of the session, from every file, into
the session as **one** numbered message: who wrote them, then each note
with its file, line and the quoted line. If the session is busy it goes in
when its turn ends, through the same queue as Send later.

Each send carries an id (`client_msg_id`). If the answer never comes (a
timeout, a dropped connection), sending again reuses that id: the hub
finds the send already stored and types nothing a second time.

Sent notes are stored on the session (migration 223, `review_notes`) with
**who sent them** ("Ada · desktop"), and show under their line on every
device of the session. A person marks each one **Resolved** (or Reopens
it); the agent's reply in the conversation says what it changed.

## Who may do what

| | read the sent notes | write drafts | send, resolve |
|---|---|---|---|
| owner, or a `drive` grant | yes | yes | yes |
| a `read` grant (a viewer) | yes | yes | no |
| an agent's host token | its sessions | — | no |
| Control (the agent) | yes | — | no |

An answer-only device sends nothing. A note is a person's review: Control's
own text for a session goes through `send_prompt` / `queue_prompt`, where a
person approves it. The prompt goes through the hub's
untrusted-input marker for a caller that needs one, as `queue_prompt` does.

## Wire

`review_notes { session_id, action? }`:

- `list` (default): the sent notes, newest send first.
- `send { client_msg_id, notes: [{path, line?, side?: new|old, quote?, body}] }`:
  at most 50 notes, each at most 4000 characters; a path is relative to the
  checkout. Answers `{ session_id, notes, delivered, queued_id?, repeat? }`.
- `resolve { note_id, resolved? }`: `resolved: false` reopens.

The desktop command is `review_notes` (Routed). A hub older than the tool
answers `E_HUB_PROTOCOL`, which the diff shows as the send's error.

## Who sent each prompt

A hub with several people shows whose prompt each turn is. Every prompt a
hub tool types or queues (`send_prompt`, `queue_prompt`, a review-notes
send) leaves a row in `prompt_authors` (migration 224): the session, when,
the prompt's first 80 characters with whitespace folded, and the sender as
the notes show them ("Ada · phone"). `session_conversation` matches each
turn to the newest unused row whose head its prompt starts with, written no
later than the turn (two minutes of clock skew allowed) and no more than a
week before it, and returns it as the turn's `author`, with `mine` set when
the reader is that person. The conversation then says "You" for your own
prompts and the colleague's name for theirs. A prompt typed in the terminal
has no row and still reads "You". A session keeps its newest 500 rows.

## Ask for review

The owner of a session asks a colleague on the same hub to review its
changes before anything is pushed: **Ask for review…** above the diff (on a
desktop connected to a hub). The `review_request` tool (migration 225):

- `ask {session_id, person, note?}` (owner only) shares the session with that
  person and stores the ask. The share is `drive`, so the colleague's notes
  can go to the agent, or `watch` when every org they are in has them as a
  viewer: they read the changes, the notes and the conversation and write
  drafts, but cannot send. A share already in place is kept as it is (an
  ask never raises or lowers a grant). Asking the same person again
  refreshes the open ask.
- `list` (the default) answers the open asks to the caller, newest first
  (`done: true` adds those finished in the last week). The desktop shows
  them in the Inbox under "Needs you", each opening its session.
- `done {id}` marks an ask to you done (`done: false` opens it again); only
  the person asked can.

Taking the share back is the Share sheet's Revoke, as for any share.
