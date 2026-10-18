<!-- GENERATED FILE — do not edit by hand.
     Regenerate with: REGEN_SETTINGS_DOCS=1 cargo fleet-test -- settings_docs_are_current -->

# Settings reference

Every operator setting fleet stores, generated from the registry in `crates/fleet-core/src/service/settings.rs`. Change one in Settings on the desktop, or over the control API with the master token (`set_setting`); `get_settings { describe: true }` returns this same metadata with each setting's current value.

Scope says where a value lives: *fleet* is one value for the whole fleet, kept on the hub when the desktop is paired with one; *fleet, per org* is the same, and an org may set its own value that its sessions read instead; *per process* is the running app's or hub's own.

## Reconcile tick

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `reconcile.interval_secs` | `20` | seconds, `0` = off | fleet | Seconds between background reconcile passes, which refresh session state on every host. Applies after a restart. |
| `reconcile.stale_working_secs` | `1800` | seconds, `0` = never | fleet | How long a working session may go without a hook, a turn, transcript growth or pane output before it reads idle. |
| `reconcile.stale_working_ttl_secs` | `86400` | seconds, shown in hours, `0` = never by age | fleet | How long a session marked stale asks for a look before the tick lifts the mark on its own. An attach or any hook lifts it sooner. |

## Sessions

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `sessions.lost_ttl_secs` | `1209600` | seconds, shown in hours, `0` = removed on the next pass | fleet | How long a resumable session lost to a host reboot, the tmux server exiting or its process being killed (out of memory) is kept before it is deleted, counted from when it was lost. |

## Restoring lost sessions

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `restore.batch_size` | `4` | 1–16 | fleet | Sessions resumed in parallel by Restore lost sessions. |
| `restore.stagger_ms` | `3000` | 0–60000 ms | fleet | Pause between starting each resumed session in a batch restore. |

## automation

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `automation.paused` | `false` | on / off | fleet | Stop every background job that acts on its own: missions, garbage collection, playbooks, repairs, tracker, catalog and folder syncs, and host refreshes. Reconcile, usage and update checks keep running, and health shows each job as paused. |
| `automation.daily_budget` | `0` | 0–1000000 USD, `0` = none | fleet | What every routine's runs together may spend in one UTC day. Once they have spent it no routine starts a run, Run now included, until the next day. A run already going finishes. |

## routines

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `routines.backoff_on_no_change` | `false` | on / off | fleet | A scheduled routine whose last runs found nothing to do waits twice as long before each next run, up to 6 hours. A run that does work brings the schedule back. |
| `routines.pause_at_limit` | `true` | on / off | fleet | A scheduled routine whose account is at its usage limit waits until the limit resets instead of skipping at every scheduled time. |

## orchestrator

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `orchestrator.enabled` | `true` | on / off | fleet | Let active missions take their next steps: keep their cards and brakes current and, under a person's grant, run what is ready. Off stops every mission's loop at once. |
| `orchestrator.max_level` | `1` | 0–3 | fleet | The most any mission's loop may do on its own, whatever the mission asks and its grant signs. 0: the loop only keeps the cards; 1: it also asks the planner, and a person presses every step; 2: runs, retries, reviews and closes within a grant; 3: also creates the planner's tasks. |
| `orchestrator.headless_checks` | `false` | on / off | fleet | Run a mission's review and its test commands as one Claude run in the implementer's checkout, with read-only tools and only those commands, instead of opening a session for each. Falls back to a session when the checkout moved or a command is not a plain one. Experimental. |
| `orchestrator.headless_model` | `sonnet` | `haiku` / `sonnet` / `opus` | fleet | The Claude model headless reviews and tests run. Experimental. |

## control

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `control.auto_continue` | `true` | on / off | fleet | Only at Within your brief or Autopilot: Control's chat sends the option its agent marked recommended once nobody has answered for the minutes below, once, even with the app closed. Typing or Cancel stops it. Never for a push, a delete or another step hard to undo. |
| `control.auto_continue_minutes` | `10` | 1–1440 minutes | fleet | How long Control's chat waits for your answer before it sends the recommended option. |
| `control.react_to_events` | `true` | on / off | fleet | Control's agent gets a short note when a session it started finishes, gets stuck, fails or is lost, when a mission it handed off waits on you, or when a host goes down or runs low. Only while it is idle, and never while automation is paused. |
| `control.feed_min_interval_secs` | `900` | seconds, `0` = no wait | fleet | The shortest time between two notes to Control's agent. A note goes only when something needs Control (a question, a failure, failing checks, a lost session, a host down); finished turns wait and go out with the next one. |
| `control.switch_at_limit` | `true` | on / off | fleet | When Control's account hits its limit or its login is signed out, resume Control's conversation under another signed-in login with headroom: on its host first, then on another host. |
| `control.sweep` | `true` | on / off | fleet | The note to Control's agent also names a pull request it handed out that has merge conflicts or failing checks, and a mission it handed off that made no progress for a while, once per change. Within your brief it fixes or reassigns them, asking as usual; at Ask first it only tells you. |
| `control.follow_quiet_hours` | `24` | 1–720 hours | fleet | Control stops following a session it started or briefed once it is done, killed or its pull request merged, or after this long without a turn. Briefing it again follows it again. |
| `control.journal` | `true` | on / off | fleet | When Control starts a new conversation (a restart, a move to another host, a compaction or a new chat), fleet first tells it what it was following, what it asked you and what you last said. |

## Playbooks

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `playbooks.press_enter` | `false` | on / off | fleet | Press Enter for sessions stuck on a "Press Enter" prompt. Auth menus, trust prompts and reconnects are always notify-only. |
| `playbooks.oom_recreate` | `false` | on / off | fleet | Recreate a session that ran out of memory, within the budget below. |
| `playbooks.oom_max_attempts` | `2` | 0–20, `0` = never | fleet | Recreates one session may get per 24 hours. A session that is working, or finished a turn after the flag, is never recreated. |

## autofix

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `autofix.auto_safe` | `true` | on / off | fleet | Fix problems whose fix only touches fleet's own files and can be undone, then tell you. Fixes that touch tokens, sign-in or running sessions always wait for you. |
| `autofix.max_attempts` | `3` | 0–10, `0` = never | fleet | How many times one problem is fixed automatically before it waits for you. |
| `autofix.detector.operator_unbound` | `true` | on / off | fleet | Notice when Control's token belongs to nobody, which leaves it unable to see any session. |
| `autofix.detector.host` | `true` | on / off | fleet | Notice hosts with out-of-date fleet files, silent hooks, or an old agent or Claude Code. |
| `autofix.detector.host_mcp_entry` | `true` | on / off | fleet | Every 10 minutes, read the fleet entry in each host's ~/.claude.json (its token is hashed on the host and never sent) and notice one that is missing, points elsewhere or carries the wrong token. |
| `autofix.detector.stuck` | `true` | on / off | fleet | Notice sessions stuck on a reconnect, a trust or sign-in prompt, a Press Enter wait or out of memory, and offer the fix. |

## recovery

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `recovery.auto_restore` | `true` | on / off | fleet | Resume a session when its host killed the process (out of memory), its tmux server stopped or the host rebooted, noted on its timeline. Control is restarted, or moved to another host once its own has been silent for five minutes, and told. |
| `recovery.grace_secs` | `60` | seconds, `0` = at once | fleet | How long after a session is lost before it is brought back, so a reboot is recognised as one and the memory that ran out can settle. |
| `recovery.max_attempts` | `2` | 0–20, `0` = never | fleet | Automatic restores one session may get per 24 hours, failed ones included. Past it the session waits for a person. |
| `recovery.max_mem_pressure` | `20` | 1–100% | fleet | Memory pressure on a host at or over which killed sessions wait to be brought back. The host's out-of-memory killer acts at 50 by default; restoring near it only feeds the next kill. |

## agents

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `agents.poll_interval_secs` | `300` | seconds, `0` = off | fleet | Seconds between two looks of one agent rule at what it watches (a repo's pull requests, a Jira project). An agent works only under a rule a person granted. |
| `agents.max_concurrent` | `3` | 1–10 | fleet | Agent jobs that run at the same time, every agent together. A pull request or a ticket never has two at once. |
| `agents.max_retries` | `3` | 1–10 | fleet | Runs one agent job may get. A failure worth retrying waits 1, 2, 4 … minutes; one that would fail the same way again ends the job at once. |

## Garbage collection

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `gc.enabled` | `false` | on / off | fleet | Stop or remove sessions that have been idle longer than the limits below. Asks to confirm. |
| `gc.bg_idle_secs` | `86400` | seconds, shown in hours, `0` = never | fleet | How long a background agent may sit idle before it is stopped. |
| `gc.shell_idle_secs` | `604800` | seconds, shown in hours, `0` = never | fleet | How long a shell session may sit inactive before it is killed. |
| `gc.work_idle_secs` | `0` | seconds, shown in hours, `0` = never | fleet | How long a work session may sit idle before it is removed. A dirty worktree goes through safe remove. |
| `gc.sweep_interval_secs` | `300` | seconds, `0` = off | fleet | Seconds between garbage-collection sweeps. |
| `gc.external_lost_ttl_secs` | `3600` | seconds, shown in hours, `0` = the next pass | fleet | How long a lost session from outside fleet is kept before it is removed. It can never be resumed; this only rides out a restart. |

## Projects

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `projects.base_path` | `{}` | JSON map: host alias → path | fleet | Per-host folder that holds your repositories. A host with no entry uses $CLAUDE_FLEET_PROJECTS_BASE (local only), then the layout default. |
| `projects.layout` | `github` | `github` / `flat` | fleet | Where a repository sits under the projects root: github puts it at root/owner/repo, flat at root/repo. |

## Tasks

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `tasks.max_age_secs` | `86400` | seconds, shown in hours, `0` = never | fleet | How long an open task (counted from its start, else its creation) may run before the liveness sweep fails it. |

## Workspace repair

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `repair.auto_on_tick` | `false` | on / off | fleet | Re-add deleted worktree directories without anyone opening them. A stale entry is dropped only when its parent folder is the one seen while it was healthy, so an unmounted volume is never touched. |
| `repair.tick_interval_secs` | `600` | ≥ 60 seconds | fleet | Seconds between automatic workspace checks, each repairing at most five worktrees. |

## downloads

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `downloads.max_file_mb` | `100` | 1–4096 MiB | fleet | Largest file a session can send to your devices; a bigger one is refused. |
| `downloads.max_total_mb` | `2048` | 1–1048576 MiB | fleet | How much the kept downloads may take together. A new file pushes out the oldest ones. |
| `downloads.keep_secs` | `604800` | seconds, shown in days, `0` = until removed | fleet | How long a sent file is kept for your devices before it is removed. |

## voice

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `voice.enabled` | `false` | on / off | fleet | Let Claude Code's /voice on a host record from the microphone of the app attached to it. The microphone opens only while you record, for a session you turned 🎤 on for. |
| `voice.max_capture_secs` | `300` | seconds, `0` = no limit | fleet | A recording longer than this is cut off. |
| `voice.claim_ttl_secs` | `1800` | seconds, `0` = never | fleet | A session's 🎤 turns itself off after this long without a recording. |

## Move to host

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `move.max_transcript_mb` | `200` | 1–4096 MiB | fleet | Largest transcript Move to host copies; a bigger one is refused. |
| `move.max_bundle_mb` | `500` | 1–4096 MiB | fleet | Largest git bundle of unpushed work Move to host relays; a bigger one is refused. |
| `move.ignored_entry_kb` | `1024` | 1–1048576 KiB | fleet | Largest single git-ignored file or directory Move to host carries; bigger ones are left behind. |
| `move.ignored_total_mb` | `20` | 1–1024 MiB | fleet | Total git-ignored payload Move to host carries. |
| `move.max_session_state_mb` | `200` | 1–4096 MiB | fleet | Largest per-session Claude directory (subagent transcripts, tool results) Move to host carries; above it the biggest files stay behind. |
| `move.wait_max_mins` | `240` | 1–10080 minutes | fleet | How long "Transfer when it finishes" waits for the session to go idle before giving up. |

## Usage

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `usage.enabled` | `true` | on / off | fleet | Sum each session's token usage from its Claude transcript and show an estimated cost. |
| `usage.interval_secs` | `300` | seconds, `0` = off | fleet | Seconds between usage passes, one batched read per host. |
| `usage.prices_json` | `{}` | JSON map: model → USD per million tokens | fleet | Per-model prices for the estimated cost, in USD per million tokens (input, output, cache_write, cache_read). {} uses the built-in prices only. |

## search

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `search.index_transcripts` | `false` | on / off | fleet | Copy what is said in each session into the hub's search index, so a search finds it in any conversation. Turning it off deletes the copied text. Experimental. Asks to confirm. |
| `search.transcript_days` | `30` | 1–365 days | fleet | Days of conversation text the search index keeps. |

## accounts

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `accounts.pause_at` | `90` | 50–100% | fleet | Used share of an account's 5-hour or weekly window at which starting a session on it asks first and offers the login with the most headroom. |
| `accounts.auto_switch` | `false` | on / off | fleet | When a session's account runs out, resume it under the login on its host with the most headroom without asking. Each person can still turn it off for their own sessions on the Accounts page. |
| `accounts.auto_switch_early` | `false` | on / off | fleet | Also move an idle session once its account passes "Ask before starting at", so its next turn does not stall at the limit. A working session is never moved. |

## Error reports

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `reports.max_rows` | `5000` | 100–100000 | fleet | Newest error and warning reports kept; older ones are pruned on every insert. |
| `reports.max_age_secs` | `604800` | seconds, shown in hours, `0` = never | fleet | How long an error or warning report is kept before the age sweep deletes it. |

## Health

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `health.context_red_pct` | `85` | 1–100% | fleet | Percent of the context window at which a session needs you. The chip turns red here and amber 15 points below. |
| `health.version_max_age_secs` | `86400` | seconds, shown in hours | fleet | How old a host's recorded Claude version may be before the "older than the fleet" badge stops trusting it and shows nothing. |
| `health.disk_low_pct` | `90` | 50–100% | fleet | Used share of a host's home filesystem at which the host reads as disk low. |
| `health.mem_low_pct` | `10` | 1–50% | fleet | Available share of a host's memory at or under which the host reads as memory low. Sessions killed there are brought back only once it recovers. |
| `health.cpu_busy_pct` | `90` | 50–100% | fleet | Busy share of a host's CPUs at which the host reads as saturated, when two samples in a row reach it. |
| `health.claude_max_behind` | `30` | 0–1000 | fleet | Patch releases a host's Claude may trail the fleet's newest before the host reads as behind. |
| `health.hooks_silent_secs` | `3600` | seconds, shown in minutes | fleet | How long a reachable host with a live session may send no hook before it reads as hooks silent. |

## provision

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `provision.force_git_tree` | `false` | on / off | fleet | Write fleet's skills even when a git work tree on the host, such as a dotfiles checkout, tracks files in fleet's two skill dirs. Off: provisioning refuses such a host; untracking or ignoring those two dirs is enough. Asks to confirm. |
| `provision.install_ag` | `true` | on / off | fleet | Provisioning installs fleet's ag launcher (~/.local/share/ag) and, when the host has no cl command, a cl shim (claude --yolo) in ~/.local/bin; panes use it when the host has no cl of its own. Off: provisioning leaves ag alone. |

## Work graph

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `work.retention.journal_days` | `365` | 0–3650 days, `0` = forever | fleet | Days a work journal row is kept once its conversation ended and its work is done or unlinked. |
| `work.retention.tracker_items_days` | `180` | 0–3650 days, `0` = forever | fleet | Days a done ticket that no session links to is kept in the cache. |
| `work.retention.timeline_work_events_days` | `180` | 0–3650 days, `0` = forever | fleet | Days handover, nudge, tidy and withdrawn-suggestion timeline events are kept; the newest of each kind per session always stays. |
| `work.recent_days` | `14` | 1–365 days | fleet | How long ended work with no live session keeps a sidebar group. |
| `work.sync_interval_secs` | `300` | seconds, `0` = off | fleet | Seconds between tracker sync passes. Under a minute is raised to one. Applies after a restart. |
| `work.describe_cache_secs` | `300` | seconds, shown in minutes, `0` = off | fleet | How long a fetched ticket description is reused before the tracker is asked again; never longer than the done-tickets retention window. |
| `work.trusted_branch_projects` | `[]` | JSON array of ids | fleet | Projects where a sole ticket key in the branch name links automatically; elsewhere it is a suggestion. Set from the work popover. |
| `work.evidence_snippets` | `true` | on / off | fleet | Keep a short, redacted prompt snippet around a detected ticket key as evidence. Off keeps only the matched text. |
| `work.session_start_context` | `false` | on / off | fleet | Give Claude the linked ticket at session start. Makes the start hook synchronous, which can add up to 2 s when the hub is down. Experimental. Applies when the hooks are next installed. |
| `work.classify_nudge` | `false` | on / off | fleet, per org | After three prompts with no ticket, ask Claude once which of your few open tickets it is on. Its answer is only ever a suggestion. Experimental. |
| `work.members_plan_sprints` | `false` | on / off | fleet, per org | Let an organisation's members, not only its admins, create and change its sprints and releases. Viewers never can; anyone can keep personal ones. |
| `work.summary_model` | `haiku` | `haiku` / `sonnet` / `opus` | fleet, per org | The model Summarise runs on for a past session, on that session's own host and account. |
| `work.context_help` | `false` | on / off | fleet | Terminal and composer: Ask asks a model about the line you are typing, with its history as context, on the session's host. A proposed command is put on the line, never run. |
| `work.help_model` | `haiku` | `haiku` / `sonnet` / `opus` | fleet | The model that answers a question asked at a terminal or composer prompt line, on the session's own host and account. |
| `work.draft_commit_messages` | `false` | on / off | fleet | Files tab: Draft writes a commit message from the staged diff with claude -p on the session's host. A draft is text you edit and commit yourself. |
| `work.draft_briefs` | `false` | on / off | fleet | Starting from a ticket: Draft writes the agent's brief from the ticket before the first prompt, on the planned host. |
| `work.draft_task_titles` | `false` | on / off | fleet | New task: once you have written the notes, the title is drafted from them with claude -p on this machine, or on the first reachable host when it is not a fleet host. A draft is text you edit before you create the task. |
| `work.draft_release_notes` | `false` | on / off | fleet | Finish: Draft writes a finished mission's release note from its merged PRs, on the mission's planner host. |
| `work.catch_up_summaries` | `false` | on / off | fleet | A watched session offers "Since 13:20": what it did since you last looked, summarised on its own host and account. |
| `work.attachment_max_mb` | `10` | 1–16 MiB | fleet | The largest file or image a task takes. At most 16 MiB: what one call between the desktop and the hub carries. |
| `work.tidy_done_days` | `2` | 1–365 days | fleet | Days a linked ticket must be done before Tidy up suggests its session. |
| `work.tidy_idle_hours` | `4` | 1–720 hours | fleet | Hours a session must be idle before any tidy reason suggests it. |
| `work.tidy_idle_unlinked_days` | `7` | 1–90 days | fleet, per org | Days a session with no work linked must sit idle and unprompted before Tidy up suggests it. Only ever suggested, never auto-tidied. |
| `work.auto_tidy` | `false` | on / off | fleet | Let the GC sweep act on the allowed tidy reasons by itself, by safe kill or archive only. Off, Tidy up only suggests. An organisation can override it. Asks to confirm. |
| `work.auto_tidy_reasons` | `done_idle,pr_merged_idle` | any of `done_idle`, `pr_merged_idle`, `not_planned` | fleet | The tidy reasons auto-tidy may act on. |

## catalog

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `catalog.scan_check_secs` | `3600` | seconds, shown in minutes, `0` = off | fleet | How often fleet looks for hosts whose asset scan is stale, and rescans them. Under five minutes is raised to five. Applies after a restart. |
| `catalog.scan_max_age_secs` | `86400` | seconds, shown in hours | fleet | A host's assets are rescanned once its last scan is older than this, and every host after the catalog or a sync changes. |
| `catalog.auto` | `true` | on / off | fleet | After each asset scan, hide fleet's own and Claude's internal assets, propose changeset cards, and, for layers already rolled out once, install what a host is missing and adopt identical copies. Never changes, overwrites or removes a copy a host already has. |
| `catalog.auto_push` | `false` | on / off | fleet | Push the catalog repo right after a changeset card commits to it or is undone. Off: push it yourself. |

## update

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `update.track` | `stable` | `stable` / `beta` / `nightly` / `dev` | fleet | Which releases the hub follows for its fleet: stable, beta (release candidates too), nightly (main, at most every two hours) or dev (every green push to main). |
| `update.hub.mode` | `notify` | `manual` / `notify` / `automatic` | fleet | manual: only a pinned version; notify: offer the update; automatic: install it at the next quiet point. |
| `update.agent.mode` | `automatic` | `manual` / `notify` / `automatic` | fleet | The same choice for fleet-agent on hosts the hub cannot reach. |
| `update.desktop.mode` | `notify` | `manual` / `notify` / `automatic` | fleet | The same choice for the desktop app. |
| `update.mobile.mode` | `notify` | `manual` / `notify` | fleet | manual or notify: a phone never installs an update silently. |
| `update.check_interval_secs` | `21600` | ≥ 900 seconds | fleet | How often the hub re-reads the release channel, and clients check again. |
| `update.window` | `` | a daily time range `HH:MM-HH:MM`, or empty for none | fleet | A daily range in UTC, like 02:00-05:00, in which automatic updates install. Outside it they wait; an offer to a person is not held. Empty: any time. |
| `update.rollout_wave_secs` | `3600` | ≥ 300 seconds | fleet | How long each wave of a staged rollout runs before the next opens, if its failure ratio stays under the rollout's halt ratio. |
| `update.mirror` | `false` | on / off | fleet | Serve the agent and hub tarballs, desktop bundles and the phone's APK from this hub, for machines that cannot reach GitHub. Each file is fetched once and checked against the signed release. |

## Decisions (Jev)

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `decide.jev.enabled` | `false` | on / off | fleet | The kill switch for TypeSafe's decision model. Off, nothing is ever sent. On, data goes only for organisations that opted in, redacted. Experimental. Asks to confirm. |
| `decide.jev.status_map` | `off` | `off` / `shadow` / `assist` | fleet | Proposing a status category for an Asana section. Shadow only records; assist suggests. Experimental. |
| `decide.jev.work_link` | `off` | `off` / `shadow` / `assist` | fleet | Choosing a ticket for a session no rule could link. Shadow only records; assist suggests. Experimental. |
| `decide.jev.start_project` | `off` | `off` / `shadow` / `assist` | fleet | Pre-selecting the repository of a task's first start. Shadow only records; assist suggests. Experimental. |
| `decide.jev.sibling_repos` | `off` | `off` / `shadow` / `assist` | fleet | Pre-ticking the other repository a ticket start also needs. Shadow only records; assist suggests. Experimental. |
| `decide.jev.host_placement` | `off` | `off` / `shadow` / `assist` | fleet | Pre-selecting the host of a new session when no rule, limit or offline host decides. Shadow only records; assist suggests. Experimental. |
| `decide.jev.quick_answer` | `off` | `off` / `shadow` / `assist` | fleet | Showing the likely option first in an agent's question or a chat form. Never on a push, a permission or a risky option. Shadow only records; assist suggests. Experimental. |
| `decide.jev.adopt_target` | `off` | `off` / `shadow` / `assist` | fleet | Prefilling the project when you adopt a pane fleet did not start. Shadow only records; assist suggests. Experimental. |
| `decide.jev.restore_target` | `off` | `off` / `shadow` / `assist` | fleet | Prefilling the project when you restore a conversation found on a host. Shadow only records; assist suggests. Experimental. |
| `decide.jev.duplicate` | `off` | `off` / `shadow` / `assist` | fleet | Flagging a proposed task that may duplicate an existing one. Shadow only records; assist suggests. Experimental. |
| `decide.jev.work_placement` | `off` | `off` / `shadow` / `assist` | fleet | Proposing a Work-view group for a new task no rule or person placed. Shadow only records; assist suggests. Experimental. |
| `decide.jev.related_session` | `off` | `off` / `shadow` / `assist` | fleet | Noticing another of your sessions working on the same thing. Shadow only records; assist suggests. Experimental. |
| `decide.jev.control_route` | `off` | `off` / `shadow` / `assist` | fleet | Proposing which mission or session a message typed in Control is about. A short or unclear message gets a question instead. Shadow only records; assist suggests. Experimental. |
| `decide.jev.summary_check` | `off` | `off` / `shadow` / `assist` | fleet | Checking a watcher's summary of a session against its transcript. Shadow only records; assist hides a summary the transcript does not support. Experimental. |
| `decide.jev.turn_outcome` | `off` | `off` / `shadow` / `assist` | fleet | Reading what a turn came to (finished, a question, stuck) from the end of the screen when hooks say nothing. Shadow only records; assist sets the Inbox state, and any hook overrides it. Sends reply text only for organisations that allow it. Experimental. |
| `decide.jev.mission_triage` | `off` | `off` / `shadow` / `assist` | fleet | Proposing a stuck mission's outcome and next step. Never completes a mission or sets Verified. Shadow only records; assist suggests. Experimental. |
| `decide.jev.routine_run_outcome` | `off` | `off` / `shadow` / `assist` | fleet | Reading whether a routine run did work, found nothing to do or needs you, from the end of its screen. Shadow only records; assist sets the outcome, so a run with nothing to do stays out of the Inbox. A failed exit or a rule wins. Sends reply text only for organisations that allow it. Experimental. |
| `decide.jev.pr_triage` | `off` | `off` / `shadow` / `assist` | fleet | Guessing what a stuck pull request needs (a fix, a regenerate, a base merge, a re-run, or a person) when the PR shepherd finds it conflicting or red. Sends the PR's check names and states, no code. Shadow only records; assist is recorded the same way for now. Experimental. |
| `decide.jev.main_ticket` | `off` | `off` / `shadow` / `assist` | fleet | Proposing the main ticket in Review when a session's first prompt names several. A branch naming one decides without Jev. Shadow only records; assist suggests. Experimental. |
| `decide.jev.tracker_duplicate` | `off` | `off` / `shadow` / `assist` | fleet | Flagging in Review a new local task that may be the same work as an open tracker ticket. Shadow only records; assist suggests. Experimental. |
| `decide.jev.resume_or_new` | `off` | `off` / `shadow` / `assist` | fleet | Proposing, in New session, whether to resume a past session of the same work or start fresh. A single recent past session decides without Jev. Shadow only records; assist suggests. Experimental. |
| `decide.jev.limit_switch` | `off` | `off` / `shadow` / `assist` | fleet | Asking which login an out-of-credit session should resume under when it is switched automatically. The login with the most headroom is used either way; Jev's pick is recorded beside it so the two can be compared. Experimental. |
| `decide.jev.restore_timing` | `off` | `off` / `shadow` / `assist` | fleet | Asking whether a killed session due back on a loaded host, or one whose recovery failed, should resume now, wait or move. Off: wait while loaded. Assist follows the answer, never past the low-memory floor; a move is only noted. Experimental. |
| `decide.jev.unassigned` | `false` | on / off | fleet | Also send sessions and tickets that belong to no organisation. Experimental. Asks to confirm. |
| `decide.jev.unassigned_reply` | `false` | on / off | fleet | Also send the reply text of sessions that belong to no organisation (turn outcome), on top of sending unassigned sessions at all. Experimental. Asks to confirm. |
| `decide.jev.timeout_ms` | `1500` | 100–30000 ms | fleet | How long one call may take. A call is never retried. |
| `decide.jev.breaker_failures` | `5` | 1–100 | fleet | Failed calls in a row that open the circuit breaker. |
| `decide.jev.breaker_open_secs` | `300` | 10–86400 seconds | fleet | How long an open breaker refuses calls. |
| `decide.jev.daily_token_budget` | `2000000` | 0–1000000000 tokens, `0` = none | fleet | Input tokens the decision model may be sent per UTC day. At $0.042 per million, the default is under $0.09 a day. |
| `decide.jev.model` | `jev-1.13.0` | `jev-1.13.0` / `jev-latest` | fleet | The model version a request names. jev-1.13.0 is pinned; jev-latest follows TypeSafe. |
| `decide.haiku.shadow` | `false` | on / off | fleet | Also ask Claude every question a Jev use case in shadow or assist asks, on the host below, and record its answer beside Jev's to compare them. Nothing acts on it. Experimental. Asks to confirm. |
| `decide.haiku.host` | `` | text, up to 64 characters | fleet | The host whose Claude account answers. Only questions about its own organisation are sent to it. Experimental. |
| `decide.haiku.model` | `haiku` | `haiku` / `sonnet` / `opus` | fleet | The Claude model the second opinion asks. Experimental. |
| `decide.haiku.daily_calls` | `200` | 0–10000, `0` = none | fleet | Questions the second opinion may ask per UTC day. One at a time; a question that comes while one is running is not asked. Experimental. |
| `decide.retention_days` | `90` | 0–3650 days, `0` = forever | fleet | Days a decision record (ids and numbers, never text) is kept. |
| `decide.jev.floors` | `` | text, up to 8192 characters | fleet | Per-feature confidence floors fitted from runs a person confirmed, corrected or rejected (fleet-hub decide floor fit). A fitted floor only raises a feature's built-in one, and only for the question version and model it was fitted against. |

## Identity and audit

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `auth.audit_retention_days` | `365` | 0–3650 days, `0` = forever | fleet | Days an administrative change (who changed which person, group, role or device) is kept. |
| `auth.device_token_ttl_days` | `90` | 0–3650 days, `0` = never | fleet | Days a paired device may go unused before it is signed out and must be paired again. Peer hubs and updaters are never signed out. |
| `auth.device_token_max_age_days` | `0` | 0–3650 days, `0` = off | fleet | Days after pairing a device is signed out, however often it is used. Peer hubs and updaters are never signed out. |

## Hub daemon (read-only)

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `hub.bind` | `127.0.0.1` | text, up to 255 characters | per process | The address the hub daemon listens on. Loopback unless the daemon was started with a routable bind. Read-only here: change it with fleet-hub serve --bind. |
| `hub.public_url` | `` | text, up to 2048 characters | per process | The URL hosts and paired clients reach the hub at. Empty means loopback with a reverse tunnel per host. Read-only here: change it with fleet-hub serve --public-url. |
| `hub.allowed_hosts` | `` | text, up to 4096 characters | per process | Extra Host header values the hub accepts, comma-separated, besides the ones its bind and public URL imply. Read-only here: change it with fleet-hub serve --allowed-host. |
| `hub.local_host` | `true` | on / off | per process | Whether the hub's own machine is a fleet host. On for the desktop; the daemon turns it off by default. Read-only here: change it with fleet-hub serve --local-host. |
| `hub.allow_plaintext` | `false` | on / off | per process | Whether the hub daemon may serve a routable bind without TLS. Read-only here: change it with fleet-hub serve --allow-plaintext. |
| `hub.tls` | `off` | `off` / `cert` | per process | How the hub daemon terminates TLS: off, behind a proxy, or cert, with its own certificate. Read-only here: change it with fleet-hub serve --tls. |

## Control API (read-only)

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `mcp.enabled` | `false` | on / off | per process | Whether the embedded control API (MCP) runs, so an AI assistant can drive the fleet. Read-only here: change it with Settings → Control API. |
| `mcp.port` | `4180` | 1–65535 | per process | The localhost port the control API listens on. Read-only here: change it with Settings → Control API. |
| `mcp.confirm_destructive` | `false` | on / off | per process | Every destructive control API call waits for a confirmation on the desktop. Read-only here: change it with Settings → Control API. |
| `mcp.broadcast_interval_secs` | `30` | seconds | per process | Shortest time between two broadcast prompts from the same caller. Read-only here: change it with the settings table only. |

## budget

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `budget.org_daily_usd` | `0` | 0–1000000 USD, `0` = none | fleet, per org | Estimated spend an org's sessions may reach in one UTC day before fleet warns. Each org can set its own. Fleet only warns; it never stops a session. |
| `budget.org_monthly_usd` | `0` | 0–10000000 USD, `0` = none | fleet, per org | Estimated spend an org's sessions may reach in one calendar month (UTC) before fleet warns. Each org can set its own. Fleet only warns; it never stops a session. |

## notify

| Setting | Default | Range | Scope | What it does |
|---|---|---|---|---|
| `notify.desktop` | `needs_you,failed,blocked,routine_failed` | any of `needs_you`, `failed`, `blocked`, `done`, `routine_failed` | fleet | The session states the desktop shows a notification for. |
| `notify.phone` | `needs_you,failed,blocked,routine_failed` | any of `needs_you`, `failed`, `blocked`, `done`, `routine_failed` | fleet | The session states the phone shows a notification for. |
| `notify.sound` | `needs_you` | any of `needs_you`, `failed`, `blocked`, `done`, `routine_failed` | fleet | The session states whose notification also plays a sound. |
| `notify.quiet_hours` | `` | a daily time range `HH:MM-HH:MM`, or empty for none | fleet | A daily range, like 22:00-07:30, in which no notification is shown or sounded, on each device's own clock. Empty: none. |
| `notify.quiet_except` | `failed` | any of `needs_you`, `failed`, `blocked`, `done`, `routine_failed` | fleet | The states that still notify during quiet hours. |
