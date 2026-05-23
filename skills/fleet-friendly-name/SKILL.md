---
name: fleet-friendly-name
description: Set this claude-fleet session's friendly display name when picking up a new task. Triggers when the user gives a new task, you switch to a new task, or whenever the session label would be stale (e.g. "fix login bug" instead of "dev-claude-fleet"). Single MCP call, no chatter.
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

## How to run (one shot, ~3 tool calls)

1. **Discover identity.** Run a single Bash command:

   ```bash
   echo "$(tmux display-message -p '#S')|$(hostname -s)"
   ```

   The left side is `tmux_name`. The right side is your machine's short
   hostname — claude-fleet host aliases usually match it (e.g. `mefistos`,
   `mac`, `hetzner`). If `set_friendly_name` returns `E_NOTFOUND`, retry with
   `hostname` (no `-s`); if that also fails, see "Host alias mismatch" below.

2. **Pick a 3–6 word label.** Imperative phrase, no quotes/punctuation, no
   ticket IDs or branch names. Examples:
   - `add friendly name to sessions`
   - `debug login redirect loop`
   - `migrate auth middleware`
   - `review hardening spec`

3. **Set it.** One MCP call:

   ```
   mcp__claude-fleet__set_friendly_name {
     host_alias: "<hostname>",
     tmux_name: "<#S>",
     friendly_name: "<label>"
   }
   ```

That's it. Do not announce the call to the user, do not summarize the result —
the sidebar updates live via the row event.

## Clearing the label

Pass an empty string as `friendly_name` to clear it (the row falls back to
`tmux_name` in the sidebar).

## Host alias mismatch

If both `hostname -s` and `hostname` return `E_NOTFOUND`, this machine's
claude-fleet host alias does not match either form. **Do not guess** — emit a
single short notice to the user:

> claude-fleet: this host's alias does not match `hostname`/`hostname -s`. Open
> the claude-fleet app, find this host's alias in the host picker, and either
> rename the host to match `hostname -s` or invoke `set_friendly_name` once
> manually with the correct alias.

Then stop. Do not retry blindly; the user fixes it once and the next session
on this host works automatically.

## Token discipline

This skill should add ≤ 4 tool calls per task pickup: one Bash to read
identity, one MCP call, and at most one retry with the alternate hostname
form. Do not list_sessions, do not capture panes, do not chat about the label.
