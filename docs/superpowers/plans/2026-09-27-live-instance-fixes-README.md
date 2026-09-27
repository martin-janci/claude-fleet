# Live-instance fixes — the four plans and how they fit

Spec: `docs/ux/2026-09-27-live-instance-analysis/README.md` (themes T1–T7,
the 25-row code table) and its six lens files. All four plans were written
against `origin/main` @ `7dad1665` (0.3.1); every `file:line` is current.

| Plan | File | Tasks | Effort | Contract |
|---|---|---|---|---|
| A · Session state machine | [2026-09-27-session-state-machine.md](2026-09-27-session-state-machine.md) | 8 | 6 S + 2 M | 2 serde(default) fields on `Health`; revision 4 unchanged |
| B · Host identity & health | [2026-09-27-host-identity-health.md](2026-09-27-host-identity-health.md) | 7 | 4 S + 3 M | `Health.hosts[]` + `HostRow` fields, serde(default); 1 new tool + 2 CLI subcommands |
| C · Desktop UX quick wins | [2026-09-27-desktop-ux-quick-wins.md](2026-09-27-desktop-ux-quick-wins.md) | 8 | 6 S + 2 M | Tasks 1–7 TS/desktop-only; Task 8 one routed tool |
| D · Hub ops & accounting | [2026-09-27-hub-ops-accounting.md](2026-09-27-hub-ops-accounting.md) | 7 | 3 S + 4 M | `Health.hub`, `tunnels_mode`, `peer_links_total`, usage labels, serde(default) |

## Execution order

The plans are independent in *what* they deliver but not in *where* they
write. Run them as **A → D → B → C**, or in parallel only where the shared
files below are not both touched at once.

1. **A first.** It defines the attention reasons (`stop_failed`,
   `context_full`, `stale_working`, `ci_failing`) and the hub-exported
   `Health.context_red_pct` that C's `lost` bucket and B's `disk_low` /
   `claude_behind` reasons rank against. Migration **061**.
2. **D second.** Migration **062**; `Health.hub` / `tunnels_mode` /
   `peer_links_total` land beside A's `Health.context_red_pct` in one more
   `REGEN_HUB_CONTRACT` run. Task 1 (backup + upgrade scripts) has no code
   dependency and can be picked first by anyone.
3. **B third.** Migrations **063–065**; `Health.hosts[]` beside A's and D's
   fields; `active_hosts()` (Task 4) replaces filters that D's Task 7
   (streamed reconcile writes) also touches in `reconcile.rs` — B rebases on D.
4. **C last.** Tasks 1–7 are TypeScript and rebase trivially; Task 1 and 3
   extend `TRIAGE_BUCKETS` / `attention.ts` that A's Task 6 and 8 already
   grew, so C rebases on A. Task 8 (`dismiss_ghost_sessions`) is
   independent of everything and can ship first if bulk dismissal is urgent.

Migration numbers are reserved as above; a plan executed out of order keeps
its number (gaps are fine, collisions are not).

## Shared files (both plans edit the same file)

| File | Plans | Rule |
|---|---|---|
| `crates/fleet-core/src/store/schema.rs` (`MIGRATIONS`) | A, B, D | append-only; numbers reserved above |
| `crates/fleet-core/src/store/rows.rs` (`SessionRow`, `HostRow`) | A, B, D | each adds fields; the 7 `SessionRow` literals A touches are re-touched by nobody else |
| `crates/fleet-core/src/service/health.rs` (`Health`, `summarize`) | A, B, D | one `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` per plan; never hand-merge the golden |
| `crates/fleet-core/src/service/sessions/reconcile.rs` | A (stale-working veto), B (version/health writers, `active_hosts`), D (streamed writes) | D's join rewrite before B's writer additions |
| `crates/fleet-core/src/service/tick.rs` | A (age-out call), B (health probe), D (`TickStats`) | additive; order as above |
| `crates/fleet-core/src/service/settings.rs` + `src/lib/fleet_settings.ts` + `SettingsDialog.svelte` | A, B | the `every_spec_has_a_settings_dialog_row` test forces all three per new key |
| `crates/fleet-core/src/service/hooks.rs` | A (StopFailure class), B (`hosts.last_hook_at`) | additive |
| `crates/fleet-core/src/mcp/tools/fleet.rs` | B, D | `fleet_health` payload; both additive |
| `src/lib/attention.ts` + test | A, C | A defines the reasons, C the `lost` bucket ordering |
| `src/lib/HostDetail.svelte`, `SidebarFilters.svelte` | B, C | B adds the Health block, C the Dismiss-all and Re-list controls |
| `src-tauri/src/backend/events.rs`, `src-tauri/src/lib.rs` | C (HubStatus versions), D (`Last-Event-ID`, startup logging) | additive |

## Decisions the plan authors left to you

Each is stated inside the plan with the default the plan implements.

- **A-1** `playbooks.oom_max_attempts` default 2 per 24 h would still have
  allowed the second recreate of session 21480; the detector rewrite is what
  prevents it. Set the default to 1 if a single recreate per day is the
  intent.
- **A-2** Stale `working` becomes `idle` with `sessions.stale_working_at`
  (a column, migration 061) rather than staying `working` and being derived
  in attention. The plan follows the spec's "becomes idle".
- **A-3** `needs_attention_with(threshold)` keeps an 85 default for callers
  that do not pass the setting. Remove the default path if no caller may drift.
- **A-4** `kind = shell` rows leave `sessions_total` too, so
  `sum(by_status) == sessions_total` holds. Keep them in the total if the
  headline count should include shells.
- **B-1** `merge_host` is LocalOnly (hub tool is master-only; a paired
  desktop holds a client token and would always get `E_FORBIDDEN`), with
  `fleet-hub host merge <from> <into>` as the operator path. Flip to routed
  only if client tokens gain a master-scoped variant.
- **B-2** The git-tree preflight *refuses* re-provisioning into a skills dir
  that is inside a git work tree unless `provision.force_git_tree`. On the
  live fleet that skips `mac` and `mefistos` (dotfiles checkouts) until the
  setting is on or the dirs are untracked. The lens proposed warn-not-refuse.
- **B-3** `retire_local_sessions` hard-deletes `local` rows on the next hub
  start (no 14-day TTL). One-way on the live hub; the runbook takes a
  `.backup` first.
- **C-1** `dismiss_ghost_sessions` keeps `confirm: false` (like the singular
  on main) and confirms in the desktop dialog; a hub-confirmed tool would
  dead-end the desktop button in hub mode.
- **C-2** `displayName` labels `bg:` rows regardless of the friendly-names
  toggle; the operator row is recognised by its fixed tmux name + kind, not
  by the system project.
- **D-1** On a keychain timeout the desktop yields `Backend::Unavailable`
  with a banner, not local mode (two-brains rule). "Show the window before
  resolve" is left out: it needs a `Resolving` backend state across all 173
  verdicts.
- **D-2** The `.env`-pinned image tag lives only in the behind-proxy compose
  variant, because `release.sh` and `check-version-consistency.sh` require
  the literal pin in `deploy/hub/docker-compose.yml`.
- **D-3** `HubGauges` for `/metrics` reads sessions/hosts under the writer
  lock at scrape cadence; route through the read pool if hub latency is strict.
- **D-4** The 1 s `AUTH_FAIL_INTERVAL` may trip the routing test's
  back-to-back bad-bearer requests; the plan names the fix.

## Already on main since the analysis branch (verified by the authors, not re-done)

- `buildOutsideFleet` filters lost rows and `lostReasonLabel('local_disabled')`
  exists (PR #316) — C Task 1 only re-words and adds the `Lost (n)` tail.
- `external` ghosts are excluded from the 14-day TTL and reaped next pass
  (`store/reconcile.rs:616-619`) — A's `gc.external_lost_ttl_secs` is a grace.
- `detect_stuck` already ignores OOM text under live REPL chrome
  (`pane_intel.rs:400-409`) but still whole-word matches `oom` — A Task 2.
- `FLEET_HUB_LOG_DIR` and the hub side of `Last-Event-ID` / `resumed`
  (`events_route.rs:520/543/682`) — D Task 4 is desktop-only.
- `agent_version` is already in the agent `Hello` frame with serde(default)
  — B needs no fleet-proto change.

## Operator actions that no plan runs (from the analysis "Do now")

Backup the hub DB with `sqlite3 .backup`; free disk on htz (`~/.paperclip`
89 GB) and mefistos (fleet cargo targets under `~/.cache`); `chmod 600` the
world-readable `~/.claude/settings.json.bak` on the Mac; upgrade desktop
then agent to 0.3.1; re-provision hosts with the master token. B Task 5
(`fleet-hub host merge local mac`) and B Task 6 (re-provision on hub start)
replace the last two once shipped.
