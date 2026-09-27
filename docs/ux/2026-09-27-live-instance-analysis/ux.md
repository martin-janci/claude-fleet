# UX / product review of the live claude-fleet desktop against its real fleet — 2026-09-27

Scope: what the user of the desktop app (hub-client mode, `mac-desktop` → `https://fleet.rlt.sk`)
actually sees with this fleet: 43 live + 19 lost session rows, 6 hosts (one hidden, one agent-only),
77 projects, an operator session, desktop 0.2.42 against hub 0.3.1. Read-only; code + live data only.

Method: Svelte/TS in `src/`, `src/lib/*.ts`, the hub-client backend in `src-tauri/src/backend`, the
service layer in `crates/fleet-core`, plus filtered `list_sessions` reads on the hub. The existing
audit (`docs/ux/2026-09-21-audit`, UX-01…140, UXPR-01…38, D1–D14) is referenced by id and not
repeated; everything below is what the LIVE DATA exposes that the audit's screenshots could not.

Caveat on evidence: the worktree checked is `package.json` 0.2.39 on
`feature/instance-db-agents-analysis-0e4558` (newer than the audit's v0.2.33, older in version
number than the running 0.2.42 build). Line numbers are from this worktree. The parity queue the
audit planned (UXPR-13/22/26 …) is NOT landed here: `src/lib/hub_verdicts.generated.json` lists
**81** LocalOnly commands (the audit counted 70 and targeted 42).

Live numbers used (from `fleet_health` and filtered `list_sessions`): sessions_total 43 (idle 38 /
working 3 / unknown 2), ghosts 3, context_red 5, hosts 5/6 reachable; `include_lost` rows 63, of
which 19 carry `lost_at` (mac 13, hidden `local` 6); all 16 `bg:<uuid>` lost rows are
`kind: "external"`; `local` rows have `lost_reason: "local_disabled"`; three uuids
(`cb0bab25…`, `11d6176e…`, `a0336855…`) exist as ghosts on BOTH `mac` and `local`.

Severity: P0 blocks or misleads the operator today · P1 wrong picture of the fleet · P2 friction ·
P3 polish. Effort: S ≤ 1 day · M 1–3 days · L > 3 days.

---

## 1. Ghost and duplicate rows

### F-01 · P1 · A lost `bg:` row renders as a live one, with a live status chip and no way to dismiss it
- **Evidence.** `src/lib/SessionRowItem.svelte:437-458`: the `readOnly` ("Outside fleet") branch is
  checked BEFORE the `status === 'ghost'` branch (`:459`), and the comment says so on purpose ("a
  ghosted external row must not offer Recreate / Dismiss"). The read-only branch draws
  `status-dot status-ghost` plus the `claude_status` chip and nothing else — no "lost 2d · host
  rebooted" marker, no Dismiss. `src/lib/sidebar_index.ts:137-151` `buildOutsideFleet` applies no
  `lost_at` filter. `src/lib/attention.ts:196` `classify()` returns early for `external` rows
  (`working` / `idle` from `claude_status`), so a lost external row is never `lifecycle`.
  Live: "Outside fleet (20)" holds 5 running + 15 lost `bg:` rows; the 6 `local` rows show
  `claude_status: working` (ids 21486, 21490, 21491, 21498) although lost since 2026-09-26; `mac`
  row 21504 shows `blocked`. Their `lost_at` span five separate loss events from 1789981930 to
  1790421034 — six days of accumulation.
- **Related.** UX-06, UX-91, UX-106, Q-II.13 (Outside fleet stays independent of the bg toggle).
- **Proposed.** (a) `SessionRowItem`: test `status === 'ghost'` first for every kind; a lost
  external row renders the ghost skeleton (name · `lost 2d · host rebooted` · Dismiss), never a
  status chip. (b) `buildOutsideFleet` splits `lost_at !== null` rows into a collapsed
  `Lost (15)` tail under the live ones, or drops them when `showLost` is off. (c) `classify()`:
  `external && lost_at` → `idle` (never `working`). (d) `lostReasonLabel` (`sessions.ts:227-238`)
  gains `local_disabled` → "local host disabled" — today it falls to `null` and the row shows a bare
  "lost 1d".
- **Effort.** S.

### F-02 · P1 · A hidden host's rows leak into every list, frozen forever, and duplicate the renamed host's rows
- **Evidence.** `sidebar_index.ts:64` `rowMatches` filters on host equality only; the host pills
  hide hidden hosts (`SidebarFilters.svelte:121`, `HostChips.svelte:42`), so with the filter on
  `all` the `local` rows are in the tree and there is no pill to isolate or exclude them. The hub's
  reconcile skips hidden hosts (`crates/fleet-core/src/service/sessions/reconcile.rs:1428`
  `filter(|(h, _)| !h.hidden)`), and the Phase-2 reaper runs per reconciled host
  (`store/reconcile.rs:558-611`, `WHERE host_alias=?1 AND status='ghost'`), so a hidden host's
  ghosts are never re-evaluated and never reaped: `claude_status` stays whatever it was when the
  host was hidden. Live: `local` (hidden, unreachable, last ping 2026-09-20) carries 6 ghosts whose
  `last_activity_at` is frozen at 1789929074; three of them are the same Claude conversations as
  `mac` rows 21504/21505/21507 (the host was renamed `local` → `mac`), with different cost totals
  (`cb0bab25…`: $50 on `local`, $94 on `mac`), so `fleet_health.usage_by_host.mac` double-counts.
- **Related.** UX-71 (hidden `local` counted in "Hosts 6"), UX-96, UXPR-23 D12 (hidden hosts fold
  into `N hidden ▸`), consolidation-01 §5 item on `find_by_unique_cwd` / uuid collisions.
- **Proposed.** (a) `sessionVisible`: a row on a hidden host is visible only when `hostFilter` is
  that alias; the tree footer says `6 rows on hidden hosts` with a link that sets the filter.
  (b) Hub: `hide_host` ghosts the host's rows with `lost_reason = 'host_hidden'` and the reaper
  visits hidden hosts (no probe, reap only). (c) Dedupe on `claude_session_id`: when a live or lost
  row with the same uuid exists on another host, keep the newest and delete the older (host rename
  is the only way this happens); `usage::per_host_totals` skips ghosts of a duplicated uuid.
- **Effort.** M (a: S in TS; b+c: M in fleet-core, contract-neutral).

### F-03 · P2 · `fleet_health.ghosts = 3` vs 19 lost rows: two definitions of "ghost", neither explained in the UI
- **Evidence.** `crates/fleet-core/src/service/health.rs:113-121` skips `kind == "external"` from
  every session count; the 16 `bg:` ghosts are external, so 3 is exactly the non-external ghosts
  (`bright vega` 21631, `dev-martin-janci-claude-fleet` 21706, `fleet-probe` 21541 — each with
  `needs_attention: {reason: lifecycle}`). The desktop, however, lists with `include_lost: true`
  (`src-tauri/src/backend/remote.rs:709`), so it draws all 19. `HostDetail.svelte:88`
  `restorable` excludes `bg`/`external`, so on `mac` the button says "Restore 3 lost sessions…"
  (the three `kind: work` ghosts all carry a `claude_session_id`) while the row list under the
  same host shows 13 ghosts. Nothing tells the user the other 10 are un-restorable agent rows.
- **Related.** UX-19 (HostDetail), UXPR-23.
- **Proposed.** `HostDetail` lost section: "13 lost rows · 3 restorable · 10 agent rows without a
  pane" with `Dismiss the 10` next to `Restore 3…`. `fleet_health` adds `lost_rows` (all kinds) so
  the roll-up and the list agree; the MCP description for `ghosts` says "fleet-managed only".
- **Effort.** S.

### F-04 · P1 · No bulk dismiss: 19 ghosts = 19 clicks, and 16 of them have no click at all
- **Evidence.** `dismiss_ghost_session` is one id per call
  (`crates/fleet-core/src/service/sessions/lifecycle.rs:1422-1445`); the only UI is the per-row `×`
  (`SessionRowItem.svelte:479-486`), which the read-only branch does not render (F-01). Select
  mode ignores ghosts by design (UXPR-36 model; `selection.ts:144`), and the bulk bar has Kill /
  Prompt only. The 14-day TTL (`settings.rs:214`, `sessions.lost_ttl_secs = 1209600`) keeps every
  `host_reboot` row with a `claude_session_id` — which every `bg:` row has — for two weeks.
- **Related.** UX-114 (ghost/bg in bulk), UX-123 (no select-all), UXPR-36/37.
- **Proposed.** Hub tool `dismiss_ghost_sessions { host_alias?, kind?, older_than_secs?,
  keep_restorable: true }` (Routed; `ClientOnly` visibility per ADR 0003; T1), returning the ids
  removed. UI: a `Dismiss all lost (n)` control in three places that already exist — the
  `Outside fleet` header, the `Other sessions` header, and HostDetail's lost section — with the
  restorable rows protected behind a checkbox. Select mode may include ghosts when the bulk action
  is Dismiss (the eligibility table from UXPR-37 gets one row).
- **Effort.** M.

### F-05 · P2 · A lost row outranks running work in the tree for 14 days
- **Evidence.** `attention.ts:122-131` `TRIAGE_BUCKETS`: `lifecycle` (index 4) sorts above
  `working` (6); `sortProjectsBySeverity` (`sidebar_index.ts:172-181`) floats the project by its
  worst row. On `mac` the three non-external ghosts pin `claude-fleet` to the top of the tree and
  keep `⚠ Needs you (3)` lit until the TTL reaps them or the user clicks 3 times. None of the three
  is something to act on today (bright vega lost 2026-09-25 with `last_prompt: "1"`).
- **Related.** UX-92 (no status sort), UXPR-30, Q-II.14.
- **Proposed.** Split the bucket: `lifecycle` keeps `safe_kill failed/requested`; a new `lost`
  bucket goes after `idle_long` for rows whose `lost_at` is older than one reconcile cycle, counted
  in the filter but not in the pill (same rule as `idle_long`). Restorable rows stay `lifecycle`
  for the first hour after the loss, then age into `lost`.
- **Effort.** S.

---

## 2. Background `bg:<uuid>` sessions

### F-06 · P1 · Twenty rows named by uuid, five of them alive
- **Evidence.** `primaryName` is `friendly_name ?? tmux_name` (`SessionRowItem.svelte:149`);
  agent rows are named `bg:<claude session id>` (`service/sessions/reconcile.rs:996-999`) and get no
  friendly name (the auto-name path is prompt-driven, and nothing sends a prompt to a `bg` row).
  The 🤖 badge exists only for `kind === 'bg'` (`:502`); the external `bg:` rows have no badge at
  all. Live: 5 running `bg:` rows under `claude-fleet` on `mac` (4 idle, 1 working) plus the 15
  lost ones — the row text is `bg:6141be6b-cff2-4e28-b51b-13b66606ec55`, and the rows carry
  everything a label needs (`model`, `project_id`, `turn_seq`, `context_pct`, `last_stop_at`).
- **Related.** UX-06 (raw `bg:<uuid>` — mechanism confirmed, still open), UXPR-04/06 (`displayName`
  with `external` default — not landed here), UX-15.
- **Proposed.** Pure TS, no wire change: `displayName()` for a `tmux_name` starting `bg:` returns
  `agent <first 6 of uuid> · <model short> · <turn_seq> turns` (e.g. `agent 6141be · opus-5 · 10
  turns`), tooltip keeps the uuid. Backend follow-up (UXPR-04): `upsert_bg_session` sets a default
  `friendly_name` from cwd + model once, so the hub and the phone agree.
- **Effort.** S (TS) · M (with the backend default).

### F-07 · P2 · A finished bg row is never retired from the list
- **Evidence.** A live bg row that goes inactive becomes `stopped`
  (`service/sessions/reconcile.rs:1113-1117`), and stays in the tree as a `stopped` row. GC kills
  bg rows only with `gc.enabled` (default `false`, `settings.rs:239`) after `gc.bg_idle_secs`
  (86 400) and only `kind == 'bg'`; `external` rows are never collected (`gc.rs:118-122`). Once
  lost, the 14-day TTL applies (F-04). In hub mode the one explicit retire action,
  `dismiss_agent_session`, is LocalOnly with the reason "use Kill instead" (`hub.ts:228`), and Kill
  on an inactive external row is what UX-82 already flagged as hidden.
- **Related.** UX-82, UX-91, UX-114, UXPR-26 (RN `dismiss_agent_session`), Q-II.13.
- **Proposed.** (a) Sidebar: fold `kind bg|external` rows of a project into an `Agents (5 · 1
  working)` sub-row, collapsed by default when all are idle/stopped — the same `Done · n` fold the
  work groups already have (`Sidebar.svelte:1060-1080`). (b) Hub setting `agents.retire_after_secs`
  (default 6 h): a `stopped`/`idle` pane-less row past it is deleted by the sweep without a kill —
  there is no process to kill. (c) Route `dismiss_agent_session` (UXPR-26) so the per-row `×`
  works in hub mode.
- **Effort.** M.

---

## 3. Scale: 30 sessions on one host, 43 rows on screen

### F-08 · P1 · The list has no order inside a project and reshuffles on every refetch
- **Evidence.** `buildSessionsByProject` (`sidebar_index.ts:102-118`) keeps store order; the
  store takes its order from the backend's `ORDER BY last_activity_at DESC` on every list
  (`sessions.ts:283-290`, "the list owns ORDER"); `byTriage` is exported and never called (UX-92).
  `last_activity_at` is rewritten by every reconcile pass (`sidebar_index.ts:145`, and the `local`
  rows all share 1789929074), and the window-focus refetch runs every 30 s (`App.svelte:302-320`),
  so the 17 `papayapos-backend` and 9 `pos-frontend` rows on `claude-fleet-trn` change order under
  the cursor with no visual cause. 38 of 43 rows are idle; nothing collapses them; the project header
  shows only a count (`Sidebar.svelte:1146`).
- **Related.** UX-92, UX-97 (`8h 23m` is age not activity), UXPR-30 (`rows.sort` pref),
  UXPR-31 (recency on session not project).
- **Proposed — the smallest change that makes 43 rows scannable.** In `buildSessionsByProject`
  sort each project's rows by `byTriage` and, within `idle`, by `displayName` (stable, alphabetical
  — the `pd-NNNN` branch names then read as a list); collapse the idle tail beyond 5 rows into a
  `+12 idle ▸` toggle per project (state in `collapsed`-style sets, persisted); show `working ·
  blocked` counts in the project header (`2 ⚡ · 1 ⏸ · 14`). No wire change, one `$derived`.
- **Effort.** S.

### F-09 · P1 · Five `context_red` sessions are not an attention signal anywhere
- **Evidence.** `fleet_health.context_red = 5` at `CONTEXT_RED_THRESHOLD = 85`
  (`health.rs:93`); the desktop uses 70 / 90 (`attention.ts:75-76`) and draws a context badge only
  on line 2 of a row (`SessionRowItem.svelte:722-735`), which `rows.details` off removes; `classify()`
  has no context bucket, so these five sit among the 38 idle rows and never enter `Needs you`. Two
  of the lost external rows on `mac` carry 94 % and 86 % as well, so the number is also inflated by
  ghosts on the hub side (health counts non-external only, but a lost `kind='bg'` row would count).
  The `-term` shell rows (`sleek-castor-term`, `noble-virgo-term`) have `claude_status: null` and
  therefore no chip at all (`:451 {:else if sess.claude_status}`) — they read as "nothing to say"
  rather than "a shell".
- **Related.** UX-98 (100 % without action; thresholds), UXPR-34 (80/95), Q-II.14 (a)
  `context_full` bucket.
- **Proposed.** One threshold shared by hub and desktop (adopt 85 from `health.rs` or move both to
  UXPR-34's 95 — one constant, exported from `wire_contract`-adjacent code so the two cannot drift);
  a `context_full` bucket between `failed` and `lifecycle`, counted in the pill; the chip on line 1
  when ≥ threshold; shell rows get a neutral `shell` chip. Health: skip ghosts when counting
  `context_red`.
- **Effort.** S.

### F-10 · P2 · Host pills carry no count, so the one host with 30 rows is indistinguishable
- **Evidence.** `SidebarFilters.svelte:115-130` renders `all` + one pill per non-hidden host with
  the alias only; the audit measured them wrapping to 2–3 rows at 280 px (UX-89). With 30 of 43 rows
  on `claude-fleet-trn`, `all` is effectively that host plus noise.
- **Related.** UX-89, UXPR-28 (host `<select>`).
- **Proposed.** When UXPR-28 turns the pills into a `<select>`, each option reads `trn · 30 (1 ⚡)`;
  until then, a count suffix on the pill (`hostVisibleSessions` already exists).
- **Effort.** S.

---

## 4. Version skew: desktop 0.2.42 behind hub 0.3.1

### F-11 · P1 · The footer shows the HUB's version as if it were the app's; the app's own version is shown nowhere
- **Evidence.** `App.svelte:955-958`: `v{health.version} · db … · schema …` with the comment "In
  remote mode this is the HUB's version". The hub badge beside it says `hub: https://fleet.rlt.sk (as
  mac-desktop)` — no version, no contract. `HubStatus` (`src-tauri/src/commands/hub.rs:42-70`,
  `hub.ts:31-52`) carries neither `hub_version` nor `contract`. The only skew mechanism is the wire
  contract (`src-tauri/src/backend/contract.rs:107,116`: `MIN_HUB_CONTRACT = MAX_HUB_CONTRACT =
  4`; `events.rs:405-466`), and 0.3.1 still reports revision 4, so the app treats a hub three minor
  releases ahead as `InRange` and shows no banner. What the user sees today: `v0.3.1 · db: ok ·
  schema 60` at the bottom of a 0.2.42 app.
- **What happens across the skew.** Hub tools the desktop does not know: unused (no UI). Desktop
  commands the hub does not know: refused after the click (`E_HUB_PROTOCOL`/tool unknown), or
  swallowed where the caller is best-effort (`TerminalView.svelte:527-533` `unarchiveSession` —
  "an older hub without the action refuses it"). Hub row fields the desktop does not know: dropped
  by serde (the `SessionRow` comments mark `org_id`, `work_suggested` etc. as "absent from a hub
  older than …" — the reverse direction, a newer hub, is silent by construction). The 81 LocalOnly
  commands refuse with a `REASONS` sentence before the click regardless of what the hub could do
  (`get_fleet_settings`, `list_account_usage`, `tunnel_status`, `session_tool_detail`,
  `inspect_safe_kill`, `dismiss_agent_session` — all still LocalOnly here).
- **Related.** UX-16/17/18/21 (parity families), UX-42/49/70/85 (reasons that point nowhere),
  11-expert-review §4 ("no updater, not even a version check").
- **Proposed.** (a) Footer: `app 0.2.42 · hub 0.3.1 · contract 4` — the app version from
  `@tauri-apps/api/app getVersion()` (no backend change), the hub version from the `health.version`
  the footer already has; when `hub.minor > app.minor` the badge gets `⬆ app older than hub` with a
  tooltip naming the release tag. (b) `HubStatus` gains `hub_version` and `hub_contract` from the
  `ready` frame (`events.rs:414-419` already parses it) — Settings → Hub shows both under the
  paired-as line. (c) The `E_HUB_CONTRACT` banner text (`hub_connection.ts:38-45`) is fine; add the
  same sentence to Settings → Hub, where the user goes to fix it.
- **Effort.** S.

### F-12 · P2 · The reasons for LocalOnly refusals describe a standalone app, not this fleet
- **Evidence.** `hub.ts:204` `tunnel_status: 'the tunnels belong to whichever process owns the
  fleet'` while the hub's `fleet_health.tunnels` is `{}` and the pre-hub desktop log shows the
  `claude-fleet-trn` tunnel restarting 133 times in five hours — under the hub the same failure would
  be invisible from the desktop. `list_account_usage: 'this app does not poll account usage while a
  hub owns the fleet, so its cache stays empty'` — the footer usage segment (`App.svelte:986-994`)
  therefore never renders in hub mode, yet the hub has `usage_by_day` (2026-09-21: `cost_micros` 849,590,325 = $849.59) and `usage_by_host` (mac only, see F-02).
- **Related.** UX-69 (hub has a UsageCache), UXPR-22 (`list_account_usage` T0), UX-73.
- **Proposed.** Ship UXPR-22 first (the reason sentences are deleted by their routing PR, D6); until
  then the footer shows `usage: see hub` linking to Hosts with the hub's `usage_by_day` total, which
  `fleet_health` already returns to the desktop.
- **Effort.** S (interim) · M (UXPR-22).

---

## 5. Host health

### F-13 · P1 · The "Claude Code older than the newest in the fleet" badge is wrong on 3 of the 4 hosts it flags
- **Evidence.** `hosts_view.ts:206-212` compares `host.claude_version` against the fleet's newest;
  `claude_version` is written only by an explicit probe (`service/hosts.rs:434,506` →
  `store/hosts_accounts.rs:192`); the reconcile tick passes the CACHED value straight through
  (`service/sessions/reconcile.rs:765,878` `claude_version: host.claude_version.as_deref()` →
  `store/reconcile.rs:197`), and the `host:pinged` event patches only `reachable`/`last_pinged_at`
  (`hosts.ts:121-133`). Live: hub says mac 2.1.235 / mefistos 2.1.234 / oci 2.1.220 / htz 2.1.214 vs
  trn 2.1.277 → four `⬆` badges; SSH shows mac 2.1.282, mefistos 2.1.267, oci 2.1.282 — newer than
  trn. Only htz is truly old, and its badge looks like the other three.
- **Related.** UX-68 (`?` without a reason), UXPR-22/23/35 (usage marks), UX-73.
- **Proposed.** Stamp `claude_version_at`; the reconcile probe runs `claude --version` when the stamp
  is older than 6 h (one extra command per host per 6 h, the tmux `list-sessions` already goes over
  the same connection); `HostRow` gains `claude_version_at` (`#[serde(default)]`, contract-neutral);
  the badge renders only when the stamp is fresh and reads `2.1.214 · checked 2 h ago`.
- **Effort.** S.

### F-14 · P1 · Disk 98 % on two hosts, a 57-day uptime, a 0.2.26 agent, and load 5–6 are invisible — there is no host health at all
- **Evidence.** `HostRow` has `reachable`, `claude_version`, `tmux_version`, `last_pinged_at`,
  `transport`, `org_id` and nothing else (`crates/fleet-core/src/store/rows.rs:637-658`,
  `hosts.ts:6-19`); `HostDetail.svelte:281-314` shows online/offline, last ping, claude, tmux.
  `fleet-agent`'s `agent_version` (0.2.26 on trn, a hub at 0.3.1) is not in `HostRow` and appears in
  no view (`grep agent_version src/lib` → nothing). Live: mefistos 98 % (14 G free), htz 98 % (3.6 G
  free, 7.7 G RAM), mac 90 %; the desktop DB's `stuck` history shows session 21480 hitting `oom` eight
  times in 80 minutes with the `oom_recreate` playbook looping — the one signal that would have
  predicted it (memory/disk) is not collected.
- **Related.** UX-19 (HostDetail), UX-68, UXPR-23 (`HubScopeNote what='hosts'`), UXPR-35 (usage marks
  on the model).
- **Proposed.** Probe extension, contract-neutral (`#[serde(default)]`): `disk_pct`, `disk_free_bytes`
  (`df -Pk "$HOME"`), `load1`, `mem_free_bytes`, `uptime_secs`, and for agent hosts `agent_version`
  from the registry. Surfaces: (1) `HostsList` attention gets `disk_full` (≥ 90 warn / ≥ 95 crit)
  ranked above `claude_old`, and `agent_old` when `agent_version < hub version`; (2) HostDetail gets
  a `Health` block (`disk 98 % · 3.6 G free · load 1.2 · up 144 d · agent 0.2.26 ⬆`); (3) the sidebar
  host pill gets a red dot for crit; (4) `fleet_health.hosts[]` carries the same so the operator
  agent and the phone see it; (5) the `oom_recreate` playbook refuses to loop on a host whose
  `mem_free` is below a floor and raises `stuck: oom` to `Needs you` instead.
- **Effort.** M.

---

## 6. Smoothness: optimistic merge, events, terminal

### F-15 · P3 · The render path is sound at 63 rows; one duplicate per-row computation
- **Evidence.** Every `{#each}` is keyed (`Sidebar.svelte:991,1051,1123,1162,1181,1199`); indices are
  built once per store change (`sidebar_index.ts` header comment; `Sidebar.svelte:376-403,530-566`);
  events are batched on a timer (`events.ts:105-134`) and applied in one `sessions.update`
  (`sessions.ts:568-577`); rows are merged by identity so only the changed `SessionRowItem`
  re-renders (`row_store.ts` `mergeInto`); the clock ticks every 30 s (`Sidebar.svelte:177`).
  `workKeyFor(sess, branchById)` is called twice per row per render (`Sidebar.svelte:922-923`).
- **Proposed.** Memoise `workKeyFor` in the `workIndex` derived; nothing else.
- **Effort.** S.

### F-16 · P2 · Order churn (F-08) is the smoothness problem, not rendering
- **Evidence.** See F-08: `loadSessions` takes ORDER from the backend every time (`sessions.ts:283-290`),
  the focus refetch every 30 s, `last_activity_at` rewritten per reconcile. Keyed blocks make the
  DOM move rather than repaint, which is exactly what reads as "the list jumped".
- **Proposed.** Same fix as F-08 (a deterministic in-project sort makes list order a pure function
  of row content, so refetches cannot move a row whose content did not change).
- **Effort.** S (covered by F-08).

### F-17 · P1 · 30 of 43 sessions cannot be attached from this desktop, and the app finds out after trying, every time
- **Evidence.** `pty_open` spawns `ssh <alias>` with this machine's config — the hub is not in the
  path (`src-tauri/src/pty.rs:500-511`); `claude-fleet-trn` is `transport: agent` and not
  SSH-reachable from here. `TerminalView.svelte:1076-1090` deliberately attaches anyway and explains
  only after `openError` (`:1239-1247`). `resolveSessionView` (`session_view.ts`) returns the
  preference (terminal by default) whenever the row has a `claude_session_id`, so selecting any trn
  row means: `closeTerm` → new `Screen` → `pty_open` → ssh failure → error text, then a manual
  switch to Conversation.
- **Related.** UX-25 (`↻ reconnect`), UX-01 (glyphs), 11-expert-review §4 (IPC timeouts).
- **Proposed.** Remember the last attach outcome per host for the process lifetime; when the host is
  `agent` transport and the last attach failed (or none was tried and no `Host` block matches),
  `resolveSessionView` returns `conversation` and the terminal tab shows a one-line
  `Terminal: no SSH route to claude-fleet-trn from this machine · Try anyway` instead of spawning
  ssh. Conversation is fully routed (`session_conversation`, `send_prompt`), so nothing is lost.
- **Effort.** S.

### F-18 · P2 · The sidebar's Refresh is a full fleet reconcile on the hub, with no feedback and no "last updated"
- **Evidence.** `Sidebar.svelte:355-357` → `loadSessions({ force: true })` →
  `remote.rs:707-711` `list_sessions { force: true }` → `session_ops.rs:46-50` `refresh_sessions`
  = reconcile every reachable host now, in addition to the hub's own 20 s tick
  (`reconcile.interval_secs` default 20, `settings.rs:209`). The button only disables while loading
  (`SidebarFilters.svelte:101`); nothing on screen says when the list was last refreshed or that the
  hub is ticking. The desktop's own tick is off in hub mode ("skipping the reconcile tick" in the
  log), which is right, but the UI still presents Refresh as the way to get fresh rows.
- **Related.** UX-01, UX-73 (false "every 5 min"), consolidation-01 Q 03-Q5.
- **Proposed.** In hub mode label the control `Re-list` (calls `list_sessions` without `force`) and
  show `hub tick 20 s · updated 12 s ago` next to it (from the newest `row_version`/event time);
  keep the forced reconcile per host in HostDetail (`probe_host` routes already).
- **Effort.** S.

---

## 7. The operator (`fleet-operator`)

### F-19 · P1 · The agent that acts on the user's behalf is an unmarked ordinary row, and its panel state is a snapshot taken when the panel opened
- **Evidence.** The operator is a `kind: "work"` session named `fleet-operator` under the system
  project `operator` (`crates/fleet-core/src/service/operator.rs:119-125,425`); `projects.ts` `system`
  only hides the project from pickers, the tree draws the row like any other
  (`Sidebar.svelte:572-580`), and `SessionRowItem` has no operator branch (no match for
  `operator|controller` in it, `attention.ts`, or `session_view.ts`). Clicking it attaches a
  terminal to the agent. The panel's state (`operatorState`) is set only by `refreshOperator()`
  inside `openAgent()` (`operator.ts:246-262`); no event updates it, so the panel keeps saying
  "ready" while the row goes `blocked`, `stuck` or `lost` — which is the mechanism behind UX-13's
  "state changed between two openings", closed in iteration 11 as unreproducible from code.
  Live: operator on `mefistos`, opus-5, context 50 %, bypass permissions; it appears in the tree as
  `fleet-operator` under `operator`, identical to `pd2758-e2e` next to it.
- **Related.** UX-13, UX-14, UX-15, consolidation-01 §5 / -02 lens 11 (operator in the sidebar,
  `IconAgent`, "Fleet operator" as `chosen`), UXPR-04.
- **Proposed.** (a) Row: `IconAgent` badge + label `Fleet operator` (via `displayName`), click opens
  the Agent panel in the last open mode (`openAgent()`), with `Open terminal` inside the panel
  header; the row is excluded from bulk selection and from `Needs you` counts except when
  `claude_status === 'blocked'`, where the entry reads `operator is waiting for you`.
  (b) `operatorState` becomes a derived of `operatorRow` + the last status call: `lost_at` → `lost`,
  `stuck_kind` → shown as the header badge, `claude_status: blocked` → header `waiting for you` with
  the existing `AnswerPrompt` bound to `pending_input` — the row events the app already receives
  drive it, no polling. (c) `refreshOperator()` also runs on `session:updated` for the operator's
  id when `operatorState` is a blocked kind, so `token_revoked`/`no_host` clear themselves.
- **Effort.** M.

### F-20 · P2 · Two meanings of "blocked", and one recovery instruction that points at a refusal
- **Evidence.** `OperatorBlocked` (`operator.ts:9`: absent / lost / no_mcp / token_revoked /
  no_host) is a panel state; `claude_status: blocked` is the row's "waiting for input" — the same
  word in the header badge (`AgentPanel.svelte:98-107` `stateBadge` prints the raw state). The
  `token_revoked` copy says "Kill its session from the sidebar and press the button again"
  (`operator.ts:174-177`), while `refuse_if_operator` (`operator.rs:813-824`) refuses
  `kill_session` on the operator row — if the desktop's Kill goes through the same service path the
  instruction is a dead end; either way it sends the user to the sidebar where the row is unmarked
  (F-19).
- **Proposed.** Rename the panel states in copy (`not running`, `session lost`, `no tools`,
  `no access`, `no host`), keep `blocked` for the row's status only; `token_revoked` gets its own
  action `Re-mint token and restart` (ensure_operator with `rotate: true`) instead of prose.
- **Effort.** S.

---

## Prioritised top 10

| # | Finding | Sev | Effort | Why first |
|---|---|---|---|---|
| 1 | F-01 lost `bg:` rows drawn as live (`working` on rows lost a day ago), no dismiss | P1 | S | Wrong picture for 15 of the 63 rows; pure TS |
| 2 | F-17 default to Conversation for agent-transport hosts instead of a failing attach | P1 | S | Affects 30 of 43 sessions on every selection |
| 3 | F-08 + F-16 in-project triage sort, alphabetical idle tail, `+n idle` fold | P1 | S | Makes 43 rows scannable and stops the reshuffle; pure TS |
| 4 | F-11 footer `app 0.2.42 · hub 0.3.1 · contract 4`, `hub_version` in HubStatus | P1 | S | The user currently reads the hub's version as the app's |
| 5 | F-13 `claude_version_at` + refresh on the tick; badge only when fresh | P1 | S | The one health badge that exists is wrong on 3 of 4 hosts |
| 6 | F-04 `dismiss_ghost_sessions` hub tool + "Dismiss all lost (n)" | P1 | M | 19 ghosts, 16 of them un-dismissable today |
| 7 | F-14 host health probe (disk/load/mem/agent_version) + badges + `fleet_health.hosts[]` | P1 | M | 98 % disk on two hosts and a 0.2.26 agent are invisible; feeds the oom loop |
| 8 | F-19 operator row badge, panel state derived from the live row | P1 | M | The user cannot tell which row is the agent acting for them |
| 9 | F-02 hidden-host rows out of the tree; hub reaps hidden hosts; dedupe by uuid | P1 | M | Frozen duplicates; usage double-counted |
| 10 | F-09 one context threshold, `context_full` bucket in `Needs you` | P1 | S | 5 `context_red` sessions are the real attention signal in this fleet and are invisible |

Then: F-06 (uuid labels, S), F-07 (agent fold + retire, M), F-05 (`lost` bucket, S), F-18 (Re-list +
"updated 12 s ago", S), F-12 (interim usage from `fleet_health`, S), F-03, F-10, F-20, F-15.

Items 1–5 and 10 are TS-only or contract-neutral and can ship as one small PR each without waiting
for the parity lane (UXPR-13/22/26), which this worktree shows has not landed.
