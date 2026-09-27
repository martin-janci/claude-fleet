# Live instance analysis — 2026-09-27

Six expert lenses over the running fleet (hub `fleet.rlt.sk` 0.3.1 on the NAS,
desktop 0.2.42 in hub-client mode, fleet-agent 0.2.26 on claude-fleet-trn,
five SSH/agent hosts, 63 session rows, 77 projects, 350 worktrees). Every
lens was read-only. Raw facts the lenses started from are in
[00-brief.md](00-brief.md); each lens file carries evidence with `file:line`,
root cause, fix and effort.

| Lens | File | Findings |
|---|---|---|
| Data correctness / sync | [data-sync.md](data-sync.md) | 7 |
| Host provisioning / setup | [hosts-provisioning.md](hosts-provisioning.md) | 9 |
| Hub daemon / deployment | [hub-ops.md](hub-ops.md) | 9 |
| Session lifecycle / recovery | [lifecycle.md](lifecycle.md) | 10 |
| Performance / logs / observability | [perf-logs.md](perf-logs.md) | 10 |
| UX / smoothness | [ux.md](ux.md) | 20 |

## Two caveats that shape every number below

1. **The MCP token this analysis used is a per-host token bound to `mac`, not
   the master token.** `usage_report {host_alias: mefistos}` answers
   `E_FORBIDDEN`. So `fleet_health.usage_by_host` listing only `mac`, and
   `sessions_total 43` / `ghosts 3`, are *fence artefacts of the caller*, not
   collection gaps. Re-run those three numbers with the master token before
   acting on them.
2. **This worktree is ~371 commits behind `origin/main`** (CHANGELOG stops at
   0.2.39). Lenses cite `origin/main` where the code moved (`local_disabled`,
   `retire_local_sessions`, the `refresh_projects` fix, the tunnel fix
   `1eb18513`). Line numbers for `src/` are from the 0.2.39 tree.

## Cross-cutting themes (what several lenses found independently)

### T1 — The `local` → `mac` rename was never finished
The hub DB is a copy of the desktop DB; the Mac was then re-added as `mac`.
Result: a hidden `local` row (`provisioned: true`, `reachable: false`, linked
to a *different* account than `mac`), 6 ghost `bg:` rows on it (3 of them
duplicates of `mac` rows by `claude_session_id`, still `claude_status:
working`), **249 of 350 worktree rows stranded on `local` while `mac` has 0**
(so `list_worktrees {host_alias: mac}` is empty and `delete_worktree` on them
is refused), and the whole project tree pinned to a host nothing probes.
Reconcile skips hidden hosts (`reconcile.rs:1428`) and the reaper only runs
for probed hosts, so these rows are immortal and `docs/hub.md`'s "pruned like
any other ghost" is false. Found by data-sync F2/F5, lifecycle F5, hosts F7,
hub-ops F6, ux F-02.

### T2 — Ghost accounting has two definitions and no bulk exit
`health.rs:113` excludes `kind = external` rows from every count, and every
`bg:` row is external: 63 rows → `sessions_total 43`, 19 lost rows →
`ghosts 3`. The UI files lost `bg:` rows as *live* (`SessionRowItem.svelte`
tests the external branch before the ghost branch) so 15 ghosts, six of them
saying `working` a day after loss, sit under "Outside fleet (20)" with no
Dismiss. Ghost dismissal is one id per call and ghosts are excluded from
select mode: 19 ghosts = 19 clicks, 16 of which do not exist. The Mac
genuinely reboots daily (`kern.boottime` matches every `host_reboot`
verdict), and external rows get the 14-day TTL meant for resumable rows.
Found by data-sync F3, lifecycle F4/F6, ux F-01/F-03/F-04/F-05.

### T3 — Host state is a Sep 20 snapshot stamped with today's ping
`list_hosts.claude_version` is only written by `probe_host`; reconcile
writes the stored value back every tick (`reconcile.rs:765,878`). Actual vs
cached: mac 2.1.282 (2.1.235), mefistos 2.1.267 (2.1.234), oci 2.1.282
(2.1.220), htz 2.1.214 (68 patch releases behind). The only host badge that
exists (`claude_old`, `hosts_view.ts:206`) is therefore wrong on 3 of 4
hosts. There is **no disk, load, memory, uptime or agent-version signal at
all**; two hosts are at 98 % disk (htz 3.6 GB free, `~/.paperclip` = 89 GB;
mefistos 14 GB free, 46 GB of it fleet's own cargo targets under `~/.cache`).
The provisioned skills on every host are stale (claude-fleet-control at
commit 60695fef vs 8f1339fb shipped since v0.2.38: hosts lack the
`work`/`work_link` skill content); provisioning only runs on an explicit
`provision_hosts`, and the hub-client desktop refuses it (LocalOnly). Found by
hosts F1/F3/F4/F5/F9, data-sync F1, ux F-13/F-14.

### T4 — The status machine trusts the wrong signals
`oom` is a whole-word prose match (`pane_intel.rs:376-386`); sessions 21480
and 21340 were claude-fleet dev sessions *reading the fleet's own stuck
vocabulary*, and the `oom_recreate` playbook killed 21480 twice mid-turn
(3600 s spacing, no attempt cap, no "is it working" check; `--resume`
re-renders the word and re-flags 14 s later). `working` never ages out (two
trn rows have had no Stop or transcript growth for ~40 h). A `StopFailure`
429 is recorded as plain `idle` with no attention reason and no retry (the
user re-prompted 11× by hand). `needs_attention` flags exactly the 3 ghosts a
human can do nothing about and misses context 99/94/91/90/86 %, the stale
`working` pair, the 429 row and 4 idle sessions with failing CI. Found by
lifecycle F1/F2/F3/F7, ux F-09.

### T5 — Cost-by-day is a collection timeline, not a usage timeline
`apply_usage` books deltas into the *collection* day and a new cursor starts
at offset 0, so the hub takeover on 09-20/21 booked whole transcript
histories as "$374" and "$850" days (the desktop snapshot has a "$11,647"
day for trn on 09-18 from the same mechanism). `by_host`/`total` sum live
rows (ghosts included) while `by_day` reads `usage_daily`, so one payload
shows $507 and $2,472 for the same host and window, unlabeled. Cache-read is
75–85 % of every day's cost. Found by perf F6, data-sync F4.

### T6 — Nothing is watching the hub
No scheduled backup of `state.db`; the two `backup-*` dirs on the NAS are
`cp -p` copies of an *open WAL* database (mtime of `state.db` minutes older
than `-wal`/`-shm`). Hub logs go to `./data/logs` but `data/` is uid 1000
mode 0700 (sudo needed); the NAS compose dropped upstream's json-file caps;
`fleet_health` has no uptime / last-tick / tick-error fields; `/metrics` has
3 series. `0.0.0.0:4180` plaintext is listening on every NAS v4/v6 address
(the hub allows the routable bind because the public URL is https; the Host
allowlist is a rebinding defence, not a LAN barrier); failed bearers on
`/mcp` get one warn line and no throttling; there is no Cloudflare Access.
The desktop never sends `Last-Event-ID` so every reconnect re-lists
sessions/hosts/tasks/accounts (63 `session:updated`) and never re-lists
projects/worktrees/work (silently stale after a hub restart). Found by
hub-ops F3/F5/F7, perf F4/F5.

### T7 — Version triangle is compatible today, procedurally exposed
`CONTRACT_REVISION = 4` and `MIN..=MAX_HUB_CONTRACT = 4..=4` on both v0.2.42
and v0.3.1; the wire contract, golden rows and tool reference are
byte-identical between the tags, so the older desktop sees every hub tool.
`PROTO_VERSION = MIN_SUPPORTED_PROTO = 1`, so the 0.2.26 agent connects, but
it predates the `AgentFrame::Report` batching: trn never reports errors and
always looks healthy. The footer prints `v{health.version}` — the **hub's**
0.3.1 on a 0.2.42 app; the app's own version is shown nowhere. Found by
hub-ops F1, hosts F5, ux F-11.

## Do now (operations, no code)

1. **Backup the hub DB properly** on the NAS as root:
   `sqlite3 /volume1/docker/fleet-hub/data/state.db ".backup /volume1/docker/fleet-hub/backup-$(date +%F).db"` then `PRAGMA integrity_check`; schedule it nightly (DSM task) with 14-day retention.
2. **Free disk** before anything else fails: htz `~/.paperclip` (89 GB of a
   150 GB overlay, 3.6 GB free); mefistos `~/.cache/claude-fleet-shared-target`
   (41 GB) + `rowproj-target` (5 GB) + `~/.cargo-targets` (18 GB) +
   `/tmp/claude-1000` (9.3 GB) + journal (4.1 GB) — `/mnt/sda4` has 298 GB free.
3. **`chmod 600` or delete `~/.claude/settings.json.bak` on the Mac** — mode
   0755, world-readable, still holds the legacy `http://127.0.0.1:4180/hook?token=…`
   command hooks. Rotate the `.fleet-bak` copies from Sep 20.
4. **Upgrade order this round:** desktop `mac-desktop` → 0.3.1 (no migration
   risk in hub-client mode), then `fleet-agent` on trn → 0.3.1 (`fleet-agent
   install` with the existing token; the 30 tmux sessions are unaffected).
   Standing rule: on a `CONTRACT_REVISION` bump, hub first then desktop in the
   same window; on a `PROTO_VERSION` bump, hub first with `MIN_SUPPORTED_PROTO`
   held.
5. **With the master token:** `probe_host` × mac/mefistos/oci (refreshes the
   cached versions), `dismiss_ghost_session` × the 6 `local` ghosts, and
   re-read `fleet_health.usage_by_host` / `ghosts` / `sessions_total`.
6. **Re-provision every host** (`provision_hosts`, master token, standalone
   desktop or a hub subcommand once it exists) to ship the current
   claude-fleet-control skill.
7. **Data correction after the backup:** re-home the Mac —
   `UPDATE worktrees SET host_alias='mac' WHERE host_alias='local'` (same for
   `worktree_parent_fingerprints`, `dismissed_agents`, merge `usage_daily`),
   then delete the `local` session rows and the `local` host row.
   See data-sync F5 for the exact statements and the UNIQUE-collision check.

## Code changes, prioritised (P0/P1 first; S = hours, M = a day or two, L = more)

| # | Sev | Effort | Change | Lens |
|---|---|---|---|---|
| 1 | P0 | S then M | `oom_recreate` playbook: refuse while the session is `working`/mid-turn, cap attempts per episode, do not restart the episode on the same match; then replace the prose `oom` detector with a process/exit-code signal | lifecycle F1 |
| 2 | P1 | S | Ghosting clears `claude_status`/`stuck_kind`/`current_activity`; external ghosts get a short TTL (they are never resumable); `kind=shell` rows leave `by_status` | lifecycle F4/F6/F8 |
| 3 | P1 | S | `StopFailure` → `failed` + attention reason + `stop_failure` event kind in `session_history` | lifecycle F3 |
| 4 | P1 | M | `working` ages out: no Stop/transcript growth for N minutes ⇒ `idle` + `stale_working` attention | lifecycle F2 |
| 5 | P1 | M | Attention model: `context_full`, `stale_working`, `stop_failed`, `ci_failing` buckets; one context threshold shared by hub and desktop (hub 85 vs desktop 70/90 today) | lifecycle F7, ux F-09 |
| 6 | P1 | M | Usage day attribution: book tokens to the transcript's day; mark first-cursor backfill; label `by_host` (live rows) vs `by_day` (durable) | perf F6 |
| 7 | P1 | S | `SessionRowItem`: ghost branch before external branch; `buildOutsideFleet` filters `lost_at`; label `lost_reason: local_disabled` | ux F-01 |
| 8 | P1 | M | `dismiss_ghost_sessions` (plural) hub tool + "Dismiss all lost (n)"; ghosts selectable | ux F-04 |
| 9 | P1 | M | `host rename` / alias merge keyed on machine identity; `retire_local_sessions` reaps (not only ghosts) and re-homes worktrees; hidden hosts reaped by a pass that does not need a probe; one `active_hosts()` used by reconcile, usage, gc, prune | data-sync F2/F5, hub-ops F6 |
| 10 | P1 | S | Collect `claude --version` (and tmux) in the reconcile identity script; `claude_version_at`; badge only when fresh | data-sync F1, hosts F3, ux F-13 |
| 11 | P1 | M | Host health probe: `df` home + tmp, load, mem, uptime, `agent_version` → `HostRow`; `fleet_health.hosts[]` with `disk_low` / `claude_behind` / `agent_behind` / `hooks_silent`; HostDetail Health block; `move_session` target-space preflight | hosts F4/F5/F9, ux F-14 |
| 12 | P1 | M | Provision fingerprint per host + content-only re-provision on hub start; `provision_stale` in `list_hosts`; a `fleet-hub provision` subcommand; declare ownership of the two skill dirs (marker file, preflight warning when the target is a git tree) | hosts F1/F2 |
| 13 | P1 | S | Nightly `sqlite3 .backup` documented + scripted `upgrade.sh <ver>` (pull → backup → stop → tag → up → `/healthz`, keep 3) | hub-ops F7/F2 |
| 14 | P1 | S | Hub on Caddy's docker network, drop `ports:`; startup WARN on a routable plaintext bind; per-IP 401 limiter on `/mcp`, peer addr in the reject line | hub-ops F3 |
| 15 | P1 | S | Footer `app 0.2.42 · hub 0.3.1 · contract 4`; `hub_version` in `HubStatus`; banner when the hub is a minor ahead | ux F-11 |
| 16 | P1 | S | Agent-transport hosts default to Conversation instead of a failing SSH attach on every selection ("no SSH route · Try anyway") | ux F-17 |
| 17 | P1 | S | In-project triage sort + alphabetical idle tail + `+n idle` fold; stop re-applying backend order on the 30 s focus refetch | ux F-08/F-16 |
| 18 | P1 | M | Operator row: badge, opens the panel, panel state derived from the live row (`blocked` → "waiting for you") | ux F-19/F-20 |
| 19 | P2 | S | Desktop sends `Last-Event-ID`; resync only on `resumed:false`; resync includes projects/worktrees/work | perf F4 |
| 20 | P2 | M | Startup: log the backend-resolve step, bound the keychain wait, show the window (explains today's double start) | perf F4 |
| 21 | P2 | S+M | Usage failures at WARN; stuck/playbook transitions logged with host+session; `hub{uptime, reconcile{last_at, last_duration, consecutive_failures}}` in `fleet_health` (serde(default)); gauges on `/metrics`; `./logs` mount 2750 + json-file caps; Uptime Kuma with a readonly client token | perf F5, hub-ops F5 |
| 22 | P2 | S–M | `refresh_projects` deletes vanished in-root rows; `forget_project`; drop `ppt-epic-145..150`; dedupe the 12 duplicate worktree paths (projects 59/61/62) | data-sync F6/F7 |
| 23 | P2 | S | `bg:<uuid>` display name (`agent 6141be · opus-5 · 10 turns`); per-project `Agents (n · m working)` fold; `agents.retire_after_secs` | ux F-06/F-07 |
| 24 | P2 | S | `fleet_health.tunnels_mode` ("not applicable" on a public hub); `peer_links_total`; `known_hosts` / key-rotation runbook; `ssh-key --rotate` | perf F1, hub-ops F6/F8 |
| 25 | P3 | S | Stream per-host reconcile writes instead of waiting for the slowest host (65 s probe timeout vs 20 s tick); dedupe same-value `status_change` on insert; record hook-driven transitions; controller pair guarded (not last-writer-wins) | perf F3/F2, lifecycle F9/F10 |

Items 7, 10, 15, 16, 17 and the desktop half of 5 are TypeScript-only or
contract-neutral and ship without the parity lane (UXPR-13/22/26, which this
worktree shows has not landed: 81 LocalOnly commands vs the audit's 70 and
target 42).

## What was verified as fine

- Hooks: all 9 fleet hook events on every SSH host are `type: http` →
  `https://fleet.rlt.sk/hook`, headers file (Sep 20) still authenticates
  (hook-driven `turn_done` rows today); managed CLAUDE.md block byte-identical
  to `CLAUDE_MD_BODY` on all four hosts; no host points at an old tunnel port.
- tmux 3.3a vs 3.6a: nothing fleet uses differs.
- Shutdown budget: DRAIN 10 s + TICK_SHUTDOWN 10 s < `stop_grace_period` 30 s;
  WAL + `synchronous=NORMAL` + one transaction per host make a mid-tick
  SIGKILL safe; a torn pass cannot ghost sessions.
- Reconcile design: one SSH script per host, JoinSet fan-out, no per-session
  round trips, `session:updated` only on change, cache-first list.
- Render path: keyed each-blocks, one-pass indices, batched events, identity
  merges; the felt jank is order churn, not rendering.
- Desktop snapshot integrity: `PRAGMA foreign_keys` on, 0 FK orphans, the
  ghost/lost invariant holds.
- The 2026-09-19 tunnel restart loop (776 + ~290×3 restarts) was build 0.2.23
  with trn still on SSH transport; fixed on main in `1eb18513`.
- The 09-11 `status_change` phantom loop (2,007 NULL→NULL rows, 3 sessions
  hitting the 500 cap at exactly one event per 20 s tick) was fixed in
  `188b8dcb`; the residual is 164 `working→working` duplicates from the
  two-writer gap (hooks and reconcile both set status, only reconcile records).
