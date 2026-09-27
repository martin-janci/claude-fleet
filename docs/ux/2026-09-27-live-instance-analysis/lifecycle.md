# Session lifecycle / state machine / recovery — live-instance review (2026-09-27)

Scope: hub `fleet.rlt.sk` (fleet-hub 0.3.1, schema 60) read through the MCP server, the desktop
DB snapshot `scratchpad/db/state.db` (stale since 2026-09-25), and the source in this worktree
(`crates/fleet-core/src/service/{pane_intel,playbooks,gc,attention,hooks,tick}.rs`,
`service/sessions/{reconcile,lifecycle,restore}.rs`, `store/{sessions,reconcile}.rs`). Read-only;
no session was touched. Note: the MCP token handed to this review is bound to host `mac`
(`capture_session` on trn/mefistos refused with `E_FORBIDDEN`), so pane contents on other hosts
could not be inspected — those findings rest on row stamps and the code path.

Severity: P0 data loss / wrong automated action; P1 wrong state that misleads the operator or
automation; P2 correctness gap with a visible symptom; P3 hygiene.

---

## F1 — P0: the `oom` detector fires on prose, and the playbook kills a live, working session

**Evidence**
- `service/pane_intel.rs:376-386` (`detect_stuck`): any pane tail containing `out of memory`,
  `cannot allocate memory`, `oomkilled` or the whole word `oom` → `StuckKind::Oom`. No check that
  the REPL is gone; `derive_status` / `LIVE_REPL_CUES` (`esc to interrupt`, `? for shortcuts`,
  `pane_intel.rs:479`) are not consulted.
- Session 21480 (`dev-martin-janci-claude-fleet--misty-saturn`, mefistos, project 13 = claude-fleet)
  in the desktop snapshot (`session_events`):
  - 23:38:38 `prompt_sent` "Analýzuj MCP a jeho funkcionalitu…"
  - 23:46:10 `stuck oom` → 23:46:11 `playbook_applied oom:recreate`, `recreated`, `session_end other`
  - 23:46:25 `status_change idle` + `stuck oom` again (14 s after the resume: `claude --resume`
    re-renders the last messages, which still contain the word)
  - 23:52:49, 23:53:25 `stuck oom`; 23:53:54 `turn_done` — a full answer about the MCP tool
    surface ("73 toolov = 64 265 znakov…"). The tool descriptions it was measuring literally carry
    `stuck_kind is one of auth_menu | reconnect | trust_prompt | oom | press_enter`.
  - 01:09:10 `stuck oom` → 01:09:12 second `oom:recreate` + `session_end other` (the 3600 s spacing
    had just expired); 02:28 `turn_done` again.
  - The row never had a real OOM: it kept producing turns until 07:49.
- Session 21340 (also project 13): `stuck oom` 9× on 09-10/09-11; 21382 (kuk-agent) 1×. All three
  sessions were reading/grepping code or docs that contain the fleet's own stuck vocabulary.
- `service/playbooks.rs:98-107`: `oom` → `Recreate` when `playbooks.oom_recreate` is on (it is:
  settings snapshot), gated only by `OOM_RECREATE_MIN_SPACING_SECS = 3600` and `last_playbook_at`;
  no cap on attempts, no look at `claude_status`, `last_turn_at` or the spinner.
- `service/sessions/lifecycle.rs:1387-1392` (`recreate_session`): `tmux.kill_session` then
  `tmux.new_session(… --resume <id>)` — the in-flight turn is destroyed; the resume only restores
  the transcript.
- Episode restart: `store/reconcile.rs:436-440` re-stamps `stuck_since` whenever `stuck_kind` goes
  NULL → `oom` again, so every re-detection is a "new episode" and only the 1 h spacing stands
  between recreates. `stuck` events are written per detection, not per episode (8 rows in 83 min).
- The hub has no timeline left for 21480 (`session_history 21480` → `[]`): Phase 2 of the reap
  deletes `session_events` with the row (`store/reconcile.rs:648-651`), so the post-mortem only
  exists in the desktop snapshot.

**Root cause** Content-only detection of a process-level condition, on a fleet whose own source,
docs and MCP instructions contain the trigger word; the playbook trusts the detector blindly and
its "backoff" is a spacing, not a budget.

**Proposed fix**
1. Corroborate: return `Oom` only when the tail shows the Node fatal block
   (`FATAL ERROR: Reached heap limit`, `<--- Last few GCs --->`, `Allocation failed`) or when the
   pane's foreground command is a shell (`tmux display -p '#{pane_current_command}'`, already
   probed for `attached`) — never on bare `oom` / `out of memory` prose while
   `LIVE_REPL_CUES` are present.
2. Playbook guard: refuse `Recreate` when `claude_status == working`, when `last_turn_at >
   stuck_since`, or after 2 recreates per session per 24 h; write the refusal as
   `oom:recreate:skipped:<why>`.
3. Record `stuck` once per episode and put the matching line in the event detail so a false
   positive is diagnosable from the timeline.
4. Keep a tombstone of the timeline (like `trg_work_journal_session_delete` does for
   conversations) or defer the `session_events` delete to a later sweep.

**Effort** M (detector S, playbook guard S, timeline retention S–M).

---

## F2 — P1: two `working` rows have not worked for ~40 h; nothing can demote them

**Evidence** (`list_sessions {claude_status: working, summary:false}`, now ≈ 1790506900 = 09-27 11:01Z)
- 21639 `dev-papayapos-pos-frontend--lively-mercury` (trn): `claude_status working`,
  `last_stop_at 1790179688` (09-23 16:08Z), `last_activity_at 1790346878` (09-25 14:34Z),
  `context_at 1790364096` (09-25 19:21Z, transcript, not stale), `current_activity` is the footer
  `⏵⏵ bypass permissions on · 1 shell · ← 3 agents` — no spinner line.
- 21683 `dev-papayapos-papayapos-backend--pd-2995` (trn): same shape, `last_stop_at 1790262965`
  (09-24 15:16Z), `context_at 1790364072`.
- The third `working` row is the operator 21535 (`last_stop_at` 4 min ago) — genuine.
- Both stale rows are on the agent-transport host, so no pane capture was possible with this token.
- `service/sessions/reconcile.rs:1717-1731` (`status_candidate`): when `claude agents --json` was
  asked this pass, its status wins outright; when it was not asked (cadence), only `Blocked` from
  the pane is applied. There is no rule that turns `working` into `idle` from the absence of a
  spinner or the absence of hooks/transcript growth. An interrupted turn (Esc) fires no `Stop`
  hook (`hooks.rs:860`), so `record_prompt_submit_hook_for_row`'s `working`
  (`store/sessions.rs:1102`) persists until the next Stop.
- Hooks are installed on every host (brief), so this is not the "pre-UserPromptSubmit host" case;
  `last_hook_at` only guards in-flight passes (`store/reconcile.rs:296-309`) and is otherwise inert.

**Root cause** `working` has no staleness bound; the CLI's `claude agents` status (which reports
a session with live subagents as working) overrides the pane, and no hook exists for "turn
interrupted".

**Proposed fix** A stale-working rule in reconcile: `working` AND `last_hook_at`,
`last_turn_at`, `context_at` all older than N min (e.g. 30) AND `spinner_line(pane) == None` →
`idle` with `idle_since = max(last_stop_at, context_at)`; emit `status_change idle
(stale_working)`. Surface it in `fleet_health` as `working_stale`. **Effort** M.

---

## F3 — P1: `StopFailure` (429 rate limit) ends as a normal `idle`; nothing surfaces it

**Evidence**
- 20773 `pd2758-e2e`: `stop_failure` event 2026-09-18 08:21:41 `rate_limit: 429 … "This request
  would exceed your account's rate limit"`; the next `status_change working` rows are the user
  re-prompting by hand at 08:33, 08:54, 08:57 … (11 attempts in 90 min).
- `service/hooks.rs:1037-1070` → `record_stop_failure_hook_for_row` is literally
  `record_stop_hook_for_row` (`store/sessions.rs:1164-1169`): `claude_status='idle'`,
  `idle_since` set, one `stop_failure` event. No `failed` status, no `stuck_kind`, no retry.
- `service/attention.rs:88-95`: `Failed` requires `claude_status == "failed"`, which no hook writes
  (only `claude agents` can report it). The playbook table has no `stop_failure` kind.

**Root cause** The failure hook was modelled as "a Stop that also logs".

**Proposed fix** Set `claude_status='failed'` (kept until the next UserPromptSubmit) with the
error type in `current_activity`; attention reason `failed`; an opt-in `rate_limit` playbook that
re-sends the queued prompt after the retry-after / exponential backoff (max 3). **Effort** S.

---

## F4 — P2: ghosting keeps stale `claude_status` / `blocked` / `current_activity`

**Evidence**
- `local` ghosts 21486, 21490, 21491, 21498: `claude_status working`, `lost_at 1790431981`
  (09-26 14:13Z). `mac` ghost 21504: `claude_status blocked`, lost 09-21.
- `store/sessions.rs:332` (`mark_host_sessions_lost`) and `store/reconcile.rs:623` (Phase 1) update
  only `status`, `lost_at`, `lost_reason`; `claude_status`, `current_activity`, `stuck_kind`,
  `pending_input`, `idle_since` are untouched.
- `service/attention.rs:88-95`: `blocked` outranks `lifecycle`, so a *tmux* ghost that died while
  showing a permission prompt is reported as "waiting" — an answer nobody can give (external rows
  are exempt, which is why 21504 does not show).

**Proposed fix** On loss: `claude_status=NULL`, clear `current_activity`/`stuck_kind`/
`pending_input`, keep the old value in a `last_claude_status` column for the UI; in
`needs_attention`, test `lost_at`/ghost before `blocked`. **Effort** S.

---

## F5 — P2: hidden `local` host: 6 immortal ghosts, 3 of them duplicates of `mac` rows

**Evidence**
- `list_sessions {host_alias: local, include_lost:true}`: 6 `bg:*` rows, `kind external`,
  `lost_reason local_disabled`, `lost_at 1790431981`. `local` is `hidden:true, reachable:false`
  (`list_hosts`), last pinged 09-20.
- `service/sessions/reconcile.rs:1428`: `hosts.into_iter().filter(|(h, _)| !h.hidden)` — a hidden
  host never gets a pass, so Phase 2 (`store/reconcile.rs:642`) never runs for it and the TTL
  exemption is irrelevant: these rows live until someone calls `dismiss_ghost_session` six times.
- `cb0bab25…`, `11d6176e…`, `a0336855…` exist as 21490/21491/21498 (`local`) AND 21504/21505/21507
  (`mac`): the rename created new rows under the new alias instead of moving them.
- `local_disabled` does not occur anywhere in this checkout (`grep -rn`, `git log -S`), so it was
  written by the deployed hub binary (a newer main) or by hand.
- `fleet_health.ghosts = 3` counts only non-external `status='ghost'` (`health.rs:113-120`), so
  none of the 19 lost rows the UI shows (16 external + 3 tmux) are in the number.

**Proposed fix** (a) host hide/rename: re-key live rows to the new alias by `claude_session_id`
and hard-delete the rest; (b) let the GC tick reap ghosts on hidden/unreachable hosts once
`lost_at < now - lost_ttl` (today the TTL is only applied by a reachable host's pass);
(c) `fleet_health`: report `ghosts` per kind or count every `lost_at` row. **Effort** S–M.

---

## F6 — P2: Code-tab desktop sessions are `external`: never retired, never resumable, kept as ghosts 14 days

**Evidence**
- `reconcile.rs:1106-1107`: `AgentKind::Interactive → "external"`, `Background → "bg"`. The five
  live `bg:*` rows on `mac` (21708/21709/21718/21719/21720) are `kind external`.
- `reconcile.rs:1116-1120`: `agent_is_inactive → stopped` applies to `kind == "bg"` only;
  `service/gc.rs:120` skips `external`; `dismiss_agent_session` refuses `external`
  (`bg_sessions.rs:302`). So an external row is retired only when `claude agents --json` stops
  listing it (routine `missing` ghost, reaped next pass).
- But `store/reconcile.rs:593-596` exempts any ghost with `claude_session_id` and
  `lost_reason IN ('host_reboot','tmux_server_gone')` for `sessions.lost_ttl_secs` (14 d), for
  every kind — while `restore.rs:166-170` refuses to restore `bg`/`external`, so the exemption
  buys nothing for them.
- The Mac reboots daily (`last reboot`: 09-21 11:10, 09-23 17:20, 09-24 18:21, 09-25 23:19,
  09-26 13:09 local — each within 1–4 min of a `host_reboot` stamp: 1789981930, 1790176914,
  1790267105, 1790371204, 1790421034). The reboot verdict itself is CORRECT; it is the TTL that
  turns every reboot into a fresh batch of unresumable external ghosts (9 on `mac` today,
  first reap due 10-05).
- The brief's "an inactive agent (claude_status: stopped) is removed from the list" applies to
  `kind bg` only (`lifecycle.rs:914-934`, `reconcile.rs:1119`); no bg row exists in the fleet
  right now, so that path is unexercised.

**Proposed fix** Apply the TTL exemption only to kinds `restore_host_sessions` can resume
(`!matches!(kind, "bg" | "external")`); reap external ghosts next pass (or after a short grace,
e.g. 1 h, in case the desktop app is merely restarting). **Effort** S.

---

## F7 — P2: the attention model flags the three things a human can do nothing about and misses the five they would

**Evidence** `list_sessions {needs_attention:true}` → 21631, 21706, 21541: all `mac` tmux ghosts
(`reason lifecycle`, lost 09-23…09-25). Not flagged:
- context at 99 % (21690 PD-2543), 94 % (21686 PD-2348), 91 % (21509), 90 % (21485), 86 % (21534)
  — `fleet_health.context_red = 5` counts them (`health.rs:121`, threshold 85, all
  `context_source transcript`, not stale) but `attention.rs` has no context reason.
- the two stale `working` rows (F2), the 429 session (F3).
- idle sessions whose PR CI is `failing`: 21677, 21647, 21519, 21690.
- `stuck = 0` and no `blocked` row right now, so the two reasons the model is built around are
  empty; `waiting`/`stuck` only work while the hook/pane path keeps them fresh.

**Proposed fix** Add reasons `context_red` (≥ 90), `stale_working`, `failed_turn`, `ci_failing`
(idle + failing PR); rank `lifecycle` ghosts last and drop them after `lost_ttl`. Keep the
"no knob" principle by making thresholds constants shared with `fleet_health`. **Effort** S–M.

---

## F8 — P3: `kind=shell` rows leak into `by_status`

**Evidence** `fleet_health.by_status.unknown = 2` = `sleek-castor-term` (21479) and
`coral-bootes-term` (21513), both `kind shell`, `claude_status null`; `noble-virgo-term`
(21710, oci) is `kind shell` yet `claude_status idle` — the pane heuristic read a bare `❯` prompt
as an idle REPL. `health.rs:113` excludes only `external`; the tmux upsert writes pane-derived
status for every kind.

**Proposed fix** Exclude `shell` from `by_status`/`context_red`/`stuck`; never derive
`claude_status` for `shell` rows (skip intel or force NULL in `apply_host_reconcile`). **Effort** S.

---

## F9 — P3: "controller" is one unguarded settings pair per store; the desktop's copy is dead

**Evidence**
- `store/mod.rs:271-281`: `controller.host` / `controller.tmux` settings; the only writer is
  `register_self` (`mcp/tools/session_ops.rs:254`) — no ownership check, no expiry, last writer wins.
- Hub: the operator 21535 `fleet-operator` on mefistos is `is_controller: true` (it registered
  itself). Desktop DB: `controller.host=claude-fleet-trn / controller.tmux=review-pd2713` — a
  session that no longer exists; in hub-client mode the desktop never consults it (routing goes to
  the hub, `is_controller` arrives on the wire), so it is merely stale.
- Two controllers cannot coexist inside one store, but any session with a full token that calls
  `register_self` silently displaces the operator. The operator keeps `refuse_if_operator`
  (`operator.rs:97`, keyed on `operator.session`), so kill/recreate still refuse, but it loses the
  `playbooks.rs:88` / `gc.rs:291` / `repair_tick.rs:406` controller exemptions (GC still skips it
  only because it has no worktree, `gc.rs:132`).

**Proposed fix** Refuse `register_self` while an operator is registered unless `force`; write an
audit event on controller change; expose the controller in `fleet_health`; on hub pairing clear
the desktop's local pair. **Effort** S.

---

## F10 — P3: timeline noise hides transitions

**Evidence** Desktop snapshot: 2674 of 2948 `session_events` are `status_change`; 21340 alone has
hundreds of consecutive `busy → busy` rows (ids 2088456…2106523, every 20–60 s), and `stuck oom`
is written on every detection (F1). Genuine transitions (`idle`, `working`, `stop_failure`,
`playbook_applied`) are a few percent of the table.

**Proposed fix** Emit `status_change` only when the value differs from the stored one (the
`Prior` struct in `reconcile.rs:626` already exists for this); one `stuck` per episode. **Effort** S.

---

## Answers to the eight questions, in short

1. **OOM loop** — recreate DID happen (twice, `recreated` + `session_end other`), on a live
   working session; the only backoff is a 3600 s spacing, no attempt cap; recurrence is a
   detector false positive on the word `oom` (the fleet's own vocabulary) re-rendered by
   `--resume`. F1.
2. **Ghost lifecycle** — loss sets only `status/lost_at/lost_reason` (F4); reap is Phase 2 of the
   next pass (one cycle) except resumable mass-loss rows, kept `lost_ttl` 14 d; `gc.enabled`
   kills IDLE LIVE rows only (`gc.rs:120` skips ghosts), it never removes ghosts. Survivors:
   14-day TTL on `mac` (F6), hidden host with no pass on `local` (F5). bg auto-removal exists only
   for `kind bg` (`stopped` after 24 h idle, then GC kill); the Code-tab rows are `external`.
3. **bg on mac** — the hub sees them through `claude agents --json` over SSH (`reconcile.rs:1057`),
   keyed `bg:<claude_session_id>`, `kind external`; they become ghosts because the Mac reboots
   daily and the `host_reboot` TTL exemption ignores kind. F6.
4. **`-term` shells** — `unknown` is the accident of `claude_status NULL` on `shell` rows; oci's
   shell even says `idle`. Exclude shells. F8.
5. **Hook vs reconcile** — all hosts have `UserPromptSubmit`; the in-flight guard
   (`last_hook_at >= probe start`) is correct; the problem is the opposite: nothing ages
   `working` out (F2). `context_red = 5` is all transcript-sourced and fresh — trustworthy.
   `idle_since` far in the past (e.g. 21420, 09-18, `turn_seq 0`) is a resumed session whose
   hooks never fired; harmless but it makes the GC `work_idle` clock start early.
6. **429** — not surfaced, no retry, looks idle. F3.
7. **Controller** — a settings pair on the hub written by `register_self`; the operator holds
   it; the desktop's copy is stale and unused in hub-client mode. F9.
8. **Attention** — 3 ghosts flagged, the 5 context-red / 2 stale-working / 1 rate-limited /
   4 CI-failing rows are not. F7.

---

## Prioritised list

| # | Sev | Finding | Effort |
|---|-----|---------|--------|
| F1 | P0 | `oom` detector matches prose; playbook kills a live session, no attempt budget, episode restarts on re-detect | M |
| F2 | P1 | `working` never ages out (2 rows stale ~40 h) | M |
| F3 | P1 | StopFailure/429 recorded as plain `idle`, no attention, no retry | S |
| F4 | P2 | ghosting keeps stale `claude_status`/`blocked` | S |
| F6 | P2 | `external` ghosts kept 14 d by a TTL meant for resumable rows; Mac reboots daily | S |
| F5 | P2 | hidden `local` host: no pass ⇒ immortal ghosts, duplicated on `mac`; `ghosts=3` vs 19 | S–M |
| F7 | P2 | attention flags only ghosts; no context/stale/failed/CI reasons | S–M |
| F8 | P3 | shells counted in `by_status`, get a pane-derived status | S |
| F9 | P3 | controller = unguarded last-writer settings pair; stale desktop copy | S |
| F10 | P3 | `status_change` same-value spam, `stuck` per detection, timeline lost on reap | S |

Quick wins in one PR: F3 + F4 + F6 + F8 (all S, all in `store/reconcile.rs` / `hooks.rs` /
`health.rs`), then F1's playbook guard (S) before the detector rewrite.
