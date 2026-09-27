# claude-fleet live instance — performance / reliability / observability review

Date 2026-09-27. Read-only. Sources: desktop logs (71 hourly files, 2026-09-19-19 … 2026-09-27-08, 520 KB), the sqlite snapshot `scratchpad/db/state.db` (stale since 2026-09-25), the worktree code on `feature/instance-db-agents-analysis-0e4558`, and the hub over MCP (`fleet_health`, `usage_report`, `session_history`).

**Correction to the brief before anything else.** The MCP token this session holds is **host-bound to `mac`**, not the master: `usage_report {host_alias: mefistos}` answers `E_FORBIDDEN: … this token is bound to mac`, and an unscoped `usage_report` comes back with `"host_alias":"mac"`. Every fenced number in the brief (`usage_by_host` only `mac`, `ghosts: 3` vs 19 `lost_at` rows, 43 sessions) must be re-read as *what a mac-bound caller is allowed to see*, not as the fleet. Finding 6c below depends on this; other reviewers' conclusions built on "master token" should be re-checked.

Log level totals for the window: **1859 WARN, 273 INFO, 5 ERROR** (the brief's "195 WARN, no ERROR" undercounted by 10×; it read the tails file, not the files).

---

## 1. Tunnel supervisor restart loop — P2 (historical; fixed on main, residual observability gap)

**Evidence**

- Restart lines per hour (`grep 'tunnel] ssh exited'` over the files):
  - `claude-fleet-trn exit_code=0 restart_in=30s`: 120 / 119 / 119 / 117 / 120 / 118 / 49 per hour, 2026-09-19 19:00 → 2026-09-20 01:xx — **776 restarts**, one every 30 s for ~6.5 h.
  - `mefistos`, `claude-fleet-oci`, `claude-fleet-htz` `exit_code=255`: a 1/2/4/8/16/30 s ramp at 22:xx then ~115/h each → **293 / 287 / 287**.
  - Total 1,643 of the 1,859 WARN lines (88 %) in the whole 8-day window come from this one loop in its first 6 hours.
- The build that wrote them was **0.2.23** (`claude-fleet starting version="0.2.23"` on 09-19). The snapshot's `hosts` table shows `claude-fleet-trn transport=ssh` — trn was still an SSH host then (the agent connected only on 2026-09-26, `connected since 1790454247`), so the desktop legitimately supervised a tunnel to it.
- Root cause is documented in commit `1eb18513` (2026-09-20, `fix(tunnel): stop the reverse-tunnel restart loop and surface why it fails`): `tunnel_argv` passed no `ControlMaster=no`, so the user's `ControlMaster auto` turned `ssh -R` into a multiplexed slave that handed the forward to the master and **exited 0 in ~0.2 s, forever**; the supervisor treated any exit as a crash; 255s were orphaned `ssh -R` processes holding the remote port ("address already in use") after ungraceful restarts; stderr went to /dev/null so nothing said why.
- Current `tunnel.rs`: `HEALTHY_AFTER` 60 s, backoff 1→30 s jittered and reset after a healthy run, orphan reap before the first attempt and after a bind conflict, stderr captured, and `is_loud()` demotes restarts to DEBUG after the 3rd consecutive failure (`crates/fleet-core/src/service/tunnel.rs:42`).

**Is the loop running on the hub today?** No, and `tunnels: {}` is genuine "none configured":

- `provision.rs:295-298` — a tunnel is only ensured when `host != "local" && !base.public && !routes_to_agent(store, host)`.
- `provision.rs:441-454 reestablish_tunnels` — returns early for `base.public`, and skips `transport == "agent"`.
- `fleet.rlt.sk` is a public hub, so the supervisor has no tasks and `Health::set_tunnels` (`service/health.rs:82`) publishes an empty map with `tunnels_flapping: 0`.

**Residual gap.** `fleet_health` cannot distinguish "public hub — tunnels not applicable" from "loopback hub — nothing supervised" from "supervisor not attached". A reader (and the brief) had to go to the source to answer it.

**Proposed fix.** Add `tunnels_mode: "public" | "loopback"` (or `tunnels: null` when not applicable) to `Health`; nothing else — the loop itself is fixed. Effort **S**.

---

## 2. `session_events` churn (status_change = 91 %) — P2 historical, P3 residual

**What writes it.** `service/sessions/reconcile.rs:800-834` ("Task G"): after each host's write commits, every prior row is read back with `get_session` and a `status_change` is inserted only when `row.claude_status != prior.claude_status`; `stuck` only when a non-None kind changes. The detail stored is the **new** status (`row.claude_status.as_deref()`), so `detail IS NULL` means "changed to unknown".

**Quantified from the snapshot** (2,948 rows, 21 sessions, 2026-07-14 → 2026-09-20 18:38; the table is frozen because the hub owns the fleet since 09-20):

| transition (prev → new) | rows | first | last |
|---|---|---|---|
| NULL → NULL | **2,007** | 2026-08-17 | 2026-09-11 17:44 |
| busy → busy | 372 | 2026-08-17 | 2026-09-11 17:40 |
| working → working | 164 | 2026-09-11 19:34 | **2026-09-20 18:38** |
| working → idle / idle → working | 30 / 29 | | |

- 543 of 2,674 status_change rows (20 %) are consecutive duplicates; 2,083 gaps are 5–30 s, i.e. one per 20 s tick.
- Noisiest sessions: 240 `…property-management--dispatchers` (htz), 246 `openmarket-ai` (htz), 20775 `e1` (mefistos) — **exactly 500 rows each, all NULL→NULL, 183.7 / h (= every 19.6 s)** between 2026-09-11 15:01 and 17:44. 500 is `SESSION_EVENTS_CAP` (`store/rows.rs:1037`): the cap is what stopped them, not a fix.
- The phantom-event fix is `188b8dcb` (2026-09-11 15:47 +0200, `fix(reconcile): phantom status events…`); events stop at 17:44 UTC, the next restart. After 09-12: 247 status_change rows, **0 NULL**, but **164 `working → working`** continue up to the last snapshot minute. Two writers set `claude_status` — the hook route (`/hook` Stop/UserPromptSubmit) and reconcile's pane intel — and only reconcile records an event, so hook→idle, pane→working, hook→idle, pane→working yields `working, working` with no `idle` between. The hub's own history for a live session today (21719) shows only `turn_done`/`notification`, so the hook path dominates now, but the disagreement remains latent.

**Cost of the churn.** `store/timeline.rs:100-112` runs, on **every insert**, `DELETE … WHERE session_id=?1 AND id NOT IN (SELECT id … ORDER BY at DESC, id DESC LIMIT 500)`. The snapshot's `max(id)` is **2,118,642 for 2,948 live rows** — ~2.1 M inserts + trims over the desktop's life, most of it the phantom loop; the hub inherited that counter (its ids are ~2,152,7xx today). Reads are cheap: `idx_session_events_session (session_id, at DESC)` covers `session_history`, and the cap bounds the per-session scan. Retention: 500 per session, cascade delete on session delete (`store/reconcile.rs:648`, `projects.rs:560`, `hosts_accounts.rs:419`); no age-based GC (fine, bounded).

**Proposed fix.**
1. Record the *transition* (`prev→new`) or dedupe on insert: skip when the newest event for the session has the same `kind` + `detail` (one indexed row read). Effort **S**.
2. Make the hook route record `status_change` too (or make reconcile compare against the last *event*, not the prior row) so the two writers cannot alternate silently. Effort **S/M**.
3. Move the trim from per-insert to "every Nth insert" or to the GC sweep — it is a correctness no-op and a write-amplifier. Effort **S**.

---

## 3. Reconcile tick cost on the hub — P3 (design is sound; one scheduling flaw)

**What one pass does** (`reconcile.rs:1367-1476`, `tick.rs`):

- Tick every **20 s** (`DEFAULT_RECONCILE_INTERVAL_SECS`, `reconcile.rs:36`), `MissedTickBehavior::Skip`, a `ReconcileGate` so passes never overlap.
- Hosts fan out in a `JoinSet` (one task per non-hidden host). Per host, **one SSH round trip**: `probe_snapshot_script` (`tmux.rs:495-502`) runs `tmux list-sessions` and `capture-pane -S -8 -p` for every session in a single shell script; identity/boot-id and account come from the same snapshot. `PANE_TAIL_LINES = 8`.
- Extra round trips per host, all batched: `list_claude_agents` only when `agents_due` (`AGENTS_CADENCE`), `transcript_mtimes` (one script for all bg ids), and the PR probe (`probe_pr_info`, cached per session, `PR_PROBE_BATCH` per pass, 20 s timeout). **No per-session SSH round trip anywhere.**
- Writes: after *all* hosts have joined, one `lock(store)` per host for `reconcile_write_one_host`; inside it, `get_session` per prior row (N local SQLite point reads under the lock — microseconds). `session:updated` is emitted only when the row differs (`store/reconcile.rs:512`), so an unchanged fleet costs the desktop nothing (BE-11/FE-10 already done).
- Same tick also runs: playbooks, `gc::maybe_sweep`, `usage::spawn_collect` (gated at 300 s), task sweep, error-report drain/sweep, repair tick, worktree prune.
- `list_sessions` (`reconcile.rs:1520-1535`) is cache-first: it serves stored rows unless the last pass is older than the interval window; `force` (the UI Refresh, `refresh_sessions`) runs a full pass first.

**The flaw.** `HOST_PROBE_TIMEOUT` is **65 s** against a 20 s tick, and the pass waits for `join_next` on *every* host before writing *any* host. The logs show it happening: `[reconcile] host probe exceeded its wall clock; marking unreachable (last-known sessions kept)` — **mefistos 18, htz 16, trn 15, oci 10, local 4** in the window. One slow host (htz at 98 % disk, or a stalled SSH ControlMaster) freezes the freshness of all five hosts for up to 65 s, and because the gate is busy the next 2–3 ticks are skipped; hub clients see the whole fleet stale, not one host. Transcript reads for `context_pct`/usage are not in the pass (context comes from the pane; usage is the separate 300 s collector), so they are not the cost.

**Proposed fix.** Write each host's probe as it completes (stream `join_next` → `reconcile_write_one_host` immediately) so a slow host only delays itself; consider lowering `HOST_PROBE_TIMEOUT` toward 30 s now that the probe is one script; expose pass duration and per-host probe time (see §5). Effort **S** (streaming writes), **S** (timeout), **M** (metrics).

---

## 4. Hub-client desktop event stream — P2

**Reconnect / backoff** (`src-tauri/src/backend/events.rs`): `FIRST_BACKOFF` 1 s → `MAX_BACKOFF` 30 s, jittered, reset once a connection has delivered a row; `IDLE_TIMEOUT` = 2.5 × the hub's 15 s keep-alive = **37.5 s**; `OPEN_TIMEOUT` 30 s; `LAGGED` frame → end of stream + relist. Observed in the window: 25 × "the hub closed the event stream", 18 × "went silent … silent_for=37.5s", ~40 × "502 to GET /events" (hub restarts/upgrades behind cloudflared), 9 × "Connection reset by peer", 10 × "connect timed out". Sane.

**Gap handling — the bug.** The hub keeps a **512-event replay ring with a 15-minute grace** (`events.rs:548-559`, `REPLAY_RING`, `RING_GRACE_SECS`) and `handle_events` honours `Last-Event-ID` / `since` (`events_route.rs:545`). The desktop **never sends it**: `open_stream` writes only `Authorization`, `Accept`, `Cache-Control` (`backend/events.rs:844-847`), and every `[hub events] subscribed` line in the log says `"resumed":false`. So every reconnect — dozens per day in this window — falls back to `resync()` (`backend/events.rs:711-786`): `list_sessions(false)` + `list_hosts` + `list_tasks` + `list_accounts`, then `session:updated` for **every** row (63 incl. lost) and a `session:killed` diff. Worse, resync does **not** re-list projects, worktrees, work items, account usage or asset inventory, so any event of those kinds that fell in the gap is lost until the next full page load — a hub restart leaves the worktree/project sidebar stale.

**Frontend coalescing** (`src/lib/events.ts`): a 16 ms batch (`ROW_EVENT_FLUSH_MS`) groups events into one store flush per kind, but inside a batch each `session:updated` still calls `onSessionUpdated` per event — two updates of the same id in 16 ms are two merges, not one. Acceptable at 43 rows; the resync burst is the only case that hits it hard.

**Two starts today (08:51 / 08:53).** `Backend::resolve` runs synchronously **inside Tauri's setup closure on the main thread** (`src-tauri/src/lib.rs:199`) and reads the client token from the keychain through `security_framework` with **no timeout** (`backend/token_store.rs:76-84`; `backend/mod.rs:548-549` even warns that "a locked keychain then prompts or hangs at startup"). Gap between `claude-fleet starting` and `remote backend: skipping…` per launch: 0.03–0.08 s normally, but **41 s** (09-25 18:24:58→18:25:39), **2 m 41 s** (09-26 00:09:10→00:11:51, followed by a relaunch at 00:13:13), and **11 h 44 m** (09-26 21:07:37 → 09-27 08:51:53 — the Mac slept with the prompt pending; the user relaunched at 08:53:44, which resolved in 10 s). Nothing is logged between the two lines, so the operator sees a dead app and restarts it. No `sent SIGTERM to prior instance` today → the first instance had already been quit.

**Proposed fix.** (a) Send `Last-Event-ID` from the last frame id and only `resync()` when the `ready` frame says `resumed:false`; add projects/worktrees to resync. Effort **S**. (b) Log `resolving backend (keychain)…` before and the elapsed time after; move the keychain read off the main thread with a bounded wait and show the window in an "unlocking keychain" state. Effort **M**.

---

## 5. Logging and observability gaps — P2

**What is logged at which level** (default filter `warn,claude_fleet_lib=info,claude_fleet=info,fleet_core=info,fleet_hub=info`, `logging.rs:52`):

| failure | level | note |
|---|---|---|
| tunnel `ssh` exit 255 / 0 | WARN → DEBUG after the 3rd, every 20th WARN | fixed by `is_loud` |
| host probe wall clock (65 s) | WARN | no duration, no reason |
| usage collection failure per host | **DEBUG** (`usage.rs:691`) | invisible with the default filter |
| stuck playbook applied (oom loop 8× on 21480) | INFO `applied 1 stuck playbook(s)` | no host/session/kind |
| 429 rate-limit `stop_failure`, `auth_menu`, `reconnect` | **not logged** | only a `session_events` row |
| session lost / deleted | INFO | good, structured |
| `rmcp::service: fail to response message error=channel closed` | ERROR ×5 | dependency noise; the only ERROR lines in 8 days |
| hub 502 / stream reset / silent | WARN | good |

The app has no operational ERROR path at all; "ERROR" means nothing here and "WARN" means everything, so grepping for severity is useless.

**Rotation.** Hourly files, `MAX_LOG_FILES = 72` (`logging.rs:46-49`) — documented as "three days", but a file only exists for an hour in which the app wrote, so 71 files today span **8 days** (09-19 → 09-27); an idle app keeps a week, a chatty one 3 days. Size is not capped (a `RUST_LOG=debug` hour can grow without bound). Hub: `init_in_with(log_dir, true)` → the same rotating files under the data dir on the NAS plus stderr (docker logs); neither is readable without sudo.

**Hub health/metrics.** `/healthz` exists; `/metrics` (`mcp/metrics.rs`) exposes exactly **three series**: `fleet_tool_calls_total{caller}`, `fleet_tool_errors_total{caller}`, `fleet_event_streams_open{caller}`. Nothing about reconcile pass duration, per-host probe time/timeouts, hosts unreachable, ring lag/LAGGED count, usage collector status, DB size, or agent connections. `fleet_health` is the only operational roll-up and is fenced per caller (see the token note).

**error_reports.** It is the **hub's** table (`store/reports.rs`), fed by clients' `POST /report` (desktop flusher: 5 s / 20-batch, 10 s timeout, 5→60 s backoff; 7 × `[report] no answer from /report within 10s` in the log). The desktop snapshot's 0 rows is expected — a hub client never writes it locally. Whether the hub holds rows is invisible to a host-bound token (`GET /reports` is master-only).

**Proposed fix.** Raise usage failures to WARN (rate-limited per host); log stuck-kind transitions and playbook applications with `host`/`session`/`kind` at WARN; log the resolve/keychain step; add 6–8 gauges/histograms to `/metrics` (reconcile pass seconds, probe seconds per host, probe timeouts, hosts unreachable, ring lagged, usage collect ok/fail, session_events rows); consider a size cap alongside the file-count cap. Effort **S** (levels), **M** (metrics).

---

## 6. Usage / cost accounting — P1 for the meaning of `by_day`, P3 for the rest

**(a) Day attribution is the collection day, not the transcript's.** `store/usage.rs:172-248 apply_usage` buckets the delta into `usage_daily(day = d.now.div_euclid(86_400))`, where `now` is when the collector ran (`plan_delta … now`). A new cursor starts at offset 0, so the **first collection of a transcript books its entire history on that day**. That is the 09-21 spike: the hub took over the fleet on 09-20/21 with fresh cursors — `usage_by_day` 09-20 $374 + **09-21 $850** (1.28 B cache-read tokens "in one day" from ~40 sessions), then $294 / $588 / $190 / $422 / $118. The desktop snapshot shows the same artefact when trn was first provisioned: `usage_daily` day 20714 (2026-09-18) `claude-fleet-trn cost_micros 11,647,288,705` (**$11,647**, 14.3 B cache-read + 30 M output tokens) — impossible as one day's work for 8 sessions. Every "cost by day" chart is therefore a *collection* timeline.

**(b) `by_host` and `by_day` are different populations, presented as one report.** `report()` (`usage.rs:867-916`): `by_host`/`total` = `list_all_sessions()` **rows that still exist** (lost rows included — 21504/21505/21507 ghosts carry $95/$55/$49) summed from the session's `usage_*` columns; `by_day` = `usage_daily`. In the payload I received, `total.cost_micros = 507,233,024` while Σ`by_day` over the same window and host = **2,471,711,626** — not double-counted, but the same report shows $507 and $2,472 for "mac, 7 days" and labels neither. Sessions GC'd by `gc.rs` leave `by_host` and stay in `by_day`.

**(c) `usage_by_host` shows only `mac` because of the token fence, not a collection gap.** `usage_report {host_alias: mefistos}` → `E_FORBIDDEN … this token is bound to mac`; the unscoped report answers `host_alias: "mac"`. The brief's "per-host usage collection gap on the hub for SSH/agent hosts" cannot be concluded from this token; verify with the master token. (Collection itself — `collection_hosts`, `usage.rs:651` — runs for every reachable host and would need the agent transport to support the batch script for trn; that is a separate question the fence hides.)

**(d) Price table** (`usage.rs:118-129`, per MTok in/out/cache-write/cache-read): fable-5-1 10/50/20/**0.25**, fable 10/50/20/1.0, opus 5/25/10/0.5, opus-4-1 15/75/30/1.5, sonnet-5 2/10/4/0.2, sonnet 3/15/6/0.3, haiku 1/5/2/0.1; longest-key match; `cache_write_5m` defaults to input × 1.25 (`usage.rs:89`). The table is static, has no "as of" date, and the report only says "Not a bill". Cache-read is ~75–85 % of every day's cost in `usage_by_day` (e.g. 09-21: 1.28 B × $0.5 = $641 of $850 at the opus rate), so the cache-read rate is the single number that decides the total; `usage.prices_json` can override it but nothing surfaces which rate produced a figure.

**(e) Counting.** The AWK reader (`usage.rs:240-313`) sums `message.usage` per line, deduping by **consecutive** `msg_` id (delta against `last`); a message id that reappears non-consecutively double counts (rare; sub-agent transcripts are separate files). Interval **300 s** (`usage.interval_secs` default), one batched script per host with 8 MB chunk / 32 MB host budget, hosts serial — fine for 43 transcripts; the 20 s tick's `spawn_collect` is a cheap gate. Failures are DEBUG (§5).

**Proposed fix.** (a) Attribute by the transcript line's `timestamp` (the AWK already sees each line; emit per-UTC-day rows) and on a *fresh* cursor either backfill by timestamp or mark the delta `backfill=true` and exclude it from `by_day`. Effort **M**. (b) Label the two populations (`live_sessions_total` vs `daily_total`) or compute `by_host` from `usage_daily`. Effort **S**. (d) Stamp the price table with a date and echo the rate used per model in the report. Effort **S**.

---

## Side notes (not scored)

- `[hub events] the hub's wire contract is newer than this build understands hub_contract=3 max_contract=1` ×5 — the hub was upgraded before the desktop on 09-2x; expected under the contract rule, but each such window is a silent read-only desktop.
- A mac session's `friendly_name` in `usage_report` looks like a Slovak IBAN (SK40 6500 …). Friendly names travel to every hub client and into logs; worth a data-hygiene note for the UX/security reviewers.
- `hosts.claude_version` staleness (brief) is consistent with the probe design: the snapshot script does not run `claude --version`; it is only refreshed by probe/provision.

---

## Prioritized list

1. **P1 — Usage day attribution (6a):** book tokens to the transcript's day, not the collection day; mark first-cursor backfill. The 09-21 "$850 day" and the 09-18 "$11.6k day" are artefacts; every cost-by-day view is wrong on any day a cursor is created. Effort M.
2. **P2 — Event stream never resumes (4):** send `Last-Event-ID`, resync only on `resumed:false`, and include projects/worktrees in resync. Removes the 4-call + 63-row burst on every reconnect and the silent loss of project/worktree events across hub restarts. Effort S.
3. **P2 — Startup blocks on the keychain with no log line (4):** log the resolve step, bound the keychain wait, show the window. Explains today's double start and two earlier relaunches. Effort M.
4. **P2 — Logging levels and metrics (5):** usage failures at WARN, stuck/playbook transitions with host/session, six operational series on `/metrics`; the app currently has no meaningful ERROR level and `/metrics` cannot answer "is reconcile healthy". Effort S + M.
5. **P2 — `by_host` vs `by_day` populations (6b):** label or unify; today one payload says $507 and $2,472 for the same host and window. Effort S.
6. **P2 — Brief correction (token scope):** the review token is host-bound to `mac`; re-run `usage_by_host`, `ghosts`, and `sessions_total` with the master token before any "collection gap" or "ghost mismatch" finding is accepted.
7. **P3 — Reconcile pass waits for the slowest host (3):** stream per-host writes as probes complete; 63 probe-timeout events in the window each froze all hosts' freshness for up to 65 s. Effort S.
8. **P3 — status_change dedupe and the two-writer gap (2):** dedupe on insert, record hook-driven transitions, move the per-insert trim to GC; the 09-11 phantom loop is fixed, the `working→working` duplicates are not. Effort S.
9. **P3 — `fleet_health.tunnels` cannot say "not applicable" (1):** add `tunnels_mode`. Effort S.
10. **P3 — Log retention semantics (5):** 72 files ≠ 3 days; add a size cap or document "72 active hours". Effort S.
