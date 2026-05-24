---
name: fleet-friendly-name
description: Set this claude-fleet session's friendly display name when picking up a new task. Triggers on a new task, a task switch, or a stale session label (e.g. "fix login bug" beats "dev-claude-fleet").
---

# fleet-friendly-name

You are running inside a tmux session managed by claude-fleet. The sidebar
shows either the raw `tmux_name` or a short human-readable label
(`friendly_name`) that **you set** via MCP. Setting a good label whenever you
start a task makes the fleet sidebar scannable.

## When to run

- At the start of a new task (user prompt that opens a new topic).
- When the current task changes significantly (different file/feature).
- Once per task — do **not** update mid-task on every commit.

If unsure, prefer setting it. The sidebar falls back to `tmux_name` when
`friendly_name` is empty, so a stale label is worse than none.

## How to run

The session's `tmux_name` is reliable identity. The `host_alias` is
**configuration** — whatever the user chose in the claude-fleet host picker,
not derivable from `hostname`. Always discover the alias from the source of
truth; never guess.

1. **Read your tmux session name.** Single Bash call:

   ```bash
   tmux display-message -p '#S'
   ```

2. **Discover the alias programmatically.** Call
   `mcp__claude-fleet__list_sessions {}` and find the row whose `tmux_name`
   exactly matches your `<#S>`. Take that row's `host_alias`.

   If multiple rows match the same `tmux_name` (rare: same name on different
   hosts), prefer the row with `status: "running"` and, if still ambiguous,
   the one whose `project_id` matches your current working directory's
   project. If you still cannot pin one row, see "Cannot resolve" below.

3. **Pick a 3–6 word label.** Imperative phrase, no quotes/punctuation, no
   ticket IDs or branch names. Examples:
   - `add friendly name to sessions`
   - `debug login redirect loop`
   - `migrate auth middleware`
   - `review hardening spec`

4. **Set the name.** One MCP call with the discovered alias:

   ```
   mcp__claude-fleet__set_friendly_name {
     host_alias: "<discovered alias>",
     tmux_name: "<#S>",
     friendly_name: "<label>"
   }
   ```

Three tool calls total: Bash + list_sessions + set_friendly_name.

Do not try a hostname-based "fast path" first — `hostname -s` returns the OS
hostname, not the claude-fleet alias, and the two diverge whenever the user
renames a host in the picker. Guessing wastes a tool call on every miss.

Do not announce the call to the user, do not summarize the result — the
sidebar updates live via the row event.

## Clearing the label

Pass an empty string as `friendly_name` to clear it (the row falls back to
`tmux_name` in the sidebar).

## Cannot resolve

If `list_sessions` returns zero rows matching your `tmux_name`, this tmux
session is not registered with claude-fleet (you're running outside the
fleet, or the fleet backend hasn't reconciled yet). Skip the label silently —
the sidebar doesn't have a row to update anyway.

If multiple rows match and you cannot disambiguate, emit a single short
notice to the user and stop:

> claude-fleet: multiple sessions match tmux_name `<name>`; cannot pick one
> for the friendly-name label. Please set it manually from the app or via
> `set_friendly_name` with the correct `host_alias`.

## Token discipline

Three tool calls per task pickup: Bash + list_sessions + set_friendly_name.
No hostname-guessing fast path — that path silently breaks on renamed hosts
and wastes a call when it does. Do not chat about the label.
