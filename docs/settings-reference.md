<!-- GENERATED FILE — do not edit by hand.
     Regenerate with: REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current -->

# Settings reference

Every operator setting fleet stores, generated from the registry in `crates/fleet-core/src/service/settings.rs`. Change one in Settings on the desktop, or over the control API with the master token (`set_setting`); `get_settings { describe: true }` returns this same metadata with each setting's current value.

## Reconcile tick

| Setting | Default | Range | What it does |
|---|---|---|---|
| `reconcile.interval_secs` | `20` | seconds, `0` = off | Seconds between background reconcile passes, which refresh session state on every host. Applies after a restart. |
| `reconcile.stale_working_secs` | `1800` | seconds, `0` = never | How long a working session may go without a hook, a turn, transcript growth or pane output before it reads idle. |

## Sessions

| Setting | Default | Range | What it does |
|---|---|---|---|
| `sessions.lost_ttl_secs` | `1209600` | seconds, shown in hours, `0` = removed on the next pass | How long a resumable session lost to a host reboot or the tmux server exiting is kept before it is deleted, counted from when it was lost. |

## Restoring lost sessions

| Setting | Default | Range | What it does |
|---|---|---|---|
| `restore.batch_size` | `4` | 1–16 | Sessions resumed in parallel by Restore lost sessions. |
| `restore.stagger_ms` | `3000` | 0–60000 ms | Pause between starting each resumed session in a batch restore. |

## Playbooks

| Setting | Default | Range | What it does |
|---|---|---|---|
| `playbooks.press_enter` | `false` | on / off | Press Enter for sessions stuck on a "Press Enter" prompt. Auth menus, trust prompts and reconnects are always notify-only. |
| `playbooks.oom_recreate` | `false` | on / off | Recreate a session that ran out of memory, within the budget below. |
| `playbooks.oom_max_attempts` | `2` | 0–20, `0` = never | Recreates one session may get per 24 hours. A session that is working, or finished a turn after the flag, is never recreated. |

## Garbage collection

| Setting | Default | Range | What it does |
|---|---|---|---|
| `gc.enabled` | `false` | on / off | Stop or remove sessions that have been idle longer than the limits below. Asks to confirm. |
| `gc.bg_idle_secs` | `86400` | seconds, shown in hours, `0` = never | How long a background agent may sit idle before it is stopped. |
| `gc.shell_idle_secs` | `604800` | seconds, shown in hours, `0` = never | How long a shell session may sit inactive before it is killed. |
| `gc.work_idle_secs` | `0` | seconds, shown in hours, `0` = never | How long a work session may sit idle before it is removed. A dirty worktree goes through safe remove. |
| `gc.sweep_interval_secs` | `300` | seconds, `0` = off | Seconds between garbage-collection sweeps. |
| `gc.external_lost_ttl_secs` | `3600` | seconds, shown in hours, `0` = the next pass | How long a lost session from outside fleet is kept before it is removed. It can never be resumed; this only rides out a restart. |

## Projects

| Setting | Default | Range | What it does |
|---|---|---|---|
| `projects.base_path` | `{}` | JSON map: host alias → path | Per-host folder that holds your repositories. A host with no entry uses $CLAUDE_FLEET_PROJECTS_BASE (local only), then the layout default. |
| `projects.layout` | `github` | `github` / `flat` | Where a repository sits under the projects root: github puts it at root/owner/repo, flat at root/repo. |

## Tasks

| Setting | Default | Range | What it does |
|---|---|---|---|
| `tasks.max_age_secs` | `86400` | seconds, shown in hours, `0` = never | How long an open task (counted from its start, else its creation) may run before the liveness sweep fails it. |

## Workspace repair

| Setting | Default | Range | What it does |
|---|---|---|---|
| `repair.auto_on_tick` | `false` | on / off | Re-add deleted worktree directories without anyone opening them. A stale entry is dropped only when its parent folder is the one seen while it was healthy, so an unmounted volume is never touched. |
| `repair.tick_interval_secs` | `600` | ≥ 60 seconds | Seconds between automatic workspace checks, each repairing at most five worktrees. |

## Move to host

| Setting | Default | Range | What it does |
|---|---|---|---|
| `move.max_transcript_mb` | `200` | 1–4096 MiB | Largest transcript Move to host copies; a bigger one is refused. |
| `move.max_bundle_mb` | `500` | 1–4096 MiB | Largest git bundle of unpushed work Move to host relays; a bigger one is refused. |
| `move.ignored_entry_kb` | `1024` | 1–1048576 KiB | Largest single git-ignored file or directory Move to host carries; bigger ones are left behind. |
| `move.ignored_total_mb` | `20` | 1–1024 MiB | Total git-ignored payload Move to host carries. |
| `move.max_session_state_mb` | `200` | 1–4096 MiB | Largest per-session Claude directory (subagent transcripts, tool results) Move to host carries; above it the biggest files stay behind. |
| `move.wait_max_mins` | `240` | 1–10080 minutes | How long "Transfer when it finishes" waits for the session to go idle before giving up. |

## Usage

| Setting | Default | Range | What it does |
|---|---|---|---|
| `usage.enabled` | `true` | on / off | Sum each session's token usage from its Claude transcript and show an estimated cost. |
| `usage.interval_secs` | `300` | seconds, `0` = off | Seconds between usage passes, one batched read per host. |
| `usage.prices_json` | `{}` | JSON map: model → USD per million tokens | Per-model prices for the estimated cost, in USD per million tokens (input, output, cache_write, cache_read). {} uses the built-in prices only. |

## Error reports

| Setting | Default | Range | What it does |
|---|---|---|---|
| `reports.max_rows` | `5000` | 100–100000 | Newest error and warning reports kept; older ones are pruned on every insert. |
| `reports.max_age_secs` | `604800` | seconds, shown in hours, `0` = never | How long an error or warning report is kept before the age sweep deletes it. |

## Health

| Setting | Default | Range | What it does |
|---|---|---|---|
| `health.context_red_pct` | `85` | 1–100% | Percent of the context window at which a session needs you. The chip turns red here and amber 15 points below. |

## Work graph

| Setting | Default | Range | What it does |
|---|---|---|---|
| `work.retention.journal_days` | `365` | 0–3650 days, `0` = forever | Days a work journal row is kept once its conversation ended and its work is done or unlinked. |
| `work.retention.tracker_items_days` | `180` | 0–3650 days, `0` = forever | Days a done ticket that no session links to is kept in the cache. |
| `work.retention.timeline_work_events_days` | `180` | 0–3650 days, `0` = forever | Days handover, nudge, tidy and withdrawn-suggestion timeline events are kept; the newest of each kind per session always stays. |
| `work.recent_days` | `14` | 1–365 days | How long ended work with no live session keeps a sidebar group. |
| `work.sync_interval_secs` | `300` | seconds, `0` = off | Seconds between tracker sync passes. Under a minute is raised to one. Applies after a restart. |
| `work.trusted_branch_projects` | `[]` | JSON array of ids | Projects where a sole ticket key in the branch name links automatically; elsewhere it is a suggestion. Set from the work popover. |
| `work.evidence_snippets` | `true` | on / off | Keep a short, redacted prompt snippet around a detected ticket key as evidence. Off keeps only the matched text. |
| `work.session_start_context` | `false` | on / off | Give Claude the linked ticket at session start. Makes the start hook synchronous, which can add up to 2 s when the hub is down. Experimental. Applies when the hooks are next installed. |
| `work.classify_nudge` | `false` | on / off | After three prompts with no ticket, ask Claude once which of your few open tickets it is on. Its answer is only ever a suggestion. Experimental. |
| `work.summary_model` | `haiku` | `haiku` / `sonnet` / `opus` | The model Summarise runs on for a past session, on that session's own host and account. |
| `work.tidy_done_days` | `2` | 1–365 days | Days a linked ticket must be done before Tidy up suggests its session. |
| `work.tidy_idle_hours` | `4` | 1–720 hours | Hours a session must be idle before any tidy reason suggests it. |
| `work.tidy_idle_unlinked_days` | `7` | 1–90 days | Days a session with no work linked must sit idle and unprompted before Tidy up suggests it. Only ever suggested, never auto-tidied. |
| `work.auto_tidy` | `false` | on / off | Let the GC sweep act on the allowed tidy reasons by itself, by safe kill or archive only. Off, Tidy up only suggests. An organisation can override it. Asks to confirm. |
| `work.auto_tidy_reasons` | `done_idle,pr_merged_idle` | any of `done_idle`, `pr_merged_idle`, `not_planned` | The tidy reasons auto-tidy may act on. |

## Decisions (Jev)

| Setting | Default | Range | What it does |
|---|---|---|---|
| `decide.jev.enabled` | `false` | on / off | The kill switch for TypeSafe's decision model. Off, nothing is ever sent. On, data goes only for organisations that opted in, redacted. Experimental. Asks to confirm. |
| `decide.jev.status_map` | `off` | `off` / `shadow` / `assist` | Proposing a status category for an Asana section. Shadow only records; assist suggests. Experimental. |
| `decide.jev.work_link` | `off` | `off` / `shadow` / `assist` | Choosing a ticket for a session no rule could link. Shadow only records; assist suggests. Experimental. |
| `decide.jev.unassigned` | `false` | on / off | Also send sessions and tickets that belong to no organisation. Experimental. Asks to confirm. |
| `decide.jev.timeout_ms` | `1500` | 100–30000 ms | How long one call may take. A call is never retried. |
| `decide.jev.breaker_failures` | `5` | 1–100 | Failed calls in a row that open the circuit breaker. |
| `decide.jev.breaker_open_secs` | `300` | 10–86400 seconds | How long an open breaker refuses calls. |
| `decide.jev.daily_token_budget` | `2000000` | 0–1000000000 tokens, `0` = none | Input tokens the decision model may be sent per UTC day. At $0.042 per million, the default is under $0.09 a day. |
| `decide.jev.model` | `jev-1.13.0` | `jev-1.13.0` / `jev-latest` | The model version a request names. jev-1.13.0 is pinned; jev-latest follows TypeSafe. |
| `decide.retention_days` | `90` | 0–3650 days, `0` = forever | Days a decision record (ids and numbers, never text) is kept. |
