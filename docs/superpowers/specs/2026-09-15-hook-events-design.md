# Hook events: SessionEnd, StopFailure, Notification

**Date:** 2026-09-15
**Scope:** `src-tauri/src/commands/mcp.rs` (hook block), `src-tauri/src/mcp/hooks.rs`
(payload), `src-tauri/src/service/hooks.rs` (handlers), `src-tauri/src/store/sessions.rs`
(writes), `docs/control-api.md`, `skills/claude-fleet-control/SKILL.md`. No migration
(every column already exists), no frontend change (rows arrive via `session:updated`),
no tool-description change (reference stays byte-identical).

Follow-up to `2026-09-14-mcp-transport-and-contract-design.md`.

## Problem

Fleet learns a session's state from three Claude Code hooks (`Stop`, `UserPromptSubmit`,
`PostToolUse(EnterWorktree|ExitWorktree)`) and otherwise from the reconcile pass
(`claude agents --json` plus pane scraping every tick). Three states are therefore
late or wrong:

- A turn that ends in an **API error** (rate limit, auth, overloaded) fires
  `StopFailure`, not `Stop`. The row stays `working` (with a fresh `last_hook_at`
  from `UserPromptSubmit`, so the pane heuristic will not even correct it until the
  next pass starts after the stamp), `turn_seq` never bumps, and `run_prompt` /
  `wait_for_session { turn_gt }` wait out their full timeout.
- A session that **exits** (`SessionEnd`) is only noticed when reconcile finds the
  tmux pane gone or showing a shell; `stopped` is in the vocabulary but nothing
  authoritative produces it.
- A **permission prompt / elicitation / usage-limit wait** (`Notification`) is
  detected by pane scraping only, one tick late at best, and only for the shapes
  `pane_intel` recognises.

All fleet hosts run Claude Code ≥ 2.1.214; `type: "http"` hooks exist since 2.1.63,
`StopFailure` since 2.1.78, `Notification` matchers since 2.0.37, the
`quota_auto_resume_*` types since 2.1.234 (older hosts simply never fire them).

## Design

### Hook block (`FLEET_HOOK_EVENTS`)

Three entries are added; each is the same `hook_entry(port, token)` (`type: "http"`,
bearer header, 5 s timeout). `SessionEnd` hooks share a 1.5 s budget by default;
a per-hook `timeout` raises it, so fleet's 5 s entry is honoured.

| Event | Matcher | Effect |
|---|---|---|
| `SessionEnd` | `logout\|prompt_input_exit\|other` | `claude_status = stopped`, `idle_since` kept/started, `stuck_kind` cleared, timeline `session_end` with the reason. `clear` and `resume` are deliberately not matched: the process lives on under a new session id. |
| `StopFailure` | `` (all) | Same write as `Stop` (`idle`, `turn_seq + 1`, `last_stop_at`, `last_turn_at`) so waiters return and read the error from the transcript, plus timeline `stop_failure` with the `error` type. |
| `Notification` | `permission_prompt\|elicitation_dialog\|elicitation_url_dialog\|quota_auto_resume_stale\|quota_auto_resume_disabled\|quota_auto_resume_fired` | see table below |

`Notification` mapping (`notification_type` → status, stuck):

| type | `claude_status` | `stuck_kind` | note |
|---|---|---|---|
| `permission_prompt` | `blocked` | unchanged | Claude waits on a tool approval |
| `elicitation_dialog`, `elicitation_url_dialog` | `blocked` | unchanged | an MCP server asks the user |
| `quota_auto_resume_stale` | `blocked` | `press_enter` | Claude Code waits for Enter after a long sleep; the existing `press_enter` playbook resolves it |
| `quota_auto_resume_disabled` | `blocked` | unchanged | needs a human |
| `quota_auto_resume_fired` | `working` | cleared | the task resumed by itself |

Every notification also records a timeline event `notification` whose detail is the
`notification_type`. `idle_prompt`, `auth_success`, `elicitation_complete`,
`elicitation_response`, `agent_needs_input`, `agent_completed` are not matched
(no state change fleet needs, or only fire while the agent view is open).

Not installed, and why: `SessionStart` (Claude Code accepts only `command` /
`mcp_tool` hooks there), `SubagentStart/Stop`, `PreCompact`. `allowedHttpHookUrls`
is never written: defining it at user level would block every other http hook the
user has.

### Payload

`HookPayload` gains `reason`, `notification_type`, `message`, `title`, `error`,
`error_details` (all `Option<String>`, unknown fields still ignored) and derives
`Default` so test literals stay short. `message` / `title` are never stored — only
`notification_type` and `reason` / `error` reach the timeline.

### Store writes

Three new `Store` methods mirror `record_stop_hook` (match by `claude_session_id`,
stamp `last_hook_at = now` so the reconcile guard keeps the hook's word, emit
`session_updated`, return the row or `None`):

- `record_session_end_hook(id)` — `claude_status='stopped'`, `idle_since =
  COALESCE(idle_since, now)`, `stuck_kind = NULL`, `stuck_since = NULL`,
  `last_turn_at = now`.
- `record_stop_failure_hook(id)` — identical SQL to `record_stop_hook` (reused: the
  method calls it), the difference is the timeline event the handler writes.
- `record_notification_hook(id, status: ClaudeStatus, stuck: Option<Option<StuckKind>>)`
  — `Some(Some(k))` sets `stuck_kind=k` (restarting `stuck_since` when the kind
  changes, keeping it when equal), `Some(None)` clears both, `None` leaves both
  untouched; `idle_since` follows the status via `idle_since_sql`.

### Handlers (`service::hooks`)

`apply_hook` dispatches `SessionEnd`, `StopFailure`, `Notification` to three new
functions with the same shape as `apply_prompt_submit_hook`: `host_checked_row`
(a host token may only report about its own host), `remember_transcript_path`,
the store write, then a best-effort `insert_session_event`. `SessionEnd` with a
reason outside the matched set and `Notification` with an unmapped type are no-ops
(defensive: the matcher already filters, but a hand-posted body must not flip
state).

### Docs

`docs/control-api.md`: step 5 of *Provisioning hosts* lists the six events; the
*Orchestration* paragraph mentions `StopFailure`; a new **Hook contract** subsection
under *Security* documents the endpoint, auth, the event → matcher → effect table,
the consumed fields, the HTTP answers (204 / 400 / 403 / 500) and the re-provision
note. `skills/claude-fleet-control/SKILL.md` gets one sentence: `blocked` and
`stopped` are hook-driven on re-provisioned hosts; an API-error turn ends with a
`stop_failure` timeline event.

## Testing

- `commands/mcp.rs`: the merged settings block carries all six events with the
  exact matchers; re-running stays idempotent; a legacy block with only the old
  three is upgraded in place.
- `mcp/hooks.rs`: the new payload fields deserialize; unknown fields still ignored.
- `store/sessions.rs`: each `record_*` method's row effect, including `stuck_since`
  restart-vs-keep and the `None` → untouched case; unmatched id → `None`.
- `service/hooks.rs`: through `apply_hook` — `SessionEnd(other)` → stopped + event;
  `SessionEnd(clear)` → no-op; `StopFailure` → idle + turn_seq bump + `stop_failure`
  event with the error; each `Notification` type → the mapped status / stuck; an
  unmapped type → no-op; the host-binding refusal (`E_FORBIDDEN`) applies to all
  three; unknown session → no-op.
- `no_eprintln_tests`, `reference_is_current` unchanged.
