---
name: fleet-friendly-name
description: Set this claude-fleet session's friendly display name (sidebar label). Fires on deterministic signals only — the first user prompt of a session, the first user prompt after a `/clear` command, a ~10-prompt heartbeat re-check, or an explicit user ask to relabel. Skip otherwise.
---

# fleet-friendly-name

You are running inside a tmux session managed by claude-fleet. The sidebar
shows either the raw `tmux_name` or a short human-readable label
(`friendly_name`) that **you set** via MCP. A good, current label makes the
fleet sidebar scannable.

## When to run

Fire on these deterministic signals — never on vibes:

1. **First user prompt of this conversation.** No prior `set_friendly_name`
   tool call exists in the transcript yet.
2. **First user prompt after `/clear`.** The most recent user turn contains
   the harness marker `<command-name>/clear</command-name>` (emitted when
   the user runs `/clear`). `/clear` wipes context, so the next prompt is by
   definition a new task — relabel.
3. **Heartbeat every ~10 user prompts.** Count user prompts since your last
   `set_friendly_name` call. When the count crosses 10, glance at the
   current label vs. the recent work; relabel if they no longer match. If
   they still match, do nothing — don't burn the call.
4. **Explicit user ask.** "relabel this", "set the name to X", "rename the
   session".

Do **not** fire on commits, file saves, file switches, or a fuzzy "the task
feels different". A stale label is annoying; a flapping label is worse.

## How to run

The session's `tmux_name` is reliable identity, and fleet resolves it for you.
The `host_alias` is **configuration** (whatever the user chose in the
claude-fleet host picker, not derivable from `hostname`), so never guess it.

1. **Read your tmux session name.** Single Bash call:

   ```bash
   tmux display-message -p '#S'
   ```

2. **Find your own row.** Call `mcp__claude-fleet__whoami { tmux_name: "<#S>" }`.
   It returns your single session row; take its `id`. It is a few dozen
   tokens, where `list_sessions` returns the whole fleet (thousands).

   If it answers `E_AMBIGUOUS` (the same name on several hosts), its details
   list `{session_id, host_alias}` candidates: pick the one on the host you
   are running on — if you cannot tell, see "Cannot resolve" below.

3. **Pick a 3–6 word label.** Imperative phrase, no quotes/punctuation, no
   ticket IDs or branch names. Examples:
   - `add friendly name to sessions`
   - `debug login redirect loop`
   - `migrate auth middleware`
   - `review hardening spec`

4. **Set the name.** One MCP call with the id from step 2:

   ```
   mcp__claude-fleet__set_friendly_name {
     session_id: <id>,
     friendly_name: "<label>"
   }
   ```

   Remember the `id` for later fires in this conversation (heartbeat,
   after `/clear`): then only step 4 is needed.

Three tool calls on the first fire: Bash + whoami + set_friendly_name. Later
fires in the same conversation need one. A heartbeat that confirms the
current label is still right costs zero MCP calls — the decision is read-only
against transcript state.

Do not try a hostname-based "fast path" first — `hostname -s` returns the OS
hostname, not the claude-fleet alias, and the two diverge whenever the user
renames a host in the picker. Guessing wastes a tool call on every miss.

Do not announce the call to the user, do not summarize the result — the
sidebar updates live via the row event.

## Clearing the label

Pass an empty string as `friendly_name` to clear it (the row falls back to
`tmux_name` in the sidebar).

## Cannot resolve

If `whoami` answers `E_NOTFOUND`, this tmux session is not registered with
claude-fleet (you're running outside the fleet, or the fleet backend hasn't
reconciled yet). Skip the label silently — the sidebar doesn't have a row to
update anyway.

If `set_friendly_name` returns `E_FORBIDDEN`, this host's control-API token is
`readonly`: the label is a write to the session row, so it is refused. Skip
the label silently and do **not** retry — a permission answer never becomes a
different answer.

If `whoami` is ambiguous and you cannot pick your host, emit a single short
notice to the user and stop:

> claude-fleet: multiple sessions match tmux_name `<name>`; cannot pick one
> for the friendly-name label. Please set it manually from the app or via
> `set_friendly_name` with the correct `session_id`.

## Token discipline

First fire: three tool calls (Bash + whoami + set_friendly_name); later fires
in the same conversation: one (set_friendly_name with the remembered id).
Heartbeat: zero MCP calls if the label still matches. Never `list_sessions`
for this — it returns the whole fleet to find one row. No hostname-guessing
fast path. Do not chat about the label.
