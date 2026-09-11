# Session management, naming and AI assistance: analysis and plan

> **Point-in-time document.** This is an analysis of main as it stood at the
> commit cited below, not a description of the code today. Every `file:line`
> and every claim about current behaviour should be read against that commit.
> The owner's decisions on the questions it raises are recorded in the
> addendum (sections A.1 to A.8) at the end.

**Date:** 2026-09-11
**Basis:** `origin/main` at `5ee07ee`, which includes #60 (per-session usage and cost) and #63 to #67.
**Method:** read-only code reading plus outside research. Nothing was built or run.

**Citations.** Every `file:line` refers to main at `5ee07ee`:
- `st/` stands for `src-tauri/src/`.
- `fe/` stands for `src/lib/`.

A claim I inferred from reading but did not execute is marked **(read, not run)**.

**The owner's goal:** know at a glance what every session is doing, with up-to-date data, better descriptions (possibly written by AI), more deterministic control, and better reliability.

---

## 0. Executive summary

Fleet already has good bones:
- a single-flight reconcile tick;
- HTTP hooks with per-host tokens;
- `turn_seq`, which gives a deterministic "turn finished" signal;
- tasks, inbox, playbooks, GC and repair;
- per-session cost (#60);
- an event bus that pushes row changes to the UI.

What it lacks is a **model of activity**. Two facts drive most of the gaps:

1. **"What is it doing" comes from a screen scrape.**
   - `current_activity` is the last non-blank line of the captured pane (`capture-pane -S -8`: the whole visible screen plus 8 history lines; `st/service/pane_intel.rs:373`, `st/tmux.rs:96`). In practice that is usually the REPL footer, not the work.
   - `claude_status` comes from `claude agents --json` or a pane heuristic, and reconcile overwrites the deterministic hook status on the next pass (`st/store.rs:2710`).
   - Only three hook events are installed (`st/commands/mcp.rs:404`), and the hook body drops almost every field (`st/mcp/hooks.rs:37`).
2. **Labels are best-effort and unowned.** `friendly_name` has five writers and no record of which one wrote it:
   - `new_session` derives one;
   - a startup backfill;
   - the first five words of the first `send_prompt`;
   - the in-session skill;
   - the create dialog.

   Nothing records the source, time or history. The in-session skill fires only when the model obeys it, costs three tool calls in the working session's context, and never runs on unprovisioned hosts. Renaming a session in the sidebar renames the tmux session, which re-creates the row and loses its label, timeline, tags and usage (read, not run).

**Recommendation.** Build three layers, strictly in this order:

1. **Deterministic activity.**
   - Install the full hook set that matters: SessionStart, SessionEnd, PermissionRequest, Notification, Pre/PostCompact, SubagentStart/Stop, StopFailure.
   - Tail the transcript for "last tool / last reply".
   - Store `activity_*` plus `status_source` / `status_at` on every row, and show age and source in the UI.
   - Bind sessions deterministically with SessionStart plus a `TMUX_PANE` header.
2. **Owned labels.**
   - Keep the row id as identity and make rename happen in place.
   - Add `label_source`, `label_set_at`, `label_pinned` and a small label history.
   - Precedence: user > agent > AI > Claude's own title > prompt > branch, with anti-flap rules.
3. **Opt-in AI summaries.**
   - Output: title, doing-now, rolling summary, goal, blocker, next step.
   - Written by Haiku, triggered on `Stop`, debounced, and budget-capped.
   - Runs by default on the host through `claude -p`, so transcripts never leave it; the API on the Mac is optional.
   - Spend is accounted beside #60's `usage_daily`.

   Triage ranking, loop detection and cost anomalies stay **deterministic rules**. AI only explains or suggests. Anything that acts goes through the existing confirm gate.

**Deterministic control.** Add request ids, a delivery acknowledgement (a `UserPromptSubmit` hook that carries a hash of the prompt), a per-session prompt queue drained on `Stop`, and per-session send locks.

---

## 1. What exists today

### 1.1 Identity and naming

| Field | Meaning | Written by | Notes |
|---|---|---|---|
| `sessions.id` | fleet row id, the MCP `session_id` | insert | Real identity for MCP (MCP-6). Not stable across tmux rename (see below). |
| `tmux_name` + `host_alias` | the upsert conflict key | reconcile, `new_session` | `ON CONFLICT(host_alias, tmux_name)` (`st/store.rs:2662` onward). |
| `claude_session_id` | the Claude Code session UUID | `new_session` (`st/service/sessions.rs:1895`), reconcile from `claude agents --json` (`st/service/sessions.rs:419`), `move_session` | Hooks bind by it (`st/service/hooks.rs:85`). |
| `friendly_name` | display label (migration 016) | five writers, listed below | No source, no timestamp, no history. |
| `worktree_key`, branch | derived from the path | reconcile | The quick switcher's description shows branch, project and host (`fe/quick_switcher.ts:61`). |

**The writers of `friendly_name`:**
1. `new_session` → `derive_friendly_name` (`st/service/sessions.rs:1921`, called at `:1873`). Uses the explicit value, else the humanised branch or worktree name.
2. The startup backfill `Store::backfill_friendly_names` (`st/store.rs:2305`), for rows where it is NULL. It skips bg rows.
3. `send_prompt` → `record_prompt_outcome` (`st/service/sessions.rs:2400`).
   - It replaces a NULL or *still-default* label with the first five words of the prompt (`friendly_name_from_prompt`, `:2377`). "Still default" is checked with `default_friendly_name` (`st/store.rs:2349`).
   - **Bug (read, not run):** MCP `send_prompt` and `run_prompt` apply the untrusted-content marker *before* delivery (`st/mcp/tools.rs:2187`). `mark_untrusted` puts the marker line first (`st/mcp/guard.rs:405`). So a session's first MCP-sent prompt gets a label made of the marker words ("claudefleet message from …"), and `last_prompt` / the sidebar meta preview show the marker line.
4. The in-session **fleet-friendly-name** skill (`skills/fleet-friendly-name/SKILL.md`), installed by `provision_one` (`st/service/provision.rs:50`), plus a managed CLAUDE.md block.
   - **When it fires:** the first prompt, the first prompt after `/clear`, and a heartbeat "every ~10 prompts" (`SKILL.md:13-29`).
   - **Cost:** three tool calls per fire (Bash `tmux display`, then `list_sessions`, then `set_friendly_name`).
5. The create dialog (`fe/NewSessionDialog.svelte:356`). **No other UI surface can edit the label.** Double-clicking a sidebar row calls `rename_session`, which renames tmux (`fe/Sidebar.svelte:438-463`).

**How the skill label goes stale:**
- It depends on the model deciding to run the skill and on the model counting prompts.
- It never runs if the host is not provisioned, the MCP token is missing, the session isn't Claude, or the session is blocked or stuck. A blocked session is exactly the one whose label matters.
- It cannot see from outside what the session did.
- It spends the working session's context.
- Last writer wins against writers 1 and 3.
- `list_sessions` summary rows do not even include `friendly_name` (`st/mcp/tools.rs:491`). An orchestrating agent sees only tmux names unless it asks for `summary:false`.

**`rename_session`** (`st/service/sessions.rs:2140`) runs `tmux rename-session` and then `reconcile_one_host`. The store has no row rename. So by reading (not run):
- The next pass ghosts the old `(host, old_name)` row, which a later pass hard-deletes.
- It inserts a new row under `new_name`.
- The new row has a new id and loses `friendly_name`, `tags`, `usage_*` totals, `parent_session_id`, the timeline and the inbox.
- The frontend patches around this for UI prefs only (`migrateSessionUi`, `fe/Sidebar.svelte:459`).

**Generated names** (`st/service/names.rs`, `fe/names.ts`, spec `docs/specs/2026-09-11-session-picker-and-names-design.md`):
- `adjective-noun` pairs, with retry-then-`-N` on collision.
- Used for tmux suffixes and worktree slugs.
- Deliberately not AI.

**QuickSwitcher** (`fe/QuickSwitcher.svelte`, `fe/quick_switcher.ts:61`):
- Each row shows `friendly_name || tmux_name`, a description of `project · host · branch`, and meta = `claude_status ?? status` (`:50`).
- Fuzzy search covers those fields only, not activity or goal.
- The empty-query order is MRU.

**Sidebar row** (`fe/Sidebar.svelte:655-800`):
- **Name and badges:** a status dot, host badge, label (tmux name as the secondary line), and kind badges.
- **Meta line:** time since start and a 48-character `last_prompt` preview (`rowMeta`, `:655`).
- **Status chips:** a stuck chip or a `claude_status` chip; `current_activity` appears **only in the chip's tooltip** (`:772-787`).
- **Context and cost:** a context bar and a cost badge (#60).
- **No freshness or age indicator at all.**

### 1.2 Status signals and their freshness

**The reconcile tick** (`st/lib.rs:160-226`):
- **Cadence:** every `reconcile.interval_secs` (default 20 s, `st/service/sessions.rs:33`). The interval is **read once at startup**, so changing it needs a restart.
- **Missed ticks** are skipped.
- **Each tick:**
  1. a full reconcile, through `ReconcileGate` (`st/service/sessions.rs:230`, single-flight process-wide);
  2. stuck playbooks (`:199`);
  3. GC (`:203`);
  4. the usage collection spawn, every 300 s (`st/service/usage.rs:703`);
  5. the task sweep (`:214`);
  6. repair-on-tick (`:224`).

**Per-host probe** (`probe_with_timeout`, `st/service/sessions.rs:659`), bounded by `HOST_PROBE_TIMEOUT` 30 s (`:28`):
- `tmux list-sessions`;
- `claude agents --json` (`st/tmux.rs:308`);
- then **one `capture-pane -S -8` round-trip per session, sequentially** (`capture_pane_intel`, `:308-328`). That captures the whole visible screen plus 8 history lines; the comment at `st/service/sessions.rs:12-15` wrongly calls it an 8-line tail;
- then a PR probe, batched at 12 per host with a 300 s TTL (`st/service/outcome.rs:18`).

**Cache-first list.** `list_sessions` serves stored rows and reconciles only when the last completed pass is older than the interval (`list_sessions_with`, `st/service/sessions.rs:848`). `refresh_sessions` forces a pass (`:1224`).

**Merge rules** (`upsert_session_in_tx`, `st/store.rs:2662`):
- **`claude_status` = agents status, else the pane-derived status, else the prior value.** A hook-stamped status survives only if `last_hook_at >= this pass's probe start` (`NEW_STATUS`, `:2710`). A pass that *starts* after the hook overwrites it with the pane or agents guess.
- **`current_activity` and `context_pct` are COALESCE-preserved** (`:2739`). A failed capture leaves the old text in place indefinitely, with no timestamp.
- **`stuck_kind`** is authoritative when the pane was observed.

**Hooks installed** (`FLEET_HOOK_EVENTS`, `st/commands/mcp.rs:404`):
- `Stop`, `UserPromptSubmit`, and `PostToolUse` matched to `EnterWorktree|ExitWorktree`.
- They are HTTP hooks with a bearer token and a timeout (`hook_entry`, `:361`).
- Remote hosts get them from `provision_one` step 4 (`st/service/provision.rs:91`).
- The local host gets them **only** via the Settings button (`install_fleet_hook`, `st/commands/mcp.rs:508`, `fe/SettingsDialog.svelte:389`).

**Hook handling:**
- **Payload.** `HookPayload` keeps only `session_id`, `hook_event_name`, `tool_name`, `tool_input`, `tool_response`, `cwd` and `transcript_path` (`st/mcp/hooks.rs:37`). It drops `prompt`, `prompt_id`, `last_assistant_message`, `notification_type`, `source`, `reason`, `error` and `agent_id`.
- **Dispatch.** Unknown events are ignored (`st/service/hooks.rs:48`).
- **`Stop`** (`st/service/hooks.rs:117`, `record_stop_hook` at `st/store.rs:3159`):
  - sets `idle`, `turn_seq+1`, `last_stop_at`, `last_turn_at` and `last_hook_at`;
  - spawns the safe-kill marker scan and the task marker scan.
- **`UserPromptSubmit`** sets `working` and clears `idle_since` (`st/store.rs:3187`).
- **Hook writes do not append to `session_events`.** The timeline only gets `status_change` when *reconcile* sees a diff against its own prior value (`st/service/sessions.rs:448-468`). A hook-driven flip that happened in between leaves no trace.
- **Binding.** A hook is matched to a row only by `claude_session_id` (`host_checked_row`, `st/service/hooks.rs:85`). No row gives `Ok(())`, a silent drop.

**Status vocabularies** (`st/service/pane_intel.rs:24`, `:94`):
- `claude_status`: `working | blocked | completed | failed | stopped | idle`.
- `stuck_kind`: `auth_menu | reconnect | trust_prompt | oom | press_enter`.
- **Pane rules** (`derive_status`, `:424`):
  - `esc to interrupt` means working;
  - `? for shortcuts`, `% used`, `bypass permissions`, `shift+tab to cycle`, `enter to select` or `esc to cancel` mean **idle**;
  - a stuck match means blocked.
- There is no pane rule for a **permission prompt**. A permission dialog's footer text (select or cancel) likely matches the idle cues, so an unhooked session waiting on permission reads **idle** (read, not run; needs a real pane fixture).
- **`claude agents --json` parsing** reads only `sessionId`, `name`, `status` and `cwd` (`st/claude_agents.rs:7`). Status values outside the vocabulary are dropped with a log line (`known_agent_status`, `st/service/sessions.rs:3049`). Current docs describe `status: busy|waiting|idle` plus `state` and `waitingFor` (research §2.3). If that is what hosts emit, `busy` and `waiting` are discarded and the pane guess wins. **Verify against a live host.**

**Other observation surfaces:**
- **`capture_session` / `peek_session`:** a raw screen, and `claude logs` for bg sessions.
- **`session_transcript`:** JSONL, rendered as text plus `[tool_use]` lines, with no tool results (`parse_turns`, `st/service/transcript.rs:167`).
- **`peer_status`:** `claude_status`, `current_activity`, `stuck_kind` and `context_pct`, with **no timestamps** (`st/service/messages.rs:232`).
- **`session_events` timeline** (migration 013):
  - capped at 500 rows per session (`st/store.rs:561`);
  - kinds: status_change, prompt_sent, stuck, killed, recreated, messages, safe-kill, task, mcp_call, workspace repair;
  - **not rendered anywhere in the UI.** Only `Attention.svelte` mentions it, in a comment.
- **Triage** (`fe/attention.ts:113`, `:131`): rule-based — stuck > safe_kill > ghost > failed > idle ≥ 30 min. Severity sorts projects. There is no "blocked on permission" reason, because `blocked` is rarely produced.
- **Tags:** `set_session_tags`, filterable in `list_sessions`.
- **Usage and cost (#60)** (`st/service/usage.rs`, migration 025):
  - an incremental transcript tail every 300 s;
  - per-model price table; `usage_daily` per host and day;
  - `usage_report` and the `fleet_health.usage_*` fields;
  - subagent transcripts are not counted.
- **Tasks** (`st/service/tasks.rs`):
  - `dispatch_task` appends a `FLEET_TASK_DONE_<nonce>` instruction;
  - on Stop, fleet scans the transcript, then the pane (`:566-615`), because "the JSONL can flush AFTER Stop" (`:584`);
  - `wait_for_*` polls the DB every 500 ms (`:27`).
- **Messages:** a persisted inbox, with optional pane delivery through the same send-keys path.

### 1.3 Control

**`send_prompt`** (`build_send_commands`, `st/service/sessions.rs:2302`; `send_prompt_inner`, `:2330`):
- **Mechanics:** `tmux send-keys -l <body>`, `sleep 0.15`, `send-keys Enter`, run as one `&&` script with a 10 s SSH timeout.
- **Returns** `turn_seq_before` (`deliver_prompt`, `st/mcp/tools.rs:1380`).
- **Missing:**
  - no readiness check;
  - no acknowledgement that Claude accepted the prompt;
  - no request id, so an MCP client retrying after a timeout double-sends;
  - no per-session lock, so concurrent `send_prompt`, `send_message{deliver}`, the playbook `press_enter` and the safe-kill prompt can interleave keystrokes.

**Other steering and orchestration tools:**
- **`broadcast_prompt`:** fans out over `claude_status` / host / project filters. It is rate-limited and confirm-gated.
- **`run_prompt`** (`st/mcp/tools.rs:2692`):
  - refuses unless `claude_status` is idle, completed or stopped (`run_prompt_ready`, `:251`);
  - then does send → `wait_for_session{TurnGt}` → transcript;
  - at most 8 long polls per caller (`:1469`).
- **Playbooks** (`st/service/playbooks.rs:75`): opt-in; press_enter sends Enter, oom recreates at most once an hour, other kinds just notify; once per stuck episode.
- **GC** (`st/service/gc.rs:96`): opt-in per-kind TTLs on `idle_since`; dirty worktrees go through safe-kill.
- **Confirm gate** (`st/mcp/guard.rs:76`): broadcast, kill, delete_worktree, set_clipboard, repair_session, cancel_task and move_session. Nonces are bound to the arguments, with a 10 min TTL.
- **Repair** (`st/service/repair.rs` header):
  - probe → plan → apply → verify;
  - the Auto policy only creates things;
  - self-unregister needs the parent `dev:inode` fingerprint (migration 023);
  - on the tick it is opt-in with backoff (`st/service/repair_tick.rs:51`).
- **`move_session`** (#57) carries the usage cursors (#60).
- **`safe_kill`:** marker scan of 3000 pane lines on Stop (`st/service/safe_kill.rs:28`, `:451`).

### 1.4 Reliability

- **SSH** (`st/ssh.rs`):
  - `ControlMaster=auto` with `ControlPersist=10m`, `ConnectTimeout` and `ServerAliveInterval=5` × 2 (`:140-164`);
  - a wall clock of max(3× connect timeout, 30 s) (`:33`);
  - a master reset only when no other command is in flight and `-O check` fails (`:426`);
  - the reset count appears in diagnostics.
- **Ghosts** (`st/store.rs:2815-2833`):
  - live rows missing from a reachable probe are ghosted, subject to the `last_reconciled_at` stale-probe guard;
  - rows **already** ghost and still missing are **hard-deleted** on the next pass, together with their timeline and inbox;
  - so a ghost lives **exactly one reachable pass**, about 20 s by default (test `reconcile_ghosts_sessions_on_first_empty_probe_then_deletes_on_second`, `st/store.rs:6766`);
  - an unreachable host keeps its rows (`:2962`).
  - Consequences: the sidebar's "recreate ghost" affordance has a tiny window, and a tmux server restart erases every session's history unless the user acts within one tick.
- **`fleet_health.ghosts` is always 0.** It counts `claude_status == "ghost"` (`st/service/health.rs:70`), but ghost is a `status` value, not a `claude_status`.
- **Migrations:** a contiguous `MIGRATIONS` table (001 to 025) run in a transaction with an FK check. #60 made 025 re-runnable.
- **Diagnostics:** version, schema, host probes, tunnels, master resets and a log tail with secrets redacted (`st/service/diagnostics.rs`). It has **no hook-health data**: no last hook per host, no hook error count, no binding coverage.
- **Tests:**
  - `FakeSsh` (`st/ssh_fake.rs`) is a scripted, recording `SshExec` with wall-clock and cancellation semantics;
  - `st/service/reconcile_tests.rs` drives the real reconcile through an injected `TmuxExec`;
  - `st/fleet_e2e_tests.rs`.

### 1.5 Where data is stale, wrong or missing

| # | Gap | Severity | Evidence |
|---|---|---|---|
| D1 | **"Doing now" is usually the REPL footer.** `current_activity` is the last non-decoration line of the captured screen (visible pane plus 8 history lines), shown only in a tooltip, with no timestamp, and kept on capture failure. | High | `st/service/pane_intel.rs:373`, `st/store.rs:2739`, `fe/Sidebar.svelte:772-787` |
| D2 | **The hook status is overwritten by a heuristic on the next pass**, whose probe starts after the hook. A deterministic `working` can flip to a pane-guessed `idle` or `blocked` within ≤ 20 s, and back. | High | `st/store.rs:2710` |
| D3 | **Permission waits are not detected** on unhooked sessions, and probably read as idle. No hook covers PermissionRequest or Notification. | High | `st/service/pane_intel.rs:424-456`, `st/commands/mcp.rs:404` |
| D4 | **Hooks silently drop after `/clear`, `/resume` or a fork** (a new Claude session id) until reconcile rebinds through `claude agents`. They never rebind if two agents share a cwd. | High | `st/service/hooks.rs:85-103`, `st/claude_agents.rs:47-75` |
| D5 | **Unprovisioned hosts** (including `local` until the Settings button is pressed) get no hooks. `turn_seq` never moves, so `run_prompt` and `wait_for_session{turn_gt}` time out, and status is pane-only. Nothing in the UI says a host is unhooked. | High | `SKILL.md:130`, `st/commands/mcp.rs:508` |
| D6 | **Freshness is invisible.** `last_reconciled_at` is written but not in `SESSION_COLUMNS`, so neither the UI nor MCP sees it. The "gray out stale rows" promise in migration 014 was never built. `last_hook_at` and `transcript_path` are not on the wire either. | High | `st/store.rs:189-198`, `st/store.rs:3031`, `migrations/014_last_reconciled_at.sql:3` |
| D7 | **The label has no source, time or history.** Five writers; the skill is model-dependent and 3 calls per fire; the UI cannot edit it. | High | §1.1 |
| D8 | **MCP-sent prompts give marker-text labels** and a marker `last_prompt` (read, not run). | Medium | `st/mcp/tools.rs:2187`, `st/mcp/guard.rs:405`, `st/service/sessions.rs:2400` |
| D9 | **Tmux rename re-creates the row**, losing the label, timeline, tags, usage and inbox (read, not run). | Medium | `st/service/sessions.rs:2140-2166` |
| D10 | **`list_sessions` summary lacks `friendly_name`**, and every surface lacks timestamps. | Medium | `st/mcp/tools.rs:491` |
| D11 | **The timeline misses hook transitions** (turns, prompts accepted, permission waits, compaction) and is not shown in the UI. | Medium | `st/service/sessions.rs:448-468`, `fe/*` |
| D12 | **`claude agents` status values outside the vocabulary are discarded.** `state` and `waitingFor` are ignored (verify on a host). | Medium | `st/claude_agents.rs:7`, `st/service/sessions.rs:3049` |
| D13 | **`send_prompt` has no ack, no idempotency, no lock and no queue.** | Medium | §1.3 |
| D14 | **A ghost survives one pass**; the history is deleted with it. | Medium | `st/store.rs:2815`, `:6766` |
| D15 | **`fleet_health.ghosts` is always 0.** | Low (bug) | `st/service/health.rs:70` |
| D16 | **Pane capture costs one SSH round-trip per session per tick**, sequential inside the 30 s probe budget. With 60 sessions on one host that is the dominant cost of a pass. | Medium | `st/service/sessions.rs:308-328` |
| D17 | **The reconcile interval needs an app restart to change.** | Low | `st/lib.rs:160-170` |
| D18 | **Cost excludes subagents**, and lags up to 300 s. | Low | #60 body; `st/service/usage.rs` |
| D19 | **No hook-health telemetry**: no per-host last hook, no failures, no binding coverage. | Medium | `st/service/diagnostics.rs` |
| D20 | **`context_pct` is parsed from footer text.** It is exact in Claude's status-line JSON, which fleet does not read. | Low | `st/service/pane_intel.rs:258` |

---

## 2. Outside research

This section is a summary; the URLs are in §6. Field names come from the raw hooks doc (the summariser garbled some of them). Anthropic states that the transcript JSONL format is internal and changes between releases.

### 2.1 Claude Code hook events fleet does not use yet

The docs define 30+ events ([hooks](https://code.claude.com/docs/en/hooks)). Every event gets `session_id`, `prompt_id`, `transcript_path`, `cwd`, `permission_mode` and `hook_event_name`, plus `agent_id` and `agent_type` inside subagents. The ones that matter here:

| Event | Fires | Useful fields | Fleet use |
|---|---|---|---|
| SessionStart | startup, resume, clear, compact, fork (`source`) | `source`, `model`, `context_tokens` | **Rebind** `claude_session_id` immediately after `/clear` or `/resume` (D4); reset the label to "new task"; show the model. |
| SessionEnd | `reason`: clear, resume, logout, prompt_input_exit, other | `reason` | Mark the process ended (distinct from a tmux ghost). |
| UserPromptSubmit | before a prompt is processed | **`prompt`**, `prompt_id` | **Delivery ack** by matching the prompt hash; label seed; timeline entry. |
| PermissionRequest | *the instant* a permission prompt shows | `tool_name`, `tool_input`, suggestions | **`blocked` with `waiting_for=permission`**, shown with the tool and target. The docs recommend it over the ~6 s-delayed Notification. |
| Notification | `permission_prompt` (~6 s), `idle_prompt` (~60 s after Stop), elicitation types, `agent_needs_input` | `notification_type`, `message`, `title` | A backstop for older CLIs; "waiting for input" with the message text. |
| Stop | turn done | **`last_assistant_message`**, `background_tasks` | A free "what it just said" summary; `turn_done` timeline entry. |
| StopFailure | turn ended on an API error | `error`: rate_limit, overloaded, auth, billing, max_output_tokens, … | **`failed` or `errored` with a reason**; triage. |
| PreToolUse / PostToolUse / PostToolUseFailure | per tool call | `tool_name`, `tool_input`, `tool_use_id`, `duration_ms`, `error` | Tool-level activity ("Edit src/store.rs", "Bash: cargo test") and deterministic loop detection. |
| SubagentStart / SubagentStop | subagent lifecycle | `agent_id`, `agent_type`, `last_assistant_message` | "3 subagents running"; subagent cost follow-up (D18). |
| PreCompact / PostCompact | compaction | `trigger` (manual or auto) | "Compacting"; compaction-thrash detection; context reset. |

**Hook mechanics that matter:**
- **Handler types:** `command`, `http`, `mcp_tool`, `prompt` and `agent`.
- **HTTP hook failures don't block:** a non-2xx or a connection failure is a non-blocking error.
- **Header env vars:** HTTP headers can interpolate `$VAR`, but only for variables listed in `allowedEnvVars`. This is the basis of the pane-id binding in P3.
- **Async:** command hooks support `async`.
- **Transcript lag:** a hook can fire before the transcript line is flushed, which is why `Stop` carries `last_assistant_message`.

### 2.2 Status-line JSON

([statusline](https://code.claude.com/docs/en/statusline))

**Fields fed on stdin:**
- `session_name`: the custom name, else Claude's **AI-generated title**;
- `model`;
- `cost.total_cost_usd`, `total_duration_ms` and lines added / removed;
- `context_window.used_percentage`, which is exact and replaces footer parsing (D20);
- `rate_limits.*`;
- `pr.*` and `worktree.*`.

**Refresh:** event-driven with a 300 ms debounce, plus an optional `refreshInterval`.

Only one status line exists per user, so fleet could only use it by *wrapping* the user's own command.

### 2.3 Claude Code's own "agent view", the closest analogue

([agent-view](https://code.claude.com/docs/en/agent-view))

- **States:** Working, Needs input, Idle, Completed, Failed, Stopped.
- **`claude agents --json` fields:**
  - `state`: working, blocked, done, failed, stopped;
  - `status` while the process is alive: busy, waiting, idle;
  - `waitingFor`: permission prompt, input needed, …
- **Anti-flap summary rules worth copying:**
  - the row text updates at most every 15 s from the session's own output, **with no model call**;
  - a Haiku summary is written **at the end of each turn**;
  - it is refreshed every few minutes during long turns;
  - a blocked row shows the question.

Claude Code also titles sessions itself with a "background request to the small/fast model" after the first prompt and after a plan is accepted. A user name set with `--name` or `/rename` overrides that title ([sessions](https://code.claude.com/docs/en/sessions)). If fleet starts Claude with `--name <tmux_name>` (a comment at `st/service/sessions.rs:381` suggests it does), fleet suppresses that free AI title. **Verify.**

### 2.4 Headless mode, SDK and OpenTelemetry

**Headless mode** ([headless](https://code.claude.com/docs/en/headless), [cli-reference](https://code.claude.com/docs/en/cli-reference)):
- **Flags:** `claude -p --model haiku --output-format json|stream-json`, `--no-session-persistence`, `--max-turns`, `--max-budget-usd` and `--json-schema`.
- **`--bare`** skips hooks, skills, MCP and CLAUDE.md, but needs `ANTHROPIC_API_KEY`.
- **Cost:** the `result` event carries `total_cost_usd` and `usage`. `-p` sessions don't appear in the picker.

**Pricing** ([pricing](https://platform.claude.com/docs/en/about-claude/pricing)):
- Haiku 4.5 (`claude-haiku-4-5`) costs $1 per million input tokens and $5 per million output, and cache reads are 0.1×.
- A 3k-in / 150-out summary is about **$0.004** through the API.
- Retirement is "not sooner than 2026-10-15", so model ids must be configurable.

**Agent SDK** ([sessions](https://code.claude.com/docs/en/agent-sdk/sessions)): `listSessions()` returns `summary`, `customTitle` and `firstPrompt`.

**OpenTelemetry** ([monitoring](https://code.claude.com/docs/en/monitoring-usage)):
- metrics such as `claude_code.cost.usage` and `token.usage`;
- events `tool_result` and `api_request`, keyed by `session.id`.

This is an alternative push channel, but it needs a collector on every host. It is not recommended now.

### 2.5 Comparable tools

| Tool | Status and activity | Naming | Control and reliability |
|---|---|---|---|
| Claude Squad | Pane SHA-256 diff plus prompt-string matching. States: Running, Ready, Loading, Paused. | User title, fixed | Pause commits and removes the worktree; auto-yes. |
| ccmanager | Per-CLI detectors for idle, busy and waiting_input; **status hooks on every transition** | worktree | Haiku-judged auto-approval, which is experimental. |
| Conductor | Workspace per task, diff-centric | City names plus branch or PR title | Archive and restore history. |
| Claude agent view | Hooks and process state, two axes (state and status) plus `waitingFor` | AI title from Haiku; user name wins | attach, stop, respawn; anti-flap rules. |
| Copilot coding agent | Session logs with reasoning and tool use, plus tokens | PR title | Steer a running session; commit → session-log link. |
| Devin / Cursor cloud | Explicit enums: working, blocked, finished… / CREATING, RUNNING, FINISHED, ERROR… | title | API follow-ups and cancel. |
| sesh / zellij managers | none | Deterministic repo or dir names | frecency picker |
| Langfuse, LangSmith, AgentOps | Session → trace waterfall of LLM calls, tools and errors, plus cost | `sessionId` / `thread_id` | Scores and replay. |

Crystal and Vibe Kanban are deprecated or shut down (2026).

### 2.6 Patterns to adopt

1. Use a two-axis state: lifecycle (working, blocked, done, failed, stopped) × process (busy, waiting, idle) plus `waiting_for`.
2. Derive state from hook edges, and keep pane heuristics only as a labelled fallback.
3. Build an attention queue that shows *the question being asked*.
4. Summaries should use the cheap deterministic text continuously, the model only at turn end, and a slow refresh during long turns.
5. Detect loops with fingerprints of `(tool_name, hash(tool_input))`: 3 identical in a row, repeated failures with the same error, or retry storms.
6. Keep a stable id with a display label on top, where the user-set label always wins.
7. Every value carries its source and its age.

---

## 3. Proposals

**Principles:**
- Deterministic first, AI second.
- Every displayed value has a `*_source` and a `*_at`.
- AI output is advisory and never acts. Actions go through existing tools and the confirm gate.
- Migrations continue from **026**.

### (a) Always-fresh, deterministic status

#### P1. Install the full hook set and keep the payload fields
- **Problem:** D2, D3, D4, D11, D19. Fleet sees only turn start and turn end.
- **Design:**
  - Extend `FLEET_HOOK_EVENTS` with SessionStart, SessionEnd, PermissionRequest, Notification, StopFailure, PreCompact, PostCompact, SubagentStart and SubagentStop. All of these are low-rate and use `http` with the existing short timeout.
  - Add `prompt`, `prompt_id`, `last_assistant_message`, `notification_type`, `message`, `source`, `reason`, `error`, `agent_id`, `agent_type` and `trigger` to `HookPayload`. Keep `deny_unknown_fields` off.
  - `apply_hook` dispatches every event through `record_activity()` (P2), which updates the row and appends one `session_events` entry per edge. New kinds: `session_start`, `session_end`, `turn_started`, `turn_done`, `permission_wait`, `notification`, `stop_failure`, `compact`, `subagent_start`, `subagent_stop`.
  - Add a per-host `hosts.last_hook_at` and a hook error counter for diagnostics.
  - `provision_hosts` reinstalls the new set. Fleet reads a version tag in the fleet hook block (`"fleet_hooks": 2`) and flags hosts that have an older block.
  - **Do not** add PreToolUse or PostToolUse as synchronous HTTP (see P4).
- **Files:** `st/commands/mcp.rs` (hook section), `st/service/provision.rs` (hook step), `st/mcp/hooks.rs`, `st/service/hooks.rs`, `st/store.rs` (hook recorders), `st/service/diagnostics.rs`, migration 026.
- **Effort:** M.
- **Risk:**
  - More hook traffic, though all of it is low-rate.
  - Older Claude CLIs may lack some events (harmless: they never fire).
  - A PermissionRequest hook returning an empty 2xx must not be read as a decision. The docs say an empty body is success with no decision; confirm this on the minimum CLI version.
- **Dependencies:** none.

#### P2. Activity model on the row, with source and age
- **Problem:** D1, D2, D6. The fields are too thin to answer "what is it doing" or "since when".
- **Design:** migration 026 adds these columns to `sessions`:
  - `activity_kind`: prompt, thinking, tool, permission, input, compacting, subagents, idle, error or ended;
  - `activity_detail`: ≤ 120 characters, e.g. `Edit st/store.rs`, `Bash: cargo test`, `waiting: allow Bash(rm …)?`;
  - `activity_at` and `activity_source`: hook, transcript, agents or pane;
  - `status_source` and `status_at`;
  - `waiting_for`: permission, question, elicitation or NULL;
  - `last_error`: a StopFailure type;
  - `last_reply`: the first 280 characters of `Stop.last_assistant_message`;
  - `subagents_active`;
  - `compactions`.

  All of them go on the wire (`SESSION_COLUMNS`, `fe/sessions.ts`), together with the existing `last_reconciled_at` and `last_hook_at`. Replace `NEW_STATUS` with an explicit precedence:
  1. A hook-sourced status stands until the next hook edge. Reconcile may override it only if:
     - (a) the pane shows a `stuck_kind`, or
     - (b) the status is `working`, no hook has arrived for `status.hook_trust_secs` (default 900), and the pane shows idle chrome. It is then stored as `status_source='pane'`, which covers missed hooks.
  2. Agents status comes before the pane.
  3. The pane is used only when nothing better exists.
- **Files:** migration 026, `st/store.rs` (`upsert_session_in_tx`, the SessionRow mapper), `st/service/sessions.rs` (reconcile write), `st/service/pane_intel.rs` (a `Permission` cue, P6), `fe/sessions.ts`.
- **Effort:** M.
- **Risk:**
  - The merge SQL is delicate. Extend the MCP-1 guard tests into a race matrix (P26).
- **Dependencies:** P1 for the hook fields. The transcript source comes from P4.

#### P3. Deterministic session binding: SessionStart plus the tmux pane id
- **Problem:** D4. The binding relies on `claude agents` name or cwd matching and breaks after `/clear`, `/resume`, a fork, or a shared cwd.
- **Design:**
  - Add a header to the fleet HTTP hook entry: `"headers": {"Authorization": "...", "X-Fleet-Pane": "$TMUX_PANE"}` with `"allowedEnvVars": ["TMUX_PANE"]`. Every pane process inherits `TMUX_PANE` (`%17`), so this works for **all** tmux sessions, fleet-created or discovered.
  - Reconcile stores `tmux_pane_id` per row from `tmux list-panes -a -F '#{session_name} #{pane_id}'`. This folds into the batched probe (P24).
  - The hook handler resolves the row by `(caller host, pane id)` first, then by `claude_session_id`.
  - On `SessionStart`, it (re)writes `claude_session_id` and `transcript_path`. For `source ∈ {clear, startup}`, it resets the task-scoped fields: activity, AI summary, a non-pinned label (P10) and `turn_seq_at_session_start`.
  - Keep the `claude agents` match as a fallback for older CLIs.
- **Files:** `st/commands/mcp.rs` (`hook_entry`), `st/mcp/hooks.rs` (read the header), `st/service/hooks.rs`, `st/tmux.rs` (pane ids), `st/store.rs`, migration 026.
- **Effort:** M.
- **Risk:**
  - Header interpolation and `allowedEnvVars` are version-dependent. Detect support by whether the header arrives non-empty, and fall back.
  - Pane ids are per tmux server, so a tmux server restart invalidates them. Reconcile refreshes them every pass.
- **Dependencies:** P1.

#### P4. "Doing now" from the transcript tail, plus an optional per-tool spool
- **Problem:** D1 on every host, including unprovisioned ones.
- **Design, phase A (no new hooks):**
  - Each tick, one batched script per host reads the last ~64 KiB of each live session's transcript (`transcript_path` or the cwd-derived one).
  - It emits, per session: the last entry's type and timestamp; the last `tool_use` name plus a summarised input; whether that tool_use has a matching `tool_result` yet (unmatched means running or awaiting permission); and the first line of the last assistant text.
  - Reuse `transcript::summarize_tool_use` semantics and the jq-free awk approach of `usage.rs`, so only short strings leave the host.
  - Stored as `activity_*` with `activity_source='transcript'`.
  - The format is internal to Claude, so every parser returns `None` on anything unexpected, as `pane_intel` does.
- **Design, phase B (opt-in, per-tool fidelity):** an **async `command` hook** for PreToolUse and PostToolUse that appends one line to a host-local spool `~/.claude/fleet/events.jsonl` (0600, rotated at 16 MiB). It needs no network and no token. The tick drains it by byte offset, like #60's cursor.
  - **Why not HTTP:** the reverse tunnel is served from the Mac. While the Mac sleeps, a synchronous per-tool HTTP hook would stall *every tool call on every host* for up to the hook timeout.
- **Files:** new `st/service/activity.rs`, `st/service/sessions.rs` (the probe calls it), `st/store.rs`, and for phase B `st/commands/mcp.rs` and `st/service/provision.rs`.
- **Effort:** M (A), M (B).
- **Risk:**
  - Transcript format churn: mitigated by fail-soft parsing and fixture tests from real transcripts.
  - The spool duplicates transcript content: same permissions, rotated, and only projected fields are read.
- **Dependencies:** P2. P24 for batching.

#### P5. Fast lane: react to edges instead of waiting for the tick
- **Problem:** `blocked` and the question text should appear within seconds.
- **Design:**
  - On `PermissionRequest` or `Notification`, spawn a single-session capture of the visible pane, parse the question into `activity_detail`, and emit `session_updated`.
  - On selecting a session in the UI, run `reconcile_session(id)` (one session, one host), gated to at most once every 5 s.
  - The UI already patches rows from events, so no polling is added.
- **Files:** `st/service/hooks.rs`, `st/service/sessions.rs` (a new `reconcile_session`), `st/commands/sessions.rs`, `fe/Sidebar.svelte`.
- **Effort:** S–M.
- **Risk:** low.
- **Dependencies:** P1, P2.

#### P6. Heuristic fixes for hosts without hooks
- **Problem:** D3, D12, D20.
- **Design:**
  - `pane_intel`: detect permission and question dialogs ("Do you want to", numbered choices, "No, and tell Claude …"), yielding `Blocked` with `waiting_for=permission`. Take fixtures from real captures.
  - `claude_agents`: parse `state`, `status` and `waitingFor`, and map busy→working, waiting→blocked, done→completed.
  - Optionally wrap the status line (opt-in): fleet installs a wrapper that appends the JSON (session_name, context %, cost, model) to the spool and then runs the user's original status-line command. This gives an exact context percentage and Claude's own AI title.
- **Files:** `st/service/pane_intel.rs`, `st/claude_agents.rs`, `st/service/sessions.rs` (`known_agent_status`).
- **Effort:** S (first two), M (wrapper).
- **Risk:** a heuristic false positive. Blocked is only advisory; playbooks don't act on it.
- **Dependencies:** none.

### (b) Session descriptions and summaries written by AI

#### P7. Two-layer description: deterministic always, AI when opted in
- **Problem:** the owner wants to "easily know what every session is doing". Today there is a label and a footer line.
- **Layer 0 (deterministic, always on, free):**
  - `doing_now` = `activity_detail`;
  - `last_reply` from Stop;
  - `last_prompt`;
  - `goal_hint` = the first prompt after SessionStart(startup or clear), with the marker stripped;
  - `claude_title` = Claude Code's own AI title when available, from the status-line wrapper, `claude agents` `name`, or the SDK `customTitle`/`summary`. Stop suppressing it with `--name` once P3 no longer needs name matching.
- **Layer 1 (AI, opt-in via `ai.enabled`, per-host allowlist `ai.hosts`):** a `session_summary` object:
  - `title`: 3–6 words;
  - `doing_now`: ≤ 80 characters;
  - `summary`: ≤ 3 sentences, rolling;
  - `goal`;
  - `state`: on_track, waiting_on_user, blocked, done or looping;
  - `blocker`;
  - `next_step`;
  - `confidence`.

  It is produced with a JSON schema (`--json-schema` in `-p`, or tool-forced output through the API).
- **When:**
  - on `Stop`, only if `turn_seq` advanced and at least `ai.min_interval_secs` (default 600) has passed since the last summary for that session;
  - during a long turn, every 15 min while `working`;
  - on explicit "Summarise now" (UI, MCP);
  - on `SessionStart(clear)`, clear the summary and wait for the first Stop;
  - never while the host is unreachable, and never for bg or shell kinds by default.
- **Input:**
  - the previous summary (for the rolling update);
  - `last_prompt` and the prompts since the last summary;
  - `parse_turns` output for the turns since the last summary (assistant text plus `[tool_use]` lines; **no tool results**, which bounds size and leakage), capped at `ai.max_input_chars` (default 12k);
  - the activity and timeline events since the last summary;
  - the branch and `repo_changes` counts, which are cheap and optional.
- **Model:** a `ai.model` setting, default `claude-haiku-4-5`. Haiku retires no sooner than 2026-10-15; the model id is kept in one constant.
- **Cost estimate:**
  - About 4k in / 250 out ≈ **$0.005** per summary via the API.
  - A fleet of 50 sessions averaging 20 debounced summaries a day comes to about $5/day worst case.
  - The default cap `ai.daily_budget_usd = 2` stops generation, and the UI shows "AI paused: budget".
- **Where it runs:** a `Summarizer` trait with two implementations.
  - **`HostCli` (default when enabled):**
    - Over SSH on the session's host, run `claude -p --model haiku --output-format json --no-session-persistence` with tools and MCP disabled (the exact flags must be pinned per CLI version).
    - Fleet pipes the rendered input *on the host*: the script builds the input from the transcript there, so **the transcript never leaves the host**. Only the JSON summary comes back.
    - It uses the host's existing Claude login (subscription), and cost comes from `result.total_cost_usd`.
    - The run's own hooks carry an unbound session id and no `TMUX_PANE`, so fleet ignores them.
  - **`Api` (optional):** reqwest to the Messages API from the Mac, with the key in the OS keychain. The transcript excerpt comes to the Mac (as `session_transcript` already does) and goes to Anthropic. Exact token usage, prompt caching of the system prompt, and batching are possible.
- **Caching:**
  - The key is `(session_id, turn_seq, transcript offset)`: no regeneration without new turns.
  - The last 20 summaries per session go into `session_summaries`.
  - Label changes go into `session_labels` (P11).
- **Privacy:**
  - Off by default, per host.
  - A redaction pass reuses the diagnostics secret scrubber, plus common token regexes.
  - Tool results and file contents are never included.
  - The Settings text says where data goes for each mode.
  - An audit event `summary_generated` records the model, byte count and cost, never the content.
- **Accounting (reuses #60):**
  - A new table `ai_usage_daily(day, host_alias, purpose, calls, input_tokens, output_tokens, cost_micros)`, where purpose is summary, digest or suggest.
  - It is surfaced in `usage_report` as a separate `fleet_ai` block and in `fleet_health`, so AI spend is never confused with session spend.
  - The #60 price table prices API calls; the CLI path uses the reported cost.
- **Fallback when AI is off:** Layer 0 fills every UI slot. The AI-only slots (goal, blocker, next step) are hidden, not left empty.
- **Files:** new `st/service/summary.rs` (trait, both implementations, debounce, budget); migration 027 (`sessions.summary_*`, `session_summaries`, `ai_usage_daily`); `st/service/settings.rs` (`ai.*` rows); `st/service/usage.rs` (report block); `st/mcp/tools.rs` (new read-only `session_summary { session_id, refresh? }`, where `refresh` counts against the budget and is confirm-free); `fe/SessionDetails.svelte`, `fe/SettingsDialog.svelte`. Regenerate the reference docs.
- **Effort:** L (M without the API implementation).
- **Risk:**
  - Cost runaway: the cap and debounce are unit-tested.
  - Prompt injection from transcript content into the summariser. Its output is display-only, and it runs with no tools and no MCP.
  - Subscription rate limits on busy hosts: back off on StopFailure-like errors.
  - A stale summary: the UI shows its age and the turn it covers.
- **Dependencies:** P1 (Stop trigger, `last_reply`), P4 (activity input), P10 (the label precedence it writes into).

#### P8. The friendly-name skill: combine, then demote
- **Recommendation:** do **not** keep the skill as the primary label writer. It is model-dependent, spends context in the working session, can't run when blocked, and doesn't run on unprovisioned hosts. Deterministic fields plus an AI summary computed from outside answer "what is it doing" better and for every session.
- **Keep it as an optional agent-declared channel:**
  - Rewrite it to fire only on the first prompt and after `/clear`. Drop the 10-prompt heartbeat; the AI summariser and P10 anti-flap cover relabels.
  - It calls a new `set_session_goal { goal, label? }`, which writes `label_source='agent'`. Resolution happens server-side from the caller's host token plus `TMUX_PANE`, through an optional `pane_id` argument, so it drops from three calls to one.
  - With AI off, the skill remains the only semantic label source, so it should stay installed.
- **Files:** `skills/fleet-friendly-name/SKILL.md`, `st/service/provision.rs` (CLAUDE.md block text), `st/mcp/tools.rs`.
- **Effort:** S.
- **Risk:** low.
- **Dependencies:** P10, and P3 for the pane lookup.

#### P9. Strip the untrusted marker before deriving labels and `last_prompt` (quick win, D8)
- **Design:** pass the *unmarked* body to `record_prompt_outcome`. Either `deliver_prompt` passes both bodies, or add a `guard::strip_marker`. Add a unit test with an MCP-marked prompt.
- **Files:** `st/service/sessions.rs` (prompt section), `st/mcp/tools.rs` (`deliver_prompt`).
- **Effort:** S.
- **Risk:** low.
- **Dependencies:** none.

### (c) Naming: identity versus label

#### P10. Label ownership, precedence and anti-flap
- **Problem:** D7, D10. Anything overwrites anything.
- **Design:** migration 027 adds `label_source` (user, agent, ai, claude_title, prompt, branch, generated), `label_set_at` and `label_pinned` (0/1). `friendly_name` stays the display column so wire compatibility holds. One store function `propose_label(id, text, source, now) -> Applied | Rejected(reason)` enforces these rules:
  1. **Precedence:** user > agent > ai > claude_title > prompt > branch/generated. A lower source never replaces a higher one, **except** after `SessionStart(clear|startup)`, which demotes non-pinned labels to "stale" so the next source may write.
  2. **User labels are pinned** until the user clears the pin.
  3. **Rate limit:** at most one change per 15 min for non-user sources, and at most 6 a day.
  4. **Semantic hysteresis:** reject an AI or agent change whose token Jaccard with the current label is ≥ 0.5.
  5. **History:** `session_labels(id, session_id, at, label, source)`, capped at 50 per session, plus a `label_changed` timeline event.
- **MCP:** `set_friendly_name` gains `source` (default `agent` for host tokens and `user` for the desktop and master), `pin?`. `list_sessions` summary rows gain `friendly_name`, `label_source`, `activity_kind` and `activity_at`.
- **UI:** double-click edits the **label**. "Rename tmux session" moves to the context menu. A small source glyph appears on the label, and a history popover sits in details.
- **Files:** `st/store.rs` (label functions), `st/service/sessions.rs` (friendly-name and prompt-outcome sections), `st/mcp/tools.rs`, `fe/Sidebar.svelte` (rename UX), `fe/SessionDetails.svelte`, `fe/sessions.ts`, migration 027.
- **Effort:** M.
- **Risk:** UX change to double-click. Mention it in the release notes.
- **Dependencies:** none. P7 and P8 use it.

#### P11. Rename in place: identity survives tmux rename
- **Problem:** D9.
- **Design:** `rename_session` does, in order:
  1. validate;
  2. run `tmux rename-session`;
  3. `Store::rename_session_row(host, old, new)` updates `tmux_name` on the same row id and stamps `last_reconciled_at = now`, so the stale-probe guard (`st/store.rs:2826`) protects it from a pass already in flight;
  4. `reconcile_one_host`.

  Add a reconcile test: rename during an in-flight pass keeps the row id, label, usage and tags, and no ghost appears. Longer term, add a `sessions.uid` (ULID) exposed on the wire as the stable handle across `move_session` too. The id already survives in-place moves; move creates a target row, so the ULID links the two.
- **Files:** `st/service/sessions.rs` (rename), `st/store.rs`, `st/service/reconcile_tests.rs`.
- **Effort:** S (rename), M (uid).
- **Risk:** a race with the tick, covered by the guard and the test.
- **Dependencies:** none.

#### P12. Search and pickers use the new text
- **Design:** the quick switcher fuzzy-searches the label, goal, doing-now and tags. Its description becomes `doing_now · project · host`, and meta becomes `state · age`. An empty query shows a "Needs you" group (P13 order) above MRU.
- **Files:** `fe/quick_switcher.ts`, `fe/QuickSwitcher.svelte`.
- **Effort:** S.
- **Risk:** low.
- **Dependencies:** P2, and optionally P7.

### (d) AI for management beyond summaries

Rule: **deterministic unless a model adds something rules can't.** Nothing here acts without a human.

| Capability | Deterministic or AI | Design |
|---|---|---|
| **P13 Triage ranking** ("who needs me now") | **Deterministic** | Score = permission or question wait (age-weighted) > stuck > StopFailure/failed > **done-unread** (a turn finished since the user last opened the session; needs `last_viewed_at`, set on selection) > safe-kill failed > idle ≥ N > working > idle. A single `fe/attention.ts` `rank()` feeds the sidebar "Needs you" filter, the switcher and `fleet_digest`. AI adds at most a one-line "why" from `blocker`. |
| **P14 Stuck and loop detection** | **Deterministic rules** on the P1/P4 event stream; AI advisory | Rules: ≥ 3 consecutive identical `(tool, hash(input))`; ≥ 3 `PostToolUseFailure` with the same error in 10 min; ≥ 3 compactions an hour (thrash); `working` with no event of any kind for `status.hung_secs` (hung); StopFailure `rate_limit` storms per host; `context_pct ≥ 90` with no compaction. New `stuck_kind` values: `tool_loop`, `hung`, `compact_thrash`, `rate_limited`. The playbooks treat them as **notify-only**. AI: the summariser's `state=looping` is shown as a hint and never triggers a playbook. |
| **P15 Suggested next prompts** | AI, opt-in | Up to three chips in SessionDetails, generated with the summary (same call, no extra cost). A click fills PromptComposer; the user sends. Never auto-sent; the provenance `ai_suggested` goes into the `prompt_sent` event. |
| **P16 Grouping by goal** | Deterministic first (project, branch, `parent_session_id`, tags); AI suggests tags | A weekly or on-demand "suggest tags" pass clusters `goal` strings. The user accepts, which calls `set_session_tags`. |
| **P17 Fleet digest** | Deterministic table plus an optional AI paragraph | MCP `fleet_digest { since_secs }` and a UI panel: per-state counts, the Needs-you list with blockers, sessions that finished since the last digest (`last_reply`), top spend and burn rate, anomalies, hook-health. With AI, one paragraph on top (purpose `digest`, budgeted). |
| **P18 Cost anomaly** | **Deterministic** | Burn rate from #60 usage deltas: `usage_cost_micros` change / elapsed. Flag a session at > 3× the fleet median $/h, or above `usage.alert_usd_per_hour`. Flag a host whose daily spend exceeds 2× its 7-day average from `usage_daily`. Surface in triage and the digest. No model. |

- **Files:**
  - P13: `fe/attention.ts`, `fe/Sidebar.svelte` (filter), and `last_viewed_at` in migration 026.
  - P14: `st/service/pane_intel.rs` (vocabulary), new `st/service/loops.rs`, `st/service/playbooks.rs` (notify mapping), `skills/claude-fleet-control/SKILL.md` (vocabulary test).
  - P15–P17: `st/service/summary.rs`, `st/mcp/tools.rs`, `fe/SessionDetails.svelte`, new `fe/Digest.svelte`.
  - P18: `st/service/usage.rs`, `fe/attention.ts`.
- **Effort:** P13 S, P14 M, P15 S (after P7), P16 M, P17 M, P18 S.
- **Risk:**
  - Adding vocabulary values means the W0.2 enum test and the skill text must change together.
  - Loop rules could false-positive on legitimate polling, for example `sleep` plus a check in a loop. The threshold is configurable and the result is notify-only.

### (e) Deterministic control

#### P19. An explicit session state machine
- **Design:** derive `state` from `(status, claude_status, activity_kind, waiting_for, lost_at)` in one pure function (`st/service/state.rs`):
  - `starting` → `ready` (idle)
  - `ready` ⇄ `busy` (working, subagents)
  - `busy` → `waiting` (permission or question)
  - `waiting` → `busy`
  - `busy` → `failed` (StopFailure)
  - any → `ended` (SessionEnd) → `ghost` → `gone`

  Transitions are validated. Illegal ones, such as `ready → waiting` with no prompt, are logged as a `state_anomaly` event: a free correctness signal for the hook pipeline. Commands declare preconditions against `state`:
  - `run_prompt` needs `ready`, as today;
  - `send_prompt` gains `require_state?: ready|any`, default `any` for compatibility, where `ready` returns `E_INVALID_STATE`;
  - playbooks need `waiting` or stuck.

  The skill documents the table.
- **Files:** new `st/service/state.rs`, `st/mcp/tools.rs`, `skills/claude-fleet-control/SKILL.md`.
- **Effort:** M.
- **Risk:** low if it is derive-only at first.
- **Dependencies:** P2.

#### P20. Idempotent commands with request ids
- **Design:**
  - Add an optional `request_id` (a client UUID, ≤ 64 characters) on `send_prompt`, `run_prompt`, `dispatch_task`, `new_session`, `new_bg_session`, `kill_session`, `safe_kill_session`, `move_session` and `send_message`.
  - A table `mcp_requests(request_id PK, caller, tool, args_hash, state, result_json, created_at)` with a 24 h TTL.
  - The same id with the same args returns the stored result, or `E_IN_PROGRESS`. The same id with different args returns `E_CONFLICT`.
  - The check sits in the MCP layer, before confirm-gate nonce handling. A confirmed retry carries both.
- **Files:** migration 028, `st/mcp/guard.rs` (or new `st/mcp/idempotency.rs`), `st/mcp/tools.rs` (handlers). Regenerate docs.
- **Effort:** M.
- **Risk:** low.
- **Dependencies:** none.

#### P21. Delivery acknowledgement for prompts
- **Design:**
  - `send_prompt` creates a `prompt_deliveries` row with: `id` (the returned `delivery_id`), `session_id`, `body_sha256` over the normalized marked body, `preview`, `state`, `typed_at`, `accepted_at`, `prompt_id`, `completed_turn_seq`, `error` and `origin`.
  - `state` moves `typed → accepted` when a `UserPromptSubmit` hook arrives whose `prompt` hash (or a normalized 200-character prefix) matches, and records its `prompt_id`.
  - It moves `accepted → completed` on the next Stop (`turn_seq`).
  - It becomes `unconfirmed` if nothing matches within 15 s. At that point a fast-lane capture (P5) attaches the pane tail to the row. Typical causes: a permission dialog was open, or the REPL was busy and queued the text.
  - A new read-only tool `prompt_status { delivery_id }`.
  - `run_prompt` waits on `completed` for *its* delivery instead of any `turn_gt`, which removes the "someone else's turn satisfied my wait" race.
  - Unhooked hosts: `accepted` is never reached, and the state says `unverifiable` rather than lying.
- **Files:** migration 028, `st/service/sessions.rs` (prompt section), `st/service/hooks.rs`, `st/service/tasks.rs` (a wait condition on the delivery), `st/mcp/tools.rs`.
- **Effort:** M.
- **Risk:**
  - Claude may rewrite pasted text; for example, large pastes can be collapsed. Match on a normalized prefix plus length, and fall back to `accepted_by_turn` (any prompt_submit after typed).
  - Needs P1's `prompt` field.
- **Dependencies:** P1.

#### P22. A per-session prompt queue, with locks and ordering
- **Design:**
  - Add `prompt_deliveries.state='queued'`.
  - `send_prompt { queue: true }` (and the UI "Queue" button) enqueues. A dispatcher delivers the head when `state=ready`: on the Stop edge immediately, else on the tick. It never types into a busy or waiting REPL.
  - A per-session async lock (`DashMap<i64, tokio::sync::Mutex<()>>` in the service layer) wraps **every** keystroke path: send_prompt, message delivery, playbook press_enter, the safe-kill prompt, the task dispatch and repair respawn. Keystrokes never interleave.
  - Ordering is FIFO per session. `cancel_prompt { delivery_id }` covers queued items.
  - The UI shows the queue under the composer.
  - Tasks (`dispatch_task`) enqueue instead of typing, which removes the mid-turn delivery problem.
- **Files:** new `st/service/prompt_queue.rs`, `st/service/sessions.rs` (the send path acquires the lock), `st/service/messages.rs`, `st/service/playbooks.rs`, `st/service/safe_kill.rs`, `st/service/tasks.rs` (call sites only), `st/mcp/tools.rs`, `fe/PromptComposer.svelte`.
- **Effort:** M–L.
- **Risk:**
  - Touches many call sites. Do it after P21 and in a quiet window, like W4 F4.
  - A dispatcher bug could stall the queue. Add an age alarm and a manual "deliver now".
- **Dependencies:** P19, P21.

### (f) Reliability

#### P23. Failure modes found, with fixes

| # | Failure mode | Fix | Effort |
|---|---|---|---|
| R1 | `fleet_health.ghosts` counts `claude_status=="ghost"`, so it is always 0 (`st/service/health.rs:70`). | Count `status=="ghost"`, and add a regression test. | S |
| R2 | A ghost is hard-deleted on the next reachable pass, together with its timeline and inbox (`st/store.rs:2815`). A tmux server restart loses everything in about 20 s. | Phase 2 deletes only rows with `lost_at < now - ghost.retention_secs` (default 24 h; the setting is registered in `settings.rs`). The UI shows ghosts collapsed under the project. Adjust the existing test. | S |
| R3 | Hooks drop after `/clear` or `/resume` (D4). | P3. | M |
| R4 | Hook-sourced status is overwritten by the pane guess (D2). | P2 precedence. | M |
| R5 | Hooks missed while fleet is down or the Mac is asleep; there is no replay. | The phase B spool (P4) for high-rate events; for edges, the P2 `hook_trust_secs` fallback plus a SessionStart rebind. Record gaps as `hook_gap` events when reconcile finds `turn_seq` behind the transcript's turn count. | M |
| R6 | Sequential per-session pane capture inside the 30 s probe (D16). | P24. | M |
| R7 | Unknown `claude agents` statuses are dropped (D12). | P6. | S |
| R8 | The reconcile interval needs a restart (D17). | The tick reads the interval each loop, or uses a `tokio::sync::watch` fed by settings writes. | S |
| R9 | Tmux rename re-creates the row (D9). | P11. | S |
| R10 | Local hooks are installed only by a Settings click, and nothing says a host is unhooked (D5). | Install local hooks automatically when the control API is enabled (same merge code). Add a per-host "hooks: v2 · last event 12s ago / never" column in Settings → Hosts. `fleet_health.hosts_unhooked`. | S |
| R11 | There is no hook-health telemetry (D19). | `hosts.last_hook_at`, an `hook_errors` counter, and binding coverage (% of live Claude rows with a bound `claude_session_id` and a hook in the last 24 h) in diagnostics and `fleet_health`. | S |
| R12 | The marker prefix leaks into labels and `last_prompt` (D8). | P9. | S |
| R13 | A PreToolUse-style synchronous HTTP hook would stall tools while the Mac sleeps (a design hazard, not present today). | Document the rule in `docs/control-api.md`: high-rate hooks must be async command hooks writing to the spool. | S |

#### P24. A batched per-host probe
- **Design:** one `bash -lc` script per host prints `tmux list-sessions` and `list-panes` (pane ids), `claude agents --json`, and for every session a delimited `capture-pane -S -8` block. It optionally includes the P4 transcript-tail records. The parse runs off-lock. Round-trips per host per tick drop from N+2 to 1. Keep `-S -8`: it already captures the full visible screen, so dialogs fit. Mirror the `repair_tick` dir-check and `usage` batch style (markers plus a done sentinel).
- **Files:** `st/tmux.rs`, `st/service/sessions.rs` (probe), `st/service/reconcile_tests.rs`.
- **Effort:** M.
- **Risk:**
  - A single large output per host; cap it per session.
  - The 30 s wall clock still bounds it.
- **Dependencies:** best landed before P4 phase A, which rides the same script.

#### P25. Health SLOs, computed and shown in `fleet_health` and diagnostics

| SLO | Target | Measured from |
|---|---|---|
| Edge latency: hook arrival to row event (hooked hosts) | p95 < 2 s | Handler timing histogram in memory; diagnostics prints the p50/p95 |
| Status freshness | every live row's `max(status_at, last_reconciled_at)` within 3× the interval while its host is reachable; 99% | Row timestamps |
| Binding coverage | ≥ 99% of live Claude rows on hooked hosts are bound and have a hook in the last 24 h | P3/R11 |
| Prompt acknowledgement | ≥ 99% of `send_prompt` calls on hooked hosts `accepted` within 15 s | P21 |
| Reconcile pass duration | p95 < 10 s, and no host at the 30 s cap two passes in a row | Gate timings |
| Ghost false positives | ghost → alive again within 1 h: 0 per week | Timeline (`ghosted` then `restored`) |
| AI spend | ≤ `ai.daily_budget_usd`; summary age for active sessions under 15 min at p90 | `ai_usage_daily`, `summary_at` |

#### P26. Tests and chaos checks, using the existing FakeSsh and TmuxExec fakes
1. **Race matrix for status precedence.** A property test with random interleavings of Stop, UserPromptSubmit, PermissionRequest and reconcile passes observing arbitrary pane states. Invariant: a hook-sourced status never regresses to an *older* pane observation, and a missed hook heals within `hook_trust_secs`.
2. **Rebind after `/clear`.** Send a SessionStart(clear) with a new id and a pane header, then assert the next Stop bumps the same row.
3. **Rename during an in-flight pass.** Rename between the probe start and the write; assert the same id, label and usage, and no ghost.
4. **Ghost retention.** An empty probe, then retention, then a revived session within the window keeps its timeline. A tmux-server-restart scenario: all ghosts, then all back.
5. **Idempotency.** The same `request_id` twice gives one send-keys call in the `FakeSsh` log. Different args give `E_CONFLICT`.
6. **Queue ordering under concurrency.** Ten concurrent enqueues plus Stop edges; FakeSsh records keystrokes in FIFO order with no interleaving (it records call order).
7. **Ack matching.** A marked body with multi-line and trailing-newline variants; the unconfirmed timeout attaches the pane.
8. **Chaos: one wedged host.** FakeSsh hang (`set_wall_clock` short) on host B while A progresses. Assert A's `last_reconciled_at` advances, B's rows are marked stale in the UI data, and no ghosting happens on B.
9. **Chaos: hook storm.** 1k hook POSTs a second against the axum router in-process; no store-lock starvation of the tick (duration bound).
10. **Spool replay.** A duplicated line, a truncated last line, rotation and shrink. Reuse #60's cursor tests as the template.
11. **Summariser.** A fake `Summarizer`: debounce, budget stop, clear-reset, and privacy (the input builder never includes a `tool_result` body; fixture assertion).
12. **Label precedence and anti-flap.** A table-driven unit test over `propose_label`.
13. **Pane fixtures.** Real captures of a permission dialog, a question dialog, compaction and a rate-limit banner for `pane_intel`.

### (g) UI

#### P27. Sidebar row, which must stay scannable
- **Line 1:**
  - **Dot and label:** a state dot using the P19 colours, with a freshness ring. The ring is solid when fresh, hollow when stale beyond 3× the interval, and grey with "as of 4m" when the host is unreachable. The label follows, with a tiny source mark: A for AI, P for pinned or user.
  - **Right side:** age of the last event ("12s", "3m"), the cost badge (#60), the context bar, and a **blocker chip** when waiting (`permission: Bash`) or stuck.
- **Line 2 (the meta line, replacing today's elapsed-plus-last-prompt):** `doing_now` (P2/P7), falling back to `last_reply`'s first line, then `last_prompt`. Truncate with an ellipsis; the full text goes in the tooltip.
- **Header:** a **"Needs you (N)"** pill using P13 ranking, which replaces the stuck-only counter.
- **Files:** `fe/Sidebar.svelte` (row snippet; coordinate with W4 F5's split — land after it or inside the new `SessionRowItem`), `fe/attention.ts`, `fe/sessions.ts`.
- **Effort:** M.

#### P28. Session details
**Header:**
- the label, with source and a history popover (P10), and inline edit;
- the state with its age and source (for example "working · 8s · hook");
- `claude_title` when it differs.

**Now panel:**
- **Status:** doing-now, the blocker with the question text (P5), and a queue (P22) with cancel.
- **AI panel:** goal, rolling summary with the turn it covers and its age, next step, suggestion chips (P15), and "Summarise now".

**History panel:**
- the **timeline** rendered from `session_history`, finally visible, with filter chips for turns, prompts, errors, labels and ops;
- `last_reply`.

**Numbers:** cost with burn rate (P18), context, compactions, subagents active.

- **Files:** `fe/SessionDetails.svelte`, a new `fe/Timeline.svelte`, `fe/history`-style IPC for `session_history`.
- **Effort:** M.

#### P29. Quick switcher
Covered by P12. It is also the fastest path to the "Needs you" list: open it with an empty query to see the triage queue.

---

## 4. Waves, tracks and file ownership

The layout copies the 2026-09-10 plan: tracks inside a wave own disjoint files and can run as separate fleet sessions on separate worktrees. Migrations are pre-assigned to avoid collisions:
- 026: activity and binding (Track A);
- 027: labels and summaries (Track N creates the label columns; Track AI appends `summary_*`, `session_summaries` and `ai_usage_daily` as **028**);
- 029: deliveries and requests (Track C).

Renumber at merge if the order changes. `migrations_are_contiguous_from_one` enforces the sequence.

### Wave Q: quick wins (1–3 days, all S, all parallel)

| # | Task | Resolves | Owns files |
|---|---|---|---|
| Q1 | Count ghosts by `status` in `fleet_health`; add a test. | R1, D15 | `st/service/health.rs` |
| Q2 | Strip the untrusted marker before `record_prompt_outcome`. | P9, D8 | `st/service/sessions.rs` (the `record_prompt_outcome` / `send_prompt_inner` block only), `st/mcp/tools.rs` (`deliver_prompt` only) |
| Q3 | Add `friendly_name` (and `last_reconciled_at` once Q4 lands) to the MCP `SessionSummary`. | D10 | `st/mcp/tools.rs` (struct only) |
| Q4 | Put `last_reconciled_at` and `last_hook_at` on the wire; the Sidebar greys a row whose data is older than 3× the interval, with a tooltip "as of …". | D6 (partial) | `st/store.rs` (`SESSION_COLUMNS` and the mapper), `fe/sessions.ts`, `fe/Sidebar.svelte` (row class only) |
| Q5 | Ghost retention setting (default 24 h). | R2, D14 | `st/store.rs` (ghost phase 2), `st/service/settings.rs`, the store test |
| Q6 | Permission and question pane cues map to blocked (with real fixtures); parse `claude agents` state, `status` busy and waiting, and `waitingFor` (**check a live host's output first**). | D3, D12, P6 | `st/service/pane_intel.rs`, `st/claude_agents.rs`, `known_agent_status` |
| Q7 | Reconcile interval takes effect without a restart. | R8 | `st/lib.rs` (tick) |
| Q8 | Auto-install the local hook when the control API is enabled; show per-host "hooks: installed / never seen". | R10 | `st/commands/mcp.rs` (local install), `fe/SettingsDialog.svelte` (hosts table column) |
| Q9 | Render `session_history` in SessionDetails (read-only timeline). | D11 (display) | new `fe/Timeline.svelte`, `fe/SessionDetails.svelte` (one mount) |
| Q10 | In-place tmux rename. | P11, D9 | `st/service/sessions.rs` (the `rename_session` fn), `st/store.rs` (new fn), `st/service/reconcile_tests.rs` (new test) |

Q2 and Q10 both touch `st/service/sessions.rs`, but in disjoint functions. Q4 and Q5 both touch `st/store.rs` in disjoint sections. Run them sequentially if you prefer zero rebase risk.

### Wave 1: deterministic signals and ownership (1–2 weeks, three parallel tracks)

**Track A: hooks and activity.** Owns `st/commands/mcp.rs` (hook section), `st/mcp/hooks.rs`, `st/service/hooks.rs`, `st/service/provision.rs` (hook step), the hook recorders and activity columns in `st/store.rs`, new `st/service/activity.rs`, migration 026, and `st/service/diagnostics.rs`.

| # | Task | Proposals |
|---|---|---|
| A1 | Full low-rate hook set; payload fields; timeline edges; per-host `last_hook_at` and error counter. | P1, R11 |
| A2 | Activity columns and precedence (`status_source`, `status_at`, `hook_trust_secs`); on the wire. | P2, R4 |
| A3 | Pane-id header plus the SessionStart rebind. | P3, R3 |
| A4 | Fast-lane capture on PermissionRequest and Notification; single-session reconcile on selection. | P5 |

**Track R: probe and reliability.** Owns `st/tmux.rs`, the probe section of `st/service/sessions.rs`, and `st/service/reconcile_tests.rs`.

| # | Task | Proposals |
|---|---|---|
| R-1 | Batched per-host probe (full-screen captures, pane ids). | P24, R6 |
| R-2 | Transcript-tail activity (phase A), on the batched script. | P4 A |
| R-3 | Chaos tests 1, 4, 8 and 9 from P26. | P26 |

**Track N: naming and identity.** Owns the label functions in `st/store.rs`, the friendly-name section of `st/service/sessions.rs`, the MCP label tools in `st/mcp/tools.rs`, `skills/fleet-friendly-name/SKILL.md`, the CLAUDE.md block in `st/service/provision.rs`, the rename UX in `fe/Sidebar.svelte`, and migration 027.

| # | Task | Proposals |
|---|---|---|
| N1 | `label_source`, `label_pinned`, `label_set_at`; `propose_label`; `session_labels` history. | P10 |
| N2 | UI: double-click edits the label; context-menu tmux rename; source mark; history popover. | P10 |
| N3 | Skill rewrite: goal and label at first prompt and after `/clear`; `set_session_goal`; drop the heartbeat. | P8 |
| N4 | Quick switcher text and search. | P12 |

Cross-track contracts: A2 defines the `activity_*` wire names before R-2 writes them. Agree them in the first PR of A2, schema only.

### Wave 2: control and triage (1–2 weeks, after Wave 1 A1/A2)

**Track C: deterministic control.** Owns the new `st/service/state.rs`, `st/service/prompt_queue.rs` and `st/mcp/idempotency.rs`; migration 029; the send, run and dispatch handlers in `st/mcp/tools.rs`; the wait condition in `st/service/tasks.rs`.

| # | Task | Proposals |
|---|---|---|
| C1 | State machine (derive-only), `state_anomaly` events, `require_state`. | P19 |
| C2 | Request ids and the `mcp_requests` table. | P20 |
| C3 | Delivery acknowledgement (`prompt_deliveries`, `prompt_status`, `run_prompt` waits on its own delivery). | P21 |
| C4 | Per-session queue and keystroke lock across every send path (quiet window). | P22 |

**Track T: triage and UI.** Owns `fe/attention.ts`, the `fe/Sidebar.svelte` row (in coordination with F5), `fe/SessionDetails.svelte`, and the new `fe/Digest.svelte`.

| # | Task | Proposals |
|---|---|---|
| T1 | Ranking with "Needs you", done-unread (`last_viewed_at`), and the freshness ring. | P13, P27 |
| T2 | Session details: Now, History and Numbers panels. | P28 |
| T3 | Deterministic loop rules and new stuck kinds (notify-only playbooks). | P14 |
| T4 | Cost burn-rate anomaly. | P18 |

### Wave 3: AI layer (1–2 weeks, after A1, N1 and T2)

**Track AI.** Owns the new `st/service/summary.rs`, migration 028, the `ai.*` rows in `st/service/settings.rs`, the `fleet_ai` block in `st/service/usage.rs`, the `session_summary` and `fleet_digest` tools in `st/mcp/tools.rs`, and the AI panel in `fe/SettingsDialog.svelte`.

| # | Task | Proposals |
|---|---|---|
| AI1 | `Summarizer` trait plus the `HostCli` implementation, debounce, budget, `ai_usage_daily`, and the AI panel in details. | P7 |
| AI2 | Optional `Api` implementation (keychain key, prompt caching). | P7 |
| AI3 | Suggested prompts (same call). | P15 |
| AI4 | Fleet digest: deterministic table first, AI paragraph optional. | P17 |
| AI5 | Tag suggestions from goals. | P16 |

### Wave 4: optional depth

| # | Task | Proposals |
|---|---|---|
| X1 | Per-tool spool via async command hooks; loop rules gain per-tool fidelity. | P4 B |
| X2 | Status-line wrapper for an exact context %, Claude's own title and live cost. | P6 |
| X3 | Stable `sessions.uid` across `move_session`. | P11 |
| X4 | Count subagent transcripts in usage (the #60 follow-up). | D18 |

### Dependency sketch

```
Wave Q (all parallel)
   |
   +--> Track A (hooks/activity) --+--> Track C (control) --+
   +--> Track R (probe)  <-- R-2 needs A2 wire names         +--> Track AI (summaries, digest)
   +--> Track N (labels) ----------+--> Track T (triage/UI) -+
Wave 4 whenever; X1 after A1 + R-2; C4 needs a quiet window on the send paths.
```

**Quick wins to do first:**
- Q1: one-line bug.
- Q2: bad labels on every MCP-driven session.
- Q4: freshness visible at last.
- Q5: ghosts become recoverable.
- Q6: blocked sessions stop reading as idle.
- Q9: the timeline already exists; just show it.
- Q10: rename stops destroying history.

---

## 5. Decisions for the owner

1. **Double-click semantics.** Should it edit the display label (recommended) rather than rename tmux?
2. **Summariser location and default.** The recommendation is `HostCli`, opt-in per host, so transcripts stay on the host and billing goes to the subscription. Or `Api` with a key on the Mac? What daily budget? A default of $2 is proposed.
3. **The friendly-name skill.** Demote it to a one-call goal declaration (recommended), keep it as is, or remove it.
4. **`--name <tmux_name>` at launch.** Stop passing it so Claude's own AI title becomes available as a free label source. This depends on P3 replacing name matching.
5. **Ghost retention default.** 24 h is proposed.
6. **New `stuck_kind` values** (`tool_loop`, `hung`, `compact_thrash`, `rate_limited`). This is a vocabulary change visible to MCP clients and the skill.

---

## 6. Sources

**Claude Code and Anthropic docs** (fetched 2026-09-11):
- https://code.claude.com/docs/en/hooks: the full event list, input fields, HTTP hook semantics, `allowedEnvVars`, async command hooks
- https://code.claude.com/docs/en/statusline: status-line JSON (session_name, cost, context_window, rate_limits)
- https://code.claude.com/docs/en/agent-view: `claude agents --json` state, status and waitingFor; Haiku row summaries and anti-flap rules
- https://code.claude.com/docs/en/sessions: session naming, AI-generated titles, transcript location; the format is internal
- https://code.claude.com/docs/en/headless and https://code.claude.com/docs/en/cli-reference: `-p`, `--output-format`, `--no-session-persistence`, `--bare`, `--json-schema`
- https://code.claude.com/docs/en/agent-sdk/sessions and https://code.claude.com/docs/en/agent-sdk/typescript: `listSessions`, `summary`, `customTitle`
- https://code.claude.com/docs/en/monitoring-usage: OpenTelemetry metrics and events
- https://platform.claude.com/docs/en/about-claude/pricing and https://platform.claude.com/docs/en/about-claude/models/overview: Haiku 4.5 pricing and retirement date
- https://github.com/anthropics/claude-code/issues/32634: the `idle_prompt` timer stays timer-based

**Comparable tools:**
- https://github.com/smtg-ai/claude-squad, including `session/tmux/tmux.go` and `session/instance.go`
- https://github.com/kbwo/ccmanager, including `docs/status-hooks.md`
- https://www.conductor.build/docs/ and https://www.conductor.build/docs/concepts/workflow
- https://github.com/stravu/crystal (deprecated)
- https://github.com/BloopAI/vibe-kanban and https://vibekanban.com/blog/shutdown
- https://imbue.com/blog/sculptor-announce
- https://github.com/getAsterisk/opcode
- https://github.com/siteboon/claudecodeui
- https://learn.chatgpt.com/docs/config-file/config-advanced and https://developers.openai.com/codex/cli/reference (Codex notify, hooks, exec --json, cloud)
- https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/manage-and-track-agents and https://github.blog/changelog/2026-03-20-trace-any-copilot-coding-agent-commit-to-its-session-logs/
- https://docs.devin.ai/api-reference/v1/sessions/retrieve-details-about-an-existing-session
- https://cursor.com/docs/cloud-agent/api/endpoints
- https://github.com/joshmedeski/sesh
- https://langfuse.com/docs/observability/features/sessions, https://docs.langchain.com/langsmith/threads, https://github.com/agentops-ai/agentops

**Loop-detection references:**
- https://github.com/NousResearch/hermes-agent/issues/512
- https://docs.promptise.com/blog/ai-agent-stuck-repeating-tool-call/

**Transcript format** (third-party and unverified; used only to motivate fail-soft parsing):
- https://github.com/neilberkman/ccrider/blob/main/research/schema.md

---

## Addendum (2026-09-11): owner input and updated recommendation

### A.1 Owner input received

One message arrived directly from the owner in this session (a user turn, not relayed by team-lead). It read, verbatim:

> Decitions: label. Give me ideas about summarizers, How to improve it, what about friendly name skill. Recomend me something

I read "Decitions: label" as the answer to §5 decision 1: double-click edits the display label rather than renaming the tmux session. That is an interpretation of a one-word answer and should be confirmed. The message answered no other §5 decision; it asked for summarizer ideas and a view on the friendly-name skill.

### A.2 Label precedence (supersedes P10 rule 1 and the P8 wording)

New order: **user > AI > agent (the session's own label) > Claude's title > prompt > branch/generated.**

- **User** labels are pinned and never overwritten until the user clears the pin.
- **AI** outranks the agent. The agent declares its label once, at the start of a task, and that label goes stale as the work drifts, which is the problem the owner wants fixed. The AI summarizer observes the session from outside for its whole life.
- **Agent** labels remain the seed. After `SessionStart(clear|startup)`, non-pinned labels are demoted so the new task's agent label can apply.
- The P10 anti-flap rules still apply to AI and agent changes:
  - at most one change per 15 minutes and 6 per day;
  - reject a change whose token Jaccard with the current label is 0.5 or more;
  - record every change in `session_labels` history.
- With AI off, the order collapses to user > agent > Claude's title > prompt > branch.

### A.3 Recommendation: summarizer

Build it in three steps.

**Step 1: free descriptions, no model (S–M; do first).**
- **"Just said":** the first line of `Stop.last_assistant_message`, stored as `last_reply` (P1/P2).
- **"Doing now":** the last tool call from the transcript tail (P4 phase A).
- **Goal hint:** the first prompt after SessionStart(startup|clear), with the marker stripped (P9).
- **Claude's own title:** Claude Code already generates one with Haiku. Fleet probably suppresses it by launching with `--name`; verify that, then decide whether to stop passing `--name` (§5 decision 4, needs P3).

**Step 2: AI summarizer (opt-in; P7 with these specifics).**
- **Where:** default `HostCli`, i.e. `claude -p --model haiku` on the session's own host. The transcript never leaves the host and billing goes to that host's Claude login. The `Api` implementation on the Mac is a later option.
- **When:**
  - at the end of a turn, at most once every 10 minutes per session;
  - every 15 minutes during long turns;
  - on a "Summarize now" button.
- **Input (rolling):** the previous summary plus only what happened since: new prompts, assistant text and one line per tool call. Never tool results or file contents. About $0.005 per call.
- **Output fields:**
  - title (3–6 words);
  - doing now;
  - a 2–3 sentence summary;
  - goal;
  - blocker;
  - next step;
  - state (on_track, waiting_on_user, blocked, done or looping).
- **Guardrails:**
  - a default $2/day cap, after which the UI shows "AI paused";
  - spend logged in `ai_usage_daily` next to #60's `usage_daily`;
  - each summary shows its age and the turn it covers.

**Five improvements over a plain summary:**
1. **"Since you last looked":** when the user opens a session, show what changed since their last visit (keyed on `last_viewed_at`, P13) instead of the whole story. This is the most useful view at 60 sessions.
2. **One call per host:** batch several due sessions into one `claude -p` call, so the CLI's fixed prompt overhead is paid once. Each session's part of the output is keyed by `session_id`, and a failure falls back to per-session calls.
3. **Stable titles:** the AI title changes only when the task really changed, using the A.2 anti-flap rules.
4. **Free suggested next prompts:** the same call returns 2–3 next prompts. A click fills the composer; nothing is ever sent automatically (P15).
5. **Feedback:** a thumbs-down regenerates the summary and records the miss (a `summary_rejected` event), so the summarizer prompt can be tuned against real failures.

### A.4 Recommendation: friendly-name skill

Shrink it; don't delete it.

**Why its current form doesn't work:**
- it depends on the model remembering to run it;
- each run costs 3 tool calls in the working session's context;
- it can't run while the session is blocked;
- it never runs on hosts that were never provisioned;
- its heartbeat relies on the model counting to 10.

**The rewrite (P8, revised):**
- Fire only on the first prompt and on the first prompt after `/clear`. Drop the heartbeat.
- Make one call, `set_session_goal { goal, label }`, with the session resolved server-side from the caller's host token plus the tmux pane id (P3). That takes it from 3 calls to 1.
- It supplies the one thing only the session knows, its intent. It writes `goal` and a seed label with `label_source='agent'`. From then on the summarizer tracks drift (A.2).
- With AI off, the shrunk skill plus the Step 1 free descriptions are still a clear improvement over today.

### A.5 Order and effort

1. Quick wins (Wave Q) plus label ownership (P10) with double-click editing the label: S–M.
2. Step 1 free descriptions (P1 `last_reply`, P4 phase A, P9): S–M.
3. Skill rewrite (P8 revised): S.
4. Host summarizer with the cap, batching and "since you last looked" (P7 plus A.3): M–L.

### A.6 Still open

- Confirm the A.1 interpretation of "Decitions: label".
- Daily AI budget: is the proposed $2/day right?
- `--name` at launch: can fleet stop passing it, so Claude Code's own title comes through?
- §5 decisions 5 and 6 (ghost retention default, new `stuck_kind` values) are unanswered.

### A.7 Pre-check results (local host, Claude Code 2.1.267, 2026-09-11)

`claude agents --json`, run on the local host:

**Interactive rows** carry `pid`, `cwd`, `kind: "interactive"`, `startedAt`, `sessionId`, `name` and `status`.
- Values observed for `status`: `idle` and `busy`.
- No `state` field.

**Background rows** carry `id`, `cwd`, `kind: "background"`, `startedAt`, `sessionId`, `name` and `state`.
- Value observed for `state`: `blocked`.
- Some background rows also carry `status: "idle"`.

No `waitingFor` appeared, because no session was waiting at the time.

**Consequences (D12 confirmed):**
- `busy` is not in fleet's `ClaudeStatus` vocabulary, so `known_agent_status` drops it (`st/service/sessions.rs:3049`). Every working interactive session therefore falls back to the pane guess.
- Background `state` is never parsed (`st/claude_agents.rs:7`), so a blocked background agent is invisible.
- Q6 must:
  - parse `kind`, `state` and `waitingFor`;
  - map `busy`→working, `waiting`→blocked, `done`→completed;
  - prefer `state` over `status` for background rows.

**`--name` (§5 decision 4) is moot:**
- Interactive panes launch with `cl --resume '<id>' || cl --session-id '<id>' || cl` and no `--name` (`st/tmux.rs:435`). Only `claude --bg` passes `--name` (`st/claude_cli.rs:73`).
- The interactive `name` in agents output is Claude's default `<dir>-<2 chars>` display name (e.g. `jhkljh-f2`). It is not an AI title and never equals the tmux name.
- So `find_by_name` never matches interactive sessions; they bind by unique cwd only. That strengthens the case for P3 (pane-id binding).
- Claude's AI title, where one exists, is not in `claude agents` output. The "Claude's title" source in A.2 therefore needs the status-line wrapper (X2) or the transcript, or should be dropped.

**Not yet checked:** a remote host's output, which may run another CLI version.

### A.8 Correction (2026-09-11, found while reviewing PR #78)

The reconcile pane probe runs `tmux capture-pane -t <name> -S -8 -p` (`st/tmux.rs:96`). tmux counts `-S` from the first visible line, with negative values reaching into history. With no `-E`, the capture ends at the last visible line. So the probe captures **the whole visible screen plus 8 history lines**, not an 8-line tail.

- The code comment at `st/service/sessions.rs:12-15` is wrong about this.
- So was this report's original wording in §0, §1.2 and D1; those passages are now corrected.
- D1's conclusion stands: `current_activity` is still the last non-blank line, which is usually the REPL footer.
- Permission and question dialogs fit in the existing capture, so no longer tail is needed (P5 and P24 are corrected).
