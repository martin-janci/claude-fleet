# Data correctness / data sync review — claude-fleet live instance (2026-09-27)

Scope: does the data the hub (`fleet-hub` 0.3.1 at fleet.rlt.sk) serves, and the desktop
(0.2.42, hub-client mode) mirrors, match reality? Evidence from the repo at
`crates/fleet-core/src/...` (worktree `ux-errors-analysis-prep-f5c1e7`, branch
`feature/instance-db-agents-analysis-0e4558`, **371 commits behind origin/main** — where a
line differs on main it is called out), from hub MCP tools, and from the desktop sqlite
snapshot `scratchpad/db/state.db`. Read-only throughout; nothing was killed, deleted or sent.

## 0. A correction to the brief: the MCP token in this session is a per-host token bound to `mac`

Evidence: `usage_report {host_alias:"claude-fleet-trn"}` → `E_FORBIDDEN: the requested host is on
host claude-fleet-trn; this token is bound to mac`; `usage_report {}` echoes `"host_alias":"mac"`
although no host was requested (`mcp/tools/support.rs:995-1010 usage_scope`: `caller.host_alias`
wins over the request). Consequences for the brief's readings:

- `fleet_health.usage_by_host` and `usage_by_day` are host-scoped by
  `service/health.rs:167-172 scope_usage_to_host` (called from `mcp/tools/fleet.rs:17-21`),
  so they show `mac` only **by design of the caller**, not because the hub collects nothing
  elsewhere (see finding 4).
- Everything else (list_hosts, list_sessions, projects, worktrees, session_history) is
  fleet-wide for this token, so the other findings are unaffected.

---

## Finding 1 — `claude_version` is frozen at add/re-probe time; the reconcile tick writes the stored value back

**Severity:** P2 (wrong data on 3 of 5 hosts; misleads "which host is up to date" decisions and any
version-gated feature). **Effort:** S.

**Evidence**
- Only two writers of a *fresh* version: `service/hosts.rs:400-437 probe_with_token` (script at
  `hosts.rs:376`: `tmux -V … claude --version`) used by `add_host`/`probe_host`, and
  `hosts.rs:489 probe_local_in` for `local`.
- The per-tick pass never runs `claude --version`: the reconcile probe scripts
  (`tmux.rs:254 HOST_IDENTITY_SCRIPT`, the tmux snapshot script) collect boot id, tmux, panes, agents —
  no version. Then `service/sessions/reconcile.rs:765` and `:878` pass
  `claude_version: host.claude_version.as_deref()` — the **stored row's** value — into
  `Store::apply_host_reconcile` → `store/reconcile.rs:188-203 update_host_probe_in_tx`
  (`UPDATE hosts SET reachable=?1, claude_version=?2 …`). Same on origin/main
  (`reconcile.rs:599`, `:867`).
- Live: list_hosts says mac 2.1.235 / mefistos 2.1.234 / oci 2.1.220 while the binaries are
  2.1.282 / 2.1.267 / 2.1.282; `last_pinged_at` = 1790506687 (just now) for all, i.e. the row is
  "fresh" but the version column is from provisioning day (Sep 20).

**Root cause**
`update_host_probe` is the single reachability setter and takes versions as parameters; reconcile
reuses it with the old values so the row is not blanked. Nothing schedules a version re-probe.
The agent host (`trn`, 2.1.277) is the same: value from the initial probe over the agent transport.

**Proposed fix**
- Add `claude --version 2>/dev/null` (and `tmux -V`) to the tmux snapshot script (`tmux.rs`, behind a
  marker like `@@versions`), parse it in `parse_sessions_checked`, and pass `Some(fresh)` into
  `HostReconcile`; fall back to the stored value when the marker is missing (an older agent).
  `claude --version` is a node start (~0.3-1 s); if that is too much per 20 s tick, refresh every
  N-th pass or when `last_pinged_at - versions_at > 1h` (new column `versions_at`).
- Cheaper stopgap: `probe_host` for each host after `provision` and from a daily tick.
- Data correction today: `probe_host {alias}` for mac, mefistos, claude-fleet-oci (allowed with a
  master token; this per-host token can only re-probe `mac`).

---

## Finding 2 — Duplicate `bg:<uuid>` rows across `local` (hidden) and `mac`; `local` ghosts frozen at `working`; nothing will ever reap them

**Severity:** P1 (the same agent counted twice, dead rows shown as `working`, the operator's
"stop the agent first" trap, and per finding 5 the whole `local` → `mac` rename left 249 worktree
rows stranded). **Effort:** S for the data correction, M for the code fix.

**Evidence**
- The hub DB is a copy of the desktop DB: the snapshot's `max(sessions.id)` = 21498 and its
  `local` rows are ids 21486, 21488, 21490, 21491, 21495, 21498 — **the same ids** the hub serves
  for `local` (`list_sessions {host_alias:"local", include_lost:true}`). `docs/hub.md:1683`
  ("Copy its state.db into the hub's data dir") is the documented migration path.
- In the snapshot those rows are `kind='external'`, `status='running'`, `claude_status`
  working/idle, `last_hook_at` ≈ 1789928xxx (Sep 20), `lost_at NULL`.
- On the hub they are `status=ghost`, `lost_at=1790431981` (2026-09-26 14:13Z), and
  `session_history 21486/21490` shows `kind:lost detail:local_disabled` — written by
  `service/hub.rs:216-241 retire_local_sessions` (origin/main, commit `6f47861b`, shipped in 0.3.1),
  which calls `mark_host_sessions_lost("local", "local_disabled", &[], now, 0)`.
  `claude_status` is untouched by that path (`store/sessions.rs:332-337` sets only
  `status/lost_at/lost_reason`), so three of them still say `working` from Sep 20.
- Same machine, second alias: `mac` rows 21504 (`bg:cb0bab25…`), 21505 (`bg:11d6176e…`), 21507
  (`bg:a0336855…`) were created after the copy (ids > 21498) when the Mac was added to the hub as
  ssh host `mac`; reconcile's `claude agents` probe upserts by `(host_alias, tmux_name)`
  (`sessions` UNIQUE, `store/sessions.rs:106-151 upsert_bg_session`), so the same agent under two
  aliases is two rows. Those `mac` rows carry `lost_reason=host_reboot` (Sep 21 09:12Z) and a
  `claude_session_id`, so they are TTL-exempt from the reap for 14 days
  (`reconcile.rs:55-76 DEFAULT_LOST_TTL_SECS = 1_209_600`, `store/reconcile.rs:550-596`).
- Why the `local` ghosts never go away: Phase-2 hard delete runs only inside a probed host's apply
  (`store/reconcile.rs:520-640 ghost_and_clean`, pane-less variant called from
  `service/sessions/reconcile.rs:1150 ghost_and_clean_bg_sessions(host_alias…)`), and `local` is
  filtered out of the probe fan-out on a hub (`reconcile.rs:1406` `deps.local_host || h.alias != "local"`,
  `:1428` `!h.hidden`). `docs/hub.md:1708-1711` ("they stay dismissable and are pruned like any other
  ghost") is therefore wrong on the "pruned" half. GC (`service/gc.rs:98-124 plan`) skips ghosts and
  unreachable hosts; work tidy (`service/gc/tidy.rs`) never deletes session rows.
- The "dismiss" trap: `service/bg_sessions.rs:297-330 dismiss_agent_session` refuses
  `kind != 'bg'` ("only background agents can be removed") and `claude_status == working`
  ("stop the agent first") — both true for the `local` externals, so the UI's agent-list dismiss
  cannot remove them. `dismiss_ghost_session` (`service/sessions/lifecycle.rs:1422-1440`) checks
  only `status == 'ghost'` and does `delete_session`, so it *does* work — but only with a master
  or client token (`resolve_target` host-scopes; this `mac` token gets E_FORBIDDEN on a `local` row).
- Mac reboots are real: `sysctl kern.boottime` = 2026-09-26 11:09:47Z, loss stamp 11:10:34Z,
  `uptime` 23:55 — the `host_reboot` verdicts on `mac` are correct, not a boot-id artefact.

**Root cause**
Rows are keyed by host *alias*, not by machine identity (`boot_id`/hostname). Renaming a host in
practice means "copy the DB, hide `local`, add the machine again under a new alias", and no code
path migrates or merges the old alias's rows (sessions, worktrees, fingerprints, dismissals,
usage_daily). The 0.3.1 self-heal ghosts the sessions but never reaps them and leaves their
`claude_status`.

**Proposed fix**
Code (M):
1. `retire_local_sessions` (hub start) should also run the Phase-2 reap for `local` (call
   `ghost_and_clean` for both kind filters with an empty keep set and `lost_ttl_cutoff=None`), and
   clear `claude_status`/`stuck_kind`/`current_activity` when it ghosts a row (a ghost has no live
   status; `summarize` would otherwise keep counting a dead `working`). Alternatively make the tick
   reap hidden hosts' ghosts once per pass.
2. A real rename: `fleet-hub host rename <old> <new>` (and/or `rename_host` tool, admin-fenced) that
   rewrites `host_alias` in `sessions`, `worktrees`, `worktree_parent_fingerprints`,
   `dismissed_agents`, `usage_daily`, `host_layers`, `catalog_secrets_host`, `hosts.alias`, with
   `ON CONFLICT` merge for `(host_alias, tmux_name)` duplicates (keep the newer row, sum usage into
   `usage_daily`).
3. Dedupe by machine identity: when a probe of host B returns the `boot_id` stored for host A,
   log/flag it in `fleet_health` (`hosts_duplicate`) instead of silently double-counting.

Data correction now (S, no SQL, master token from the desktop or the phone client):
`dismiss_ghost_session {session_id}` for 21486, 21488, 21490, 21491, 21495, 21498. They are
duplicates (3) or dead Sep-20 agents (3); `delete_session` also journals their conversations
(`trg_work_journal_session_delete`) so nothing of value is lost. The `mac` ghosts (21504/5/7,
21510, 21511, 21674, 21711, 21716, 21717, 21631, 21706, 21541) reap themselves 14 days after their
`lost_at`, or sooner via `dismiss_ghost_session`/`restore_host_sessions`; leave them unless they
clutter.
SQL equivalent if the hub DB is ever edited directly (backup first, hub stopped):
`DELETE FROM session_events WHERE session_id IN (SELECT id FROM sessions WHERE host_alias='local'); DELETE FROM sessions WHERE host_alias='local';`
(`session_events` has no FK cascade — `store/reconcile.rs:645-663` deletes it by hand; the triggers
handle read_cursors and the work journal).

---

## Finding 3 — `fleet_health.ghosts=3` vs 19 rows with `lost_at`: `kind='external'` rows are excluded from every session count

**Severity:** P3 (both numbers are "right", the contract is undocumented and surprising).
**Effort:** S.

**Evidence**
- `service/health.rs:100-128 summarize`: `for s in sessions.iter().filter(|s| s.kind != "external")`
  — `sessions_total`, `by_status`, `ghosts` (`s.status == "ghost"`), `context_red`, `stuck` all skip
  external rows; only `usage_by_host` (line 128) sums every row.
- Every `bg:<uuid>` row is `kind='external'` (full row 21720: `"kind":"external"`; snapshot: all 6
  `local` bg rows external). Arithmetic: 63 rows − 20 bg rows = **43 = sessions_total**;
  19 lost − 16 bg ghosts = **3 = ghosts** (= `dev-…bright-vega` 21631, `dev-martin-janci-claude-fleet`
  21706, `fleet-probe` 21541, all `kind=work`).
- `list_sessions` has no such filter, so the two tools disagree by construction. The tool
  description (`mcp/tools/fleet.rs:8-10`) does not mention it; `health.rs:94-99` documents the
  intent ("not fleet work and must not raise its blocked / stuck roll-ups").

**Root cause / what each means**
`ghosts` = lost *fleet-launched tmux* sessions; `list_sessions include_lost` = every lost row
including observed Claude Desktop/Code agents. Neither is wrong; the roll-up silently ignores the
5 running mac agents too (one of them, 21720, is `working` at `context_pct 85` — it is not in
`context_red`, and `by_status.working=4` omits it).

**Proposed fix**
Add `external_total` / `external_ghosts` (or `by_kind`) to `Health` and say "fleet-launched
sessions only" in the tool description and `docs/control-api.md`; keep the existing counters'
semantics for compatibility.

---

## Finding 4 — `usage_by_host` lists only `mac`: caller scoping, not a collection gap (but unverifiable from this token)

**Severity:** P3 as a data issue (none found); P2 as an observability/UX issue (a per-host caller
gets a fleet-wide `usage_by_day` label with host-only numbers and no hint). **Effort:** S.

**Evidence**
- Scoping: finding 0. `usage_report {}` returns 16 sessions, all `mac`, `"host_alias":"mac"`.
- Collection on the hub visits every reachable host regardless of transport:
  `service/usage.rs:648-665 collection_hosts` (reachable hosts, `local` only when
  `hub.local_host`), `:670-692 collect_all` → `collect_host` → `run_script(exec, host, …)`; `exec`
  is the `SshClient` built with a `HostRouter` (`ssh.rs:113-126`) that sends a `transport='agent'`
  host's commands through the connected agent. Scheduled from the tick (`service/tick.rs:110
  usage::spawn_collect`), which `fleet-hub/src/serve.rs:811` starts.
- The desktop snapshot (which ran the same collector before hub mode) has `usage_daily` for all
  five hosts: trn 10 days ≈ 23,426 USD-est., local 10 days ≈ 2,353, mefistos 7 days ≈ 1,089, oci 66,
  htz 6 — so transcript collection over SSH and over the agent transport works.
- Per-day vs per-host mismatch (by_day Sep-21 = 849.6 USD vs by_host mac lifetime 505.3 USD) is the
  documented difference: `by_host` sums *existing* rows (`usage.rs:792-802 per_host_totals`), by_day
  is the durable `usage_daily` roll-up that keeps reaped sessions (`usage.rs:805-828`).

**Root cause**
The brief assumed a master token. The hub does not surface *which* scope a report is in.

**Proposed fix**
Include `"scope": {"host_alias": "mac", "reason": "per-host token"}` in `fleet_health` and
`usage_report`, and list the hosts the last collection pass actually reached (`usage_collected_at`
per host) so a real collection gap becomes visible. To verify other hosts today: run `usage_report`
with the master token (desktop in hub mode or `fleet-hub` CLI).

---

## Finding 5 — The hidden `local` host: nothing probes it, but 249 worktree rows (and the project tree) are stranded on it; `mac` has 0 worktree rows

**Severity:** P1 (every worktree-aware feature is blind on the Mac: `list_worktrees {host_alias:"mac"}`
→ `total:0`; project 13's seven worktrees are all `host_alias:"local"`; mac sessions carry
`project_id` but no `worktree_id`; `delete_worktree` on any of the 249 rows is refused with
`E_NOTFOUND "host local is disabled"` via `validate.rs:85-91 → hub::ensure_local_allowed`).
**Effort:** S (data), M (code).

**What still touches `local`** (all skip it — correct):
- reconcile: `reconcile.rs:1406`, `:1428` (not in the fan-out; not probed).
- usage collection: `usage.rs:651-665` (`local_enabled=false` on this hub).
- account-usage poll: `account_usage.rs:616-625` (`reachable && !hidden`).
- GC: `gc.rs:110-124` (unreachable hosts skipped); worktree prune: `worktree_prune.rs:400-403`
  (`alias != LOCAL_HOST && reachable && !hidden`); repair tick: `repair_tick.rs:408-411`.
- tunnels: hub reports `tunnels: {}`; the desktop's old tunnel loop (WARN lines through Sep 19) is
  pre-hub-mode and now skipped ("remote backend: skipping … the embedded control API").
- One residual: hooks called with the **master** token attribute paths to `local`
  (`service/hooks.rs:1172-1174 caller_host`, `:625 row_cwd`), which on this hub would create or
  match `local` worktree rows. Harmless as long as every host uses its own host token (they do:
  `fleet-hook.headers` per host), but a master-token hook call is a foot-gun on a local-less hub.
- On this branch `refresh_projects` still refuses (`projects.rs:198`); origin/main `6f47861b`
  returns the stored tree instead — so the hub can never rescan or heal the copied local tree.

**Root cause**
Same as finding 2: the rename is really "hide the old alias, add a new one"; worktree rows (source:
the desktop's local scan and `EnterWorktree` hooks) were never re-homed, and worktree rows for
`mac` will only appear through hooks on `mac` (`hooks.rs:1271 upsert_worktree_on(host…)`) — none
has (0 rows), likely because those sessions are Claude Desktop agents whose hook calls
resolve by pane/id and never carry a worktree event.

**Proposed fix / correction**
- Migrate, do not delete, the worktree side: `UPDATE worktrees SET host_alias='mac' WHERE host_alias='local'`
  (UNIQUE `(project_id, host_alias, name)` cannot collide — `mac` has none), same for
  `worktree_parent_fingerprints`, `dismissed_agents`, `host_layers`/`catalog_secrets_host` if any,
  and `usage_daily` (`INSERT … ON CONFLICT(day,host_alias) DO UPDATE SET … = … + excluded…`).
  Then `worktree_prune` (which now visits `mac`) removes the checkouts that no longer exist, and
  `list_host_worktrees {host_alias:"mac"}` / hooks keep them current.
- Delete the session side (finding 2). Keep the `local` host row hidden (or `remove_host` once
  its rows are gone — `store/hosts_accounts.rs delete_host` cascades nothing, so migrate first).
- Code: the `host rename` command from finding 2 does exactly this transactionally.

---

## Finding 6 — Projects cruft: the hub holds the desktop's one-time local scan and has no prune path

**Severity:** P2 (77 project rows, 12 duplicate worktree paths, six zombie `ppt-epic-*` rows,
and nothing on a local-less hub can ever remove any of them). **Effort:** S–M.

**Evidence**
- Discovery = `service/projects.rs:195-370 refresh_projects`: scans the *local* projects root, upserts
  projects/worktrees, and deletes (a) duplicates whose checkout is owned by a rediscovered project
  (`:305-330`) and (b) not-rediscovered rows **outside the root** (`:334-368`). A row inside the
  root whose directory is gone is never deleted (only "outside" rows are; `ppt-epic-145..150`,
  `base_path /Users/martinjanci/projects/github.com/martin-janci/ppt-epic-14x`, `adopted=0,
  system=0`, 0 worktrees, `last_session_at NULL` in the snapshot — these are exactly that case).
- Remote hosts never create or delete project rows: `worktree_prune.rs` removes *worktree* rows
  only (`:10-31`), `list_host_worktrees` reads. `delete_project*` is reached only from
  `refresh_projects` and `bg_sessions.rs:334 purge_project` (which also deletes Claude state on
  hosts — not a tidy tool). No `remove_project`/`forget_project` MCP tool exists (tool list checked).
- On the hub, `refresh_projects` is refused (this branch) or a read (main), so the 77 rows are the
  desktop's scan of Sep 20 frozen forever; the 12 duplicate paths under `local`
  (`sales-twins-app/.claude/worktrees/*` registered under projects 59/61/62 = linked worktrees
  scanned as repos) would be healed by `dedupe_by_common_dir` (`projects.rs:178`) only on a local
  refresh.
- `fleet/operator` (id 82) is `system=1` (`service/operator.rs`), 0 worktrees by design — not cruft.
  Single-worktree rows are ordinary repos with just `main` — a presentation problem, not data.

**Proposed fix**
- `refresh_projects`: also `delete_project_if_unused` for a not-rediscovered row **inside** the root
  whose `base_path` no longer exists on disk (bounded, explicit, logged like the "outside" case).
- Add `refresh_projects {host_alias}` for remote hosts (scan over SSH/agent with the same
  `scan_projects` shape) or at least a `forget_project {project_id}` admin tool that calls
  `delete_project_if_unused` — the only way a local-less hub can ever shrink its project list.
- Data correction now: none possible through the API from a local-less hub; with the DB stopped:
  `DELETE FROM projects WHERE id IN (29,30,31,32,33,34)` (no worktrees, no sessions reference them —
  verified in the snapshot; re-verify on the hub with `list_worktrees {project_id}` = 0 and
  `list_sessions {project_id, include_lost:true}` = []).

---

## Finding 7 — Other integrity results (desktop snapshot, `PRAGMA foreign_keys` enforced at `store/schema.rs:581`)

| Check | Result |
|---|---|
| sessions.worktree_id → worktrees, sessions.project_id → projects, worktrees.project_id, sessions/worktrees.host_alias → hosts, conversations/participants/read_cursors/tasks/work_links → sessions/items | **0 orphans** (FKs on; `session_events` has none but 0 orphans) |
| `status='ghost'` xor `lost_at` | 0 rows inconsistent |
| worktree host or project ≠ session's | 0 |
| **duplicate `(host_alias, path)` worktree rows** | **12** (all `local`, projects 59/61/62 vs each other — see finding 6) |
| **session_messages orphan** | **1** (id 1: from 1919 to 1864, both sessions gone; no FK, Phase-2 only deletes by `to_session_id` → messages *from* a reaped session leak) — P3, add `from_session_id` to the Phase-2 delete or an FK with `ON DELETE CASCADE` |
| sessions with project but no worktree_id | 37 / 40 (bg + shell rows never get one; work graph/safe-kill rely on `worktree_key`) — P3 |
| conversations with `ended_at NULL` | 25 (the 6 `local` externals will never close — reaping the row journals them) |
| hosts row for `local` | `reachable=1`, all `last_pinged_at` = 1789930333 — pre-hub snapshot, consistent |

Hub-side (from tools): `sessions_total 43 / ghosts 3` consistent with finding 3; `hosts_reachable 5/6`
= the hidden `local`; `stuck 0`; no tracker rows.

---

## Prioritised list

1. **P1 (finding 5+2, data, S):** re-home the Mac: `UPDATE worktrees … 'local'→'mac'` (+ fingerprints,
   dismissals, usage_daily merge), then `dismiss_ghost_session` × 6 `local` ghosts with a master
   token. Restores worktree visibility on `mac` and removes the double-counted agents.
2. **P1 (finding 2, code, M):** `host rename` / alias merge keyed on machine identity; have
   `retire_local_sessions` reap (not only ghost) `local` rows and clear `claude_status` on ghosting.
3. **P2 (finding 1, code, S):** collect `claude --version` in the reconcile probe (or a periodic
   re-probe) so `list_hosts.claude_version` is live; re-probe the three hosts now.
4. **P2 (finding 6, code, S–M):** `refresh_projects` deletes vanished in-root rows;
   `forget_project` (or remote `refresh_projects`) so a local-less hub can prune; drop
   `ppt-epic-145..150`.
5. **P2 (finding 4, code, S):** make the caller scope explicit in `fleet_health`/`usage_report`
   and report per-host `usage_collected_at`; verify other hosts' usage with the master token.
6. **P3 (finding 3, docs+API, S):** document "fleet-launched sessions only" and add external
   counters to `fleet_health`.
7. **P3 (finding 7, code, S):** delete `session_messages` by `from_session_id` too in Phase 2 (or
   FK cascade); consider an FK on `session_events`.
