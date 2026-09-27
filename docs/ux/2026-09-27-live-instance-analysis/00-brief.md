# claude-fleet live instance — shared brief for expert analysis (2026-09-27)

Read-only analysis. NEVER kill/restart/recreate/send_prompt/broadcast/delete anything. No git pull/push/checkout/stash. Do not read ~/.claude.json, fleet-hook.headers contents, tokens, secrets, keychain.

## Topology
- Desktop app (this Mac) runs in HUB-CLIENT mode: desktop version **0.2.42**, paired to hub `https://fleet.rlt.sk` as client `mac-desktop`, contract 4. Desktop log says: "remote backend: skipping the reconcile tick, the account-usage poll and the embedded control API — the hub owns this fleet; following its event stream instead".
- Hub: `fleet-hub` **0.3.1**, docker compose on NAS host `nas` at /volume1/docker/fleet-hub (image ghcr.io/martin-janci/fleet-hub:0.3.1, ./data bind mount, ports 4180, ingress cloudflared → NAS caddy :5080 → :4180). Dir has backup-0.2.42-*, backup-0.3.0-*, data.pre-0.2.38/40/41/42 dirs and 15 docker-compose.yml.<ver> copies. Docker/hub logs need sudo on nas — NOT available to us (sudo requires password); do not try.
- The MCP server `claude-fleet` available to you IS the hub (master token). fleet_health: version 0.3.1, schema 60, db_ready, sessions_total 43, by_status idle 38 / working 3 / unknown 2, ghosts 3, context_red 5, hosts_reachable 5/6, stuck 0, tunnels {}, peer_links_down 0, trackers none.
- fleet-agent on `claude-fleet-trn` (transport agent): agent_version **0.2.26**, connected since 1790454247, linux.
- Operator session (operator_status): ready, host mefistos, session 21535 `fleet-operator`, model claude-opus-5, context_pct 50, bypass permissions.

## Hosts (list_hosts from hub) vs what SSH shows on the host
| alias | transport | hub says claude | actual claude (ssh) | tmux | notes |
|---|---|---|---|---|---|
| local | ssh | – | – | – | hidden=true, reachable=false, provisioned, last_pinged 1789929074 (2026-09-20). Legacy alias of this Mac before rename to `mac`. |
| mac | ssh | 2.1.235 | **2.1.282** | none? (`tmux ls` = no server) | disk 90% (45G free), load 5-6 |
| mefistos | ssh | 2.1.234 | **2.1.267** | 3.6a, 12 sessions | disk **98%** (14G free), 32G RAM, up 57d |
| claude-fleet-htz | ssh | 2.1.214 | 2.1.214 | 3.3a, 0 sessions | disk **98%** (3.6G free!), 7.7G RAM, up 144d; minimal skills set only |
| claude-fleet-oci | ssh | 2.1.220 | **2.1.282** | 3.3a, 1 session | disk 25%, 24G RAM |
| claude-fleet-trn | agent | 2.1.277 | (not ssh-reachable from here) | 3.3a | 30 sessions, most papayapos backend/frontend |
=> `claude_version` shown by list_hosts is stale on 3 of 5 hosts (cached at provision/probe time, not refreshed).

Provisioned files on every SSH host: ~/.claude/CLAUDE.md managed block (BEGIN/END sentinels, points at claude-fleet-control + fleet-friendly-name skills), ~/.claude/fleet-hook.headers (87 bytes, 0600, Sep 20), ~/.claude/settings.json with hooks: SessionStart = curl POST with @fleet-hook.headers and X-Fleet-Pane header; Stop/UserPromptSubmit/Notification/PreCompact/PostCompact/SessionEnd/StopFailure entries exist (command field empty → probably `type: http` hooks). Skills dir: mac/mefistos/oci have the full user skill set (~90 skills, synced from dotfiles), htz has only 11 (claude-creds claude-fleet-control connect-to-child diagnose-why-work-stopped fleet-friendly-name paperclip* para-memory-files unlazy). mefistos also has graft hooks + rtk PreToolUse; htz has rtk PreToolUse; oci none of those.

## Sessions (hub, include_lost=true) — 63 rows total
- Running: mac 5 `bg:` background sessions (project 13 claude-fleet; 4 idle 1 working); claude-fleet-trn 30 (papayapos-backend 17, pos-frontend 9, claude-fleet 1 working `violet-mars`, kuk-agent 1); mefistos 11 (fleet-operator controller, claude-fleet x4 incl. `sleek-castor-term` shell with null claude_status, sales-twins x3, kuk-agent x2, pd2758-e2e, support-agent); oci 1 (`noble-virgo-term`, sales-twins).
- Ghosts (lost_at set): mac 13 (bg:* x9, dev-martin-janci-claude-fleet--bright-vega, dev-martin-janci-claude-fleet, fleet-probe), `local` 6 (all bg:*, lost 1790431981). fleet_health says ghosts=3 while 19 rows carry lost_at — count mismatch to explain.
- DUPLICATE bg rows across the renamed host: bg:cb0bab25…, bg:11d6176e…, bg:a0336855… exist BOTH as `mac` ghosts (ids 21504/21505/21507) AND `local` ghosts (21490/21491/21498); `local` copies still say claude_status working.
- Several ghost bg rows on `local` show claude_status `working` although lost since 2026-09-26.
- fleet_health.usage_by_host lists ONLY `mac` (cost 503 USD-ish over 7d) although sessions run on trn/mefistos; usage_by_day totals are much larger than mac alone (e.g. 2026-09-21 cost_micros 849,590,325). Possible per-host usage collection gap on the hub for SSH/agent hosts, or by_host = live rows only.
- Projects: 77 rows; many one-worktree cruft rows, ppt-epic-145..150 with worktree_count 0, `fleet/operator` id 82 (worktree 0). sales-twins-app 27 worktrees, property-management 27, papayapos-backend 18.

## Desktop local DB (pre-hub-mode snapshot, stale since 2026-09-25 20:24)
Copied to: /private/tmp/claude-501/-Users-martinjanci-projects-github-com-martin-janci-claude-fleet--claude-worktrees-ux-errors-analysis-prep-f5c1e7/e817d269-b850-497b-b5c0-e565a380e03c/scratchpad/db/state.db (sqlite3, integrity ok, WAL). Tables: accounts 6, hosts 5, projects 77, sessions 40 (host_alias `local` 6 external, mefistos 17, trn 8, htz 6, oci 3), session_events 2948 (status_change 2674, turn_done 188, notification 28, stuck 22, prompt_sent 15, playbook_applied 5…), worktrees 350 (local 249, trn 52, mefistos 49), usage_daily 32, dismissed_agents 5, conversations 26, settings 12 (mcp.enabled true port 4180, controller.host claude-fleet-trn / controller.tmux review-pd2713 (stale), repair.auto_on_tick true, playbooks.press_enter true, playbooks.oom_recreate true, gc.enabled true, hub.remote_url https://fleet.rlt.sk, hub.client_name mac-desktop).
Stuck history: session 21480 hit `oom` 8 times between 2026-09-19 23:46 and 2026-09-20 01:09 (oom_recreate playbook on, still looping); 21340 oom 7x on 09-11; auth_menu 21485 on 09-20; reconnect 21446; stop_failure 429 rate_limit on 20773 (09-18).

## Desktop logs
~/Library/Application Support/sk.rlt.claude-fleet/logs/claude-fleet.YYYY-MM-DD-HH.log (hourly). Tails of every file: /private/tmp/claude-501/-Users-martinjanci-projects-github-com-martin-janci-claude-fleet--claude-worktrees-ux-errors-analysis-prep-f5c1e7/e817d269-b850-497b-b5c0-e565a380e03c/scratchpad/desktop-log-tails.txt. Only WARN lines (195, none ERROR): `fleet_core::service::tunnel: [tunnel] ssh exited; restarting host=claude-fleet-trn exit_code=0 restart_in=30s` x133 (every ~30s through 2026-09-19 19:00–23:59, i.e. before hub mode), plus exit_code=255 restart loops for mefistos/oci/htz x14 each with 1/2/4/8/16/30s backoff. Recent lines are only startup + "[hub events] subscribed" (kinds: session host account project worktree task account_usage asset_inventory catalog sync move work). App restarted 08:51 and 08:53 today (two starts 2 min apart).

## Repo
Worktree: /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/ux-errors-analysis-prep-f5c1e7 (branch feature/instance-db-agents-analysis-0e4558). Read CLAUDE.md there first. Use `graft ask/grep/callers/skeleton` (already indexed) before reading files. Key dirs: crates/fleet-core/src/{service,store,mcp,ssh.rs,tmux.rs,events.rs}, crates/fleet-hub, crates/fleet-agent, crates/fleet-proto, src-tauri/src/backend (hub-client mode), src/lib (Svelte stores), docs/hub.md, docs/ux/2026-09-21-audit (existing UX audit — do not repeat it, reference it).

## MCP token discipline
list_sessions default summary is ~3k tokens; summary:false ~11k — use filters (host_alias, project_id, limit) and peer_status for single rows. Never poll in loops. usage_report / list_worktrees {project_id} / session_history {session_id} are cheap and allowed.
