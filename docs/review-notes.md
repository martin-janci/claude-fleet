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

An answer-only device sends nothing. The prompt goes through the hub's
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
