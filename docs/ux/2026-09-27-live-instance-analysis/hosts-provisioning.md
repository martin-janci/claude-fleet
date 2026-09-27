# Host provisioning / setup review — live claude-fleet instance (2026-09-27)

Lens: does what `provision.rs` / `hooks_install.rs` intend match what is on
each host, and is the host setup healthy. Read-only; evidence gathered over
`ssh -o BatchMode=yes` on `mac`, `mefistos`, `claude-fleet-htz`,
`claude-fleet-oci`, over the hub MCP (`list_hosts`, `probe_host
claude-fleet-trn`, `fleet_health`, `agent_status`, `session_history 21535`),
and from the repo at the analysis worktree (HEAD skills identical to
v0.2.38 … v0.3.1). No secrets were read; hook/MCP entries were inspected
through a redacting script (`scratchpad/hostprobe.sh`, raw output in
`scratchpad/probe-<host>.txt`).

Code references: `crates/fleet-core/src/service/provision.rs` (skills,
managed CLAUDE.md block, MCP entry, tmux clipboard, hooks),
`crates/fleet-core/src/service/hooks_install.rs` (`FLEET_HOOK_EVENTS`, 9
events), `crates/fleet-core/src/service/hosts.rs` (`PROBE_SCRIPT`),
`crates/fleet-core/src/service/sessions/reconcile.rs` (host writer),
`crates/fleet-core/src/service/health.rs` (`Health`),
`crates/fleet-proto/src/lib.rs` (`PROTO_VERSION`), `docs/hub.md`.

---

## What matches (so it is not repeated below)

- **Managed `~/.claude/CLAUDE.md` block** — byte-identical to `CLAUDE_MD_BODY`
  on all four SSH hosts (md5 `2ca57b88…` of the sentinel block on each; the
  body constant is unchanged from v0.2.38 through HEAD). The block was
  written 2026-09-11 and has not needed a refresh since.
- **Hooks** — all **9** events of `FLEET_HOOK_EVENTS` are present on all four
  SSH hosts (the brief counted 8; `PostToolUse` with matcher
  `EnterWorktree|ExitWorktree` is the ninth). Every one is `type: "http"`
  → `https://fleet.rlt.sk/hook`, `Authorization: Bearer <71 chars>`,
  `X-Fleet-Pane: $TMUX_PANE`, `allowedEnvVars: ["TMUX_PANE"]`, `timeout 5`;
  `SessionStart` is the async `curl … -H @"$HOME/.claude/fleet-hook.headers"`
  command hook exactly as `session_start_command` renders it. **No host
  points at an old reverse-tunnel `127.0.0.1:4180` URL** in its live
  `settings.json`. The user's own hooks (unlazy Stop, graft on mefistos,
  `rtk` PreToolUse on mefistos/htz, a `claude-code-reviewer` PostToolUse) sit
  beside fleet's untouched — the merge preserved siblings as designed, even
  through user edits on Sep 22 (mefistos) and Sep 25 (mac, htz).
- **`~/.claude.json` MCP entry** — `claude-fleet` → `https://fleet.rlt.sk/mcp`
  with an auth header on all four; `.fleet-bak` written 0600 beside it.
- **`~/.tmux.conf`** — `set -g set-clipboard on` present on all four.
- **Headers file** — `fleet-hook.headers`, 87 B, mode 0600, on all four,
  dated Sep 20 21:04/21:05 CEST (19:04 UTC on htz/oci) — the single
  provisioning run. **The tokens are still current:** the hub is receiving
  hook traffic today (`session_history 21535` shows `turn_done` rows with a
  `claude_session_id` at 1790507060 and `status_change` at 1790507127, i.e.
  Stop/UserPromptSubmit hooks from mefistos are accepted). The hub's tokens
  came over in the copied `state.db`, so "headers age Sep 20" and "hub token
  epoch" are the same event; nothing has rotated since.
- **Protocol** — `PROTO_VERSION = MIN_SUPPORTED_PROTO = 1` at both v0.2.26
  (the agent) and v0.3.1 (the hub); the hub accepts the agent and would log
  and skip any unknown frame kind. `probe_host claude-fleet-trn` over the
  agent transport worked and returned `2.1.282`.

---

## Findings

### F1 — Every host runs stale copies of the two provisioned skills — P1

**Evidence.** All four SSH hosts have `claude-fleet-control/SKILL.md` =
21 129 B, md5 `54bae333…`, and `fleet-friendly-name/SKILL.md` = 4 719 B, md5
`7a56c047…`, both mtime Sep 20 21:04. Walking the repo history, those blobs
are commit `60695fef` (2026-09-20) and `15f5d6f0` (2026-09-11). Every release
from v0.2.38 to v0.3.1 (and HEAD) embeds `8f1339fb` (25 157 B, md5
`0a36bd3e…`) and `d5f28a9c` (5 373 B, md5 `00426292…`). The control skill on
the hosts predates the work-graph M1b–M4 additions (`work` / `work_link`
tools, ticket start, detection chips); the friendly-name skill predates the
`whoami`-based row lookup and the readonly-token rule. Corroboration from
the live operator: its turn at 1790507060 says it had to send `tools/list`
to the hub by hand to learn that `work` and `work_link` exist — the skill it
was given does not mention them.

**Root cause.** Provisioning runs only on an explicit `provision_hosts` (MCP
tool, `mcp/tools/fleet.rs`). Nothing re-runs it when the embedded skill
content changes: the hub was upgraded 15 times (docker-compose copies
0.2.28 → 0.3.1) and each `fleet-hub` binary carried a newer `FLEET_SKILL`
than the hosts, but `include_str!` content is never compared to what is on
the host. In hub-client mode the desktop *refuses* `provision_hosts`
(`Verdict::LocalOnly`, `backend/verdicts.rs:712`) and points at "provision
from the hub with `fleet-hub`", but `fleet-hub` has no `provision`
subcommand — the only path is the MCP tool with the master token, which the
operator cannot call (fleet admin) and the user has not called since Sep 20.

**Proposed fix.**
1. Store a per-host `provision_fingerprint` (sha256 of skill bodies +
   `CLAUDE_MD_BODY` + hook-shape version) in `hosts` (migration), set by
   `provision_one`. On hub start and on each `probe_host`, compare with the
   binary's fingerprint; expose `provision_stale: true` in `list_hosts` and
   `fleet_health.hosts_provision_stale: [alias…]`, and a HostDetail badge
   "Provisioned with an older fleet — re-provision".
2. Add a **non-secret refresh path**: `provision_hosts { only: "content" }`
   (skills + CLAUDE.md block only, no token or `~/.claude.json` rewrite,
   allowed for the operator / a `full` client) and run it automatically on
   hub start when the fingerprint differs (it is idempotent and needs no
   Claude restart — the docs already say skills and hooks are picked up
   live).
3. Add `fleet-hub provision [--content-only]` so the verdict text is true.

**Effort.** M.

### F2 — Provisioning writes into the user's dotfiles git tree; two owners for the same files — P1

**Evidence.** On mefistos `~/.claude/skills` is a symlink to
`../dotfiles/claude/.claude/skills`; on the mac the same two skill files are
tracked in `~/dotfiles` (`git ls-files` lists
`claude/.claude/skills/claude-fleet-control/SKILL.md`,
`fleet-friendly-name/SKILL.md` **and** a `fleet-friendly-name/CLAUDE.md.snippet`
that no longer exists in the fleet repo). On both hosts `git status` shows
those files ` M` (modified, uncommitted) against the last dotfiles commit
that touched them (`776383d`, 2026-07-06). So: fleet's Sep 20 write dirtied
the dotfiles checkout on every host that keeps skills there, and the
dotfiles sync (rsync/stow, the `synced` marker dir on the mac) is a second
writer that can put the July version back — or push one host's copy to the
others — behind fleet's back. Today all four hosts happen to agree only
because the sync copied the Sep 20 files around.

**Root cause.** Neither side declares ownership. `provision.rs` writes plain
files (`write_host_file`) with no marker, and nothing in `docs/hub.md`
says "these two directories are fleet's; do not track them".

**Proposed fix.** Document ownership (see the table under F8), add a
`.fleet-managed` marker file to each shipped skill dir, and have the
provision preflight warn (not refuse) when the target resolves into a git
worktree: "skills dir is inside a git checkout (`~/dotfiles`); fleet will
overwrite tracked files". Recommend the user add the two dirs to
`.gitignore` in dotfiles (or `git rm --cached` them) and delete the orphan
`CLAUDE.md.snippet`. **Effort.** S.

### F3 — `claude_version` / `tmux_version` shown by `list_hosts` are a Sep 20 cache stamped with today's `last_pinged_at` — P2

**Evidence.** Hub `list_hosts` before this review: mac 2.1.235, mefistos
2.1.234, oci 2.1.220, trn 2.1.277 — actual: 2.1.282, 2.1.267, 2.1.282,
2.1.282 (trn from `probe_host`, which then overwrote the cached value). Every
row carries `last_pinged_at = 1790507246` (minutes old), which is
misleading: the reconcile writer passes the *stored* values straight back
(`reconcile.rs:765-766` and `:878-879`: `claude_version:
host.claude_version.as_deref()`) while refreshing `last_pinged_at`. Only
`probe_host` / `add_host` run `PROBE_SCRIPT` (`claude --version`).

**Root cause.** By design the reconcile pass never runs `claude --version`
(cost), but the row's freshness stamp does not distinguish "sessions probed"
from "versions probed".

**Proposed fix (plus the update nudge asked for in Q2).**
- Append `claude --version` to `HOST_IDENTITY_SCRIPT` (already one round
  trip per pass) or run `PROBE_SCRIPT` every Nth pass; add
  `versions_probed_at` to the row.
- **Nudge:** fleet knows the newest Claude Code across the fleet. Add
  `hosts.claude_behind: { newest: "2.1.282", by_releases: n }` to
  `list_hosts`, a `fleet_health.hosts_claude_outdated` list with a setting
  `health.claude_max_behind` (default: older than the newest by > 20 patch
  releases *or* > 30 days), and a HostDetail chip "Claude 2.1.214 — fleet
  newest is 2.1.282; `claude update` on the host". No auto-update (the
  binary is user-owned; htz's is a `~/.local/share/claude/versions/2.1.214`
  symlink that the user simply never advanced, while oci's advanced to
  2.1.282 on Sep 25). htz is 68 patch releases behind and is the only host
  where `StopFailure`/`PostCompact`/`allowedEnvVars` support is uncertain;
  the code names no floor, so the nudge should also carry the floor the hook
  set was validated on.

**Effort.** S (probe) + M (nudge UI + health field).

### F4 — Disk pressure is invisible to fleet, and two hosts are at 98 % — P1

**Evidence (read-only `du`, bounded).**
- **claude-fleet-htz** (a Docker container: `/.dockerenv`, PID 1 = sshd;
  overlay 150 G, **3.6 G free**): `/home/dev` = 100 G, of which
  **`/home/dev/.paperclip` = 89 G** (the Paperclip control-plane instance
  with its embedded Postgres, per the `paperclip-infra` notes); the rest is
  `projects` 8.6 G (sales-twins-app `.claude/worktrees` 2.9 G), `.local`
  6.5 G, `.npm` 3.2 G, `.claude` 2.9 G (`projects` 1.6 G transcripts,
  `plugins` 1.3 G), `/tmp` 5.9 G (`/tmp/claude-1000` 2.4 G in 831 entries,
  `worktree-SAL-696` 1.8 G, three leftover `*-pg*` data dirs).
- **mefistos** (480 G root, **14 G free**): `~/.cache` 54 G, of which
  **41 G `claude-fleet-shared-target`** + 5 G `claude-fleet-rowproj-target`
  (cargo target dirs of fleet's own builds on that host), `~/.cargo-targets`
  18 G, `.local` 7.3 G, `.rustup` 6.5 G, `.npm` 5.8 G, `Android` 6.2 G,
  `.konan` 3.2 G, `build` 2.4 G, `/tmp/claude-1000` 9.3 G (133 entries),
  `/var/log/journal` 4.1 G, linuxbrew 5.7 G. Docker (29.5.0, 22 containers,
  32 images) lives in `/var/lib/docker`, unreadable without sudo, and
  `docker system df` did not answer within 40 s — the ~250 G not
  attributable above is almost certainly there. `~/projects` is a symlink to
  `/mnt/sda4/projects` (1.4 T, 298 G free) — an obvious relocation target
  for the cargo targets and `/tmp/claude-1000`.
- **Fleet's blind spot.** No `df` anywhere: `PROBE_SCRIPT`,
  `HOST_IDENTITY_SCRIPT`, `HostRow`, `Health` carry no disk field
  (`grep -rn "df -\|disk_free\|statvfs"` over the four crates: nothing).
  Fleet's own writers on a host — transcripts under `~/.claude/projects`,
  `session-env`, `tmux load-buffer`, `move_session`'s carry tarball on the
  *target*, the safe-kill marker — all fail with ENOSPC before anything
  tells the operator why.

**Root cause.** Host health was scoped to reachability + versions; disk
was never a signal.

**Proposed fix — a host health signal.**
1. Add to `HOST_IDENTITY_SCRIPT`: `df -Pk "$HOME" "${TMPDIR:-/tmp}" | tail
   -n +2` → `disk_home_free_bytes`, `disk_home_pct`, `disk_tmp_free_bytes`
   on `hosts` (migration), written by the reconcile writer each pass.
2. `fleet_health.hosts_disk_low: [{alias, pct, free_bytes}]` under settings
   `health.disk_low_pct` (default 90) and `health.disk_min_free_bytes`
   (default 5 GiB); `list_hosts` carries the three fields; HostDetail shows a
   meter and a "what is using it" hint (`~/.claude/projects`, worktrees,
   `/tmp/claude-*`).
3. `move_session` preflight: refuse a target whose free space is below the
   carry size + 1 GiB (`E_TARGET_DISK`), and `new_session` on a host below
   `disk_min_free_bytes` returns a warning field.
4. A GC hook: the existing sweep (`service/gc`) could offer, never run, a
   "prune `/tmp/claude-<uid>` entries older than 7 d" suggestion per host.

**Effort.** M.

### F5 — `fleet-agent` 0.2.26 on claude-fleet-trn vs hub 0.3.1: tolerated, but silently missing features and no upgrade path but hand-work — P2

**Evidence.** `agent_status`: `agent_version 0.2.26`, connected since
1790454247. Proto window is `1..=1` on both builds, so the handshake is
accepted (`judge_proto`). Between v0.2.26 and v0.3.1 there are 23 commits
under `crates/fleet-agent` + `crates/fleet-proto`; the substantive ones are
**`AgentFrame::Report` / error-batch ring** (`6743f142`, `ac1c16a6`),
**"report error-level events to the hub on the heartbeat"** (`ab15f679`)
and **"batch report frames by bytes so the hub never refuses one"**
(`eadc2b24`). So the trn agent never sends error reports: the hub's error
channel (`reports.*` settings, *Error reports* in `docs/hub.md`) shows
nothing for that host, which reads as "healthy" rather than "old agent".
Upgrade is manual per `docs/hub.md` §*Install the agent*: download the
release tarball + `SHA256SUMS`, `install -m 0755`, then `systemctl restart
fleet-agent` (`KillMode=process` keeps the tmux servers and the 30 sessions
alive). There is no `self-update`, no `agent_behind` flag, and `fleet_health`
does not list agent versions.

**Root cause.** Version tolerance was designed (good), version *visibility*
was not: `agent_version` is stored in the registry and printed, never
compared.

**Proposed fix.** (a) `agent_status.hosts[].behind: true` when
`agent_version < hub version`, and `fleet_health.agents_behind: n`; a
HostDetail line "agent 0.2.26, hub 0.3.1 — upgrade: …" with the three
commands. (b) A "since your agent's version" list in the docs (changelog
section per release under *fleet-agent*). (c) Optional
`fleet-agent self-update --to vX` (download by tag, verify `SHA256SUMS`,
`install`, `systemctl restart`) — L; (a)+(b) are S. **Recommendation now:**
upgrade the trn agent to 0.3.1 by hand; there is no compatibility risk in
either order at proto 1.

**Effort.** S for the signal, L for self-update.

### F6 — tmux 3.3a (htz, oci, trn) vs 3.6a (mac, mefistos): nothing fleet uses differs — P3

**Evidence.** `tmux.rs` has no version gate; the stored `tmux_version` is
only displayed (`parse_tmux_version`, never compared). Features in use and
their floors: `new-session -d -e KEY=VAL` (≥ 3.2), `=NAME` exact session
targets, `respawn-pane -k`, `capture-pane -S -n -p`, `load-buffer -` (≥ 3.0,
the only floor the docs name), `set-clipboard on`; all present in 3.3a. The
one version-related hazard is handled: an in-place tmux upgrade (brew/apt)
while the old server runs yields `protocol version mismatch`, which
`parse_host_identity` treats as *unknown*, not *no server* (test at
`tmux.rs:2043`) — relevant to mefistos (up 57 d, brew tmux 3.6a) and htz
(up 144 d).

**Proposed fix.** None required. Optional S: turn the documented floor into
a check (`tmux_version < 3.0` → `probe_host` returns `tmux_ok: false` with
the reason), so the field earns its keep.

### F7 — The `local` → `mac` rename left a hidden legacy row, a split account link, and world-readable token-bearing backups on the Mac — P2 (P1 for the backup)

**Evidence.**
- Hub `list_hosts`: `local` is `hidden: true, reachable: false, provisioned:
  true`, `last_pinged_at` 1789929074 (Sep 20 18:52) and linked to account
  `796436ed…` — while `mac` is linked to `db76333a…` (the same account as
  mefistos and trn). The stale desktop DB copy has `local` *not* hidden and
  reachable; the hub hid it on first start per `docs/hub.md` ("that copied
  `local` row is hidden and marked unreachable automatically … not
  deleted"). Consequences already in the brief: 6 ghost `bg:*` rows under
  `local`, 3 of them duplicated under `mac`, `local` copies still
  `working`; `fleet_health.ghosts = 3` counts only visible hosts.
- On the Mac: **no** launchd items (`~/Library/LaunchAgents` has nothing
  fleet-related; `launchctl list` shows only the running app), **no** tunnel
  or `RemoteForward` entries in `~/.ssh/config` (the reverse tunnels were
  in-process `TunnelSupervisor` children; `fleet_health.tunnels = {}` now),
  **no** stale ControlMaster sockets (`~/.cache/claude-fleet/` holds only
  `cm-mefistos-tty.sock` and `transfer/`). `Host mac` in `~/.ssh/config`
  resolves to `mac.rlt.sk` as `martinjanci`, so the hub reaches the Mac over
  SSH to itself; `tmux ls` for uid 501 reports *no server running* although
  the row says tmux 3.6a — the 5 running `bg:*` sessions are headless, and
  any `mac` session that needs a pane would start the first server.
- Leftover files: `~/.claude/settings.json.bak` (Aug 11, **mode 0755**,
  world-readable) still contains the pre-Track-B **`http://127.0.0.1:4180/hook?token=…`**
  command hooks — a bearer token in a URL in a world-readable file (the
  `LEGACY_TOKEN_HOOK` shape `merge_hook_into_settings_json_with` strips from
  the live file, but the backup is not fleet's and was never cleaned).
  `~/.claude.json.fleet-bak` (420 KB, 0600) and
  `~/.claude/settings.json.fleet-bak` (0600) from Sep 20 carry the previous
  token generation. `~/.ssh/id_ed25519_claude_fleet_{htz,oci,oci_c,trn}` are
  the per-host keys and are fine.

**Root cause.** The rename was done by adding `mac` and letting the hub hide
`local`; nothing merges or retires the old row, and provisioning backups are
kept forever with no rotation.

**Proposed fix.** (1) Tell the user: `chmod 600 ~/.claude/settings.json.bak`
or delete it (its token is the desktop's embedded-API token; rotate that if
the app is ever put back in local mode). (2) A `retire_host` / GC rule for a
hidden legacy row: re-parent its `session_history` to the successor alias by
`claude_session_id`, drop duplicate ghost rows, then delete the row (ADR-sized
decision, since `local` is special-cased in `provision_tmux_clipboard`,
`OrgScope`, `operator.host`). (3) Provisioning keeps only the newest
`.fleet-bak` and removes it after the next successful provision. **Effort.**
S (advice + backup rotation), M (retire path).

### F8 — Skills on htz (11) vs 83–90 elsewhere is expected; ownership was never written down — P3

**Evidence.** Provisioning ships exactly two skills (`FLEET_SKILL`,
`FRIENDLY_NAME_SKILL`); everything else on mac/mefistos/oci is the user's
dotfiles set (mefistos: symlink; mac: real dir with a `synced` marker; oci:
a copy dated Aug 22). htz's 11 (`claude-creds`, the two fleet skills,
`connect-to-child`, `diagnose-why-work-stopped`, `paperclip*` ×4,
`para-memory-files`, `unlazy`) are a hand-picked subset from Aug 22 for the
Paperclip host. Hook sets differ the same way (graft on mefistos, `rtk` on
mefistos/htz, none on oci) — all user-owned.

**Ownership table (proposed for `docs/hub.md` → *Add and provision hosts*).**

| Path on host | Owner | Written by |
|---|---|---|
| `~/.claude/skills/claude-fleet-control/` | fleet | `provision_hosts` (overwrites) |
| `~/.claude/skills/fleet-friendly-name/` | fleet | `provision_hosts` (overwrites) |
| `~/.claude/CLAUDE.md` between the sentinels | fleet | `provision_claude_md` (rest is the user's) |
| `~/.claude/settings.json` → the 9 `FLEET_HOOK_EVENTS` entries | fleet | `provision_hook` (siblings kept) |
| `~/.claude/fleet-hook.headers` | fleet (secret, 0600) | `provision_hook` |
| `~/.claude.json` → `mcpServers.claude-fleet` | fleet (secret) | `provision_one` (siblings kept) |
| `~/.tmux.conf` `set-clipboard on` line | fleet (append-only) | `provision_tmux_clipboard` |
| every other skill, hook, plugin, `~/.claude/projects` | user / dotfiles | never touched |

**Effort.** S (docs).

### F9 — No way to see hook liveness or token epoch per host from fleet — P2

**Evidence.** The only proof that a host's hooks still authenticate is
indirect (`session_history` rows that can only come from a hook). `list_hosts`
has `provisioned: true` (a boolean set once) and no `provisioned_at`,
`last_hook_at`, or token `issued_at`; `list_host_tokens` is admin-only and
local-only in hub-client mode. A rotated token (`rotate_host_token`) with
a failed re-provision would show as a host that slowly goes `unknown`.

**Proposed fix.** `hosts.provisioned_at` and `hosts.last_hook_at` (stamped
by `mcp/hooks.rs` on every accepted hook), both in `list_hosts`;
`fleet_health.hosts_hooks_silent: [alias…]` for a reachable host with
running sessions and no hook in the last N minutes. **Effort.** S.

---

## Prioritised list

1. **F4 (P1)** — add disk to the host probe + `fleet_health`; and, outside
   fleet, act on htz (`~/.paperclip` 89 G of a 150 G overlay, 3.6 G free)
   and mefistos (41 G + 5 G fleet cargo targets, 18 G `.cargo-targets`, 9 G
   `/tmp/claude-1000`, 4 G journal; move targets to `/mnt/sda4`).
2. **F1 (P1)** — provision fingerprint + content-only re-provision on hub
   start; re-provision now to ship the M1b–M4 control skill.
3. **F2 (P1)** — declare ownership of the two skill dirs; untrack them in
   dotfiles; marker file + preflight warning.
4. **F7 (P1 part)** — `chmod 600`/delete `~/.claude/settings.json.bak` on
   the Mac (legacy `?token=` URL, world-readable); rotate `.fleet-bak`s.
5. **F3 (P2)** — refresh versions in the reconcile identity script; add the
   "Claude behind" nudge (htz at 2.1.214).
6. **F5 (P2)** — `agent_behind` signal; upgrade trn's agent to 0.3.1 by hand
   (gains error reporting; no proto risk).
7. **F9 (P2)** — `last_hook_at` / `provisioned_at` per host.
8. **F7 (P2 rest)** — retire the hidden `local` row and its duplicate ghosts.
9. **F8 (P3)** — ownership table in `docs/hub.md`.
10. **F6 (P3)** — nothing to do; optionally enforce the tmux ≥ 3.0 floor.

## Per-host table

| Host | Claude actual (hub cache) | tmux | Disk | Hooks | Skills (fleet's two) | Headers age | Notes |
|---|---|---|---|---|---|---|---|
| mac | 2.1.282 (2.1.235) | 3.6a per row; no server running for uid 501 | 90 %, 45 G free | OK — 9/9 http → fleet.rlt.sk | stale (60695fef / 15f5d6f0); tracked + ` M` in `~/dotfiles` | Sep 20 21:04, live | `settings.json.bak` 0755 with legacy `?token=` hooks; `.fleet-bak` ×2 from Sep 20; hidden `local` row is this machine |
| mefistos | 2.1.267 (2.1.234) | 3.6a, up 57 d | **98 %, 14 G free** | OK — 9/9 (+ user graft/rtk/unlazy) | stale; `~/.claude/skills` **is** the dotfiles checkout | Sep 20 21:05, live (turn_done today) | 41 G+5 G fleet cargo targets, 18 G `.cargo-targets`, 9.3 G `/tmp/claude-1000`, 4.1 G journal; docker unreadable; `/mnt/sda4` 298 G free |
| claude-fleet-htz | 2.1.214 (2.1.214) — 68 patch releases behind | 3.3a, up 144 d | **98 %, 3.6 G free** (container overlay) | OK — 9/9 (+ rtk/unlazy) | stale; 11 skills total (intended subset) | Sep 20 19:04 UTC, no sessions to prove it | `~/.paperclip` = 89 G; `/tmp` 5.9 G; symlinked `claude` never advanced |
| claude-fleet-oci | 2.1.282 (2.1.220) | 3.3a, up 86 d | 25 %, 72 G free | OK — 9/9 (+ unlazy) | stale; 83 skills copied Aug 22 | Sep 20 19:04 UTC | `settings.json` untouched since provisioning (fleet is its only writer) |
| claude-fleet-trn (agent) | 2.1.282 (was 2.1.277 until this review's `probe_host`) | 3.3a | not observable (no SSH; no disk field) | not observable; `provisioned: true` | not observable | not observable | `fleet-agent 0.2.26` vs hub 0.3.1; lacks the error-report frames; upgrade by hand |
| local (hidden) | — | — | — | — | — | — | legacy row, account `796436ed…` ≠ `mac`'s `db76333a…`; 6 ghost `bg:*` rows, 3 duplicated on `mac` |
