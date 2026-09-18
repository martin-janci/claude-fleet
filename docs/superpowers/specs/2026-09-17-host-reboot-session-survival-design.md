# Surviving a host reboot: never lose a session, offer to restore it

**Date:** 2026-09-17
**Status:** Design — not yet implemented. Findings F1–F5 re-verified
2026-09-18 against `origin/main` `1d6bedb`, after the `crates/fleet-core`
workspace split; paths and line numbers below are current as of that commit.
**Motivating incident:** turanga powered off 15:15:52, back 15:24:13 on
2026-09-17. `claude-fleet-trn` is a nerdctl container (`claude-fleet-host`,
namespace `om-ml`) on that box, so its tmux server died with the host. At
least 8 live work sessions were running. After the reboot the `sessions`
table held **zero** rows for them — not ghosts, not `lost_at`, gone.
Recovery required reading Claude Code's own transcripts on the host
(`~/.claude/projects/<slug>/<uuid>.jsonl`) and resuming each by hand.

## Findings: why the rows vanished

These were verified against the code before any design work, because the
premise ("fleet has a ghost concept that did not engage") turned out to be
only half right.

### F1 — The ghost concept engaged. It expired after one cycle.

`Store::ghost_and_clean` (`crates/fleet-core/src/store/reconcile.rs:255`) is a
two-phase ghost-then-reap:

- **Phase 1** sets `status='ghost', lost_at=now` on every row of the host not
  in this pass's `keep` set.
- **Phase 2** *hard-deletes* rows that were already `ghost` **before this
  pass**, along with their `session_events` and `session_messages` (neither
  table has an FK cascade).

So the grace period is exactly **one reconcile cycle** —
`DEFAULT_RECONCILE_INTERVAL_SECS = 20`. turanga was down ~8 minutes, roughly
25 ticks. `list_sessions { include_lost: true }` would have shown those 8
sessions for about twenty seconds. The soft-delete is designed for a
transient probe miss, not for an outage.

### F2 — "tmux server is gone" is indistinguishable from "zero sessions".

`list_local_sessions` (`crates/fleet-core/src/tmux.rs:484`) maps a `no server
running` stderr to `Ok(vec![])` via `is_no_server_running`
(`crates/fleet-core/src/tmux.rs:524`). An empty `Ok` means:

- the host counts as **reachable**, so `reconcile_write_one_host` takes the
  `Ok(live)` branch,
- `keep` is empty,
- therefore every tmux-kind row on the host is ghosted, then reaped.

The same holds on the **remote** path, which is the one the turanga
incident actually took: `RemoteTmux::list_sessions`
(`crates/fleet-core/src/tmux.rs:287`) also returns `Ok(Vec::new())` when
`is_no_server_running` matches the ssh output.

The genuinely-unreachable path is already safe: the `Err` branch
(`crates/fleet-core/src/service/sessions/reconcile.rs:570`) does no upserts and runs
no prune, keeping last-known rows. The all-gone case is the dangerous one,
and it is precisely the case the reboot produces.

Worth recording: `parse_sessions_checked` (`crates/fleet-core/src/tmux.rs:563`)
*already* refuses garbled `list-sessions` output with `E_TMUX`, and its
comment gives exactly this reasoning — treating it as zero sessions "would
ghost, then delete, every row on the host". The legitimate no-server case
walks straight past that guard.

### F3 — `recreate_session` already resumes correctly.

`recreate_pane_command` (`crates/fleet-core/src/service/sessions/lifecycle.rs:1069`)
→ `tmux::pane_command_for` (`crates/fleet-core/src/tmux.rs:590`) emits:

```
cl --resume '<id>' 2>/dev/null || cl --session-id '<id>' || cl; exec ${SHELL:-/bin/zsh} -l
```

It is a real `--resume`, with a create-under-that-id fallback. **R3 is
therefore a batching problem, not new machinery.**

### F4 — The spec's "detached + send-keys" constraint is already satisfied.

`tmux::new_session` passes the pane command as `tmux new-session -d`'s
command argument rather than sending keys. The reason the pane survives the
CLI exiting is the trailing `exec ${SHELL:-/bin/zsh} -l`, not the invocation
style. **We keep the current mechanism**: same guarantee, one fewer round
trip, and it is the path every other create already uses. Switching to
`send-keys` would fork the create path for no gain.

### F5 — A tmux name is not always derivable from a cwd.

`fill_session_name` (`crates/fleet-core/src/service/sessions/lifecycle.rs:356`) uses
the deterministic `dev-<owner>-<repo>--<worktree>` **only when it is free**.
A *second* session on the same worktree gets a generated
`dev-<owner>-<repo>--<adjective>-<noun>` instead. This bounds R4: see
[Known limitation](#known-limitation-derived-names-r4-only).

## Approach

Three were considered.

| | Approach | Verdict |
|---|---|---|
| **A** | **Probe-level verdict.** The probe learns the host's boot identity and whether a tmux server exists; reconcile takes a distinct mass-loss branch; *why* a row was lost becomes a stored value. | **Chosen** |
| B | Heuristic in the writer: `keep` empty + host had ≥N rows last pass ⇒ mass loss. No new round trips, no host schema. | Rejected — cannot tell a reboot from a user killing their last session, and silently fails on a one-session host. |
| C | TTL only: lengthen Phase 2's grace, no signal at all. | Rejected — smallest diff, but R2 is unmet, the two cases stay conflated, and ordinary kills linger for days. |

A is the only one that makes "why did this row go away" answerable.

## R1 + R2 — The safety net

### Probing host identity

`TmuxExec` has 7 implementors, 6 of them test fakes. Rather than change
`list_sessions`' return type and churn all of them, add a **defaulted**
trait method:

```rust
/// Host boot identity + tmux server pid, read in ONE script per probe.
/// `None` means the read did not happen or failed — NOT that the host has
/// no tmux server. The default impl returns `None`, so existing fakes
/// compile untouched and never trigger the mass-loss branch.
async fn host_identity(&self) -> Option<HostIdentity> { None }
```

```rust
pub struct HostIdentity {
    /// `/proc/sys/kernel/random/boot_id`, else `sysctl -n kern.boottime`,
    /// else `uptime -s`. `None` when every fallback produced nothing but
    /// the script itself ran.
    pub boot_id: Option<String>,
    /// `tmux display-message -p '#{pid}'`. `None` ⇒ no tmux server is
    /// running on the host.
    pub tmux_server_pid: Option<i64>,
}
```

The `Option<HostIdentity>` / `Option<tmux_server_pid>` distinction is
load-bearing and easy to collapse by accident. **A failed script yields
`None` at the outer level and produces no verdict**; only a script that ran
and found no server yields `Some(HostIdentity { tmux_server_pid: None, .. })`,
which is the signal. Flattening these two into one `None` would mass-mark
every host whose identity read failed — the exact opposite of the intent.

One script:

```sh
printf 'boot=%s\n' "$(cat /proc/sys/kernel/random/boot_id 2>/dev/null \
  || sysctl -n kern.boottime 2>/dev/null || uptime -s 2>/dev/null)"
printf 'tmuxpid=%s\n' "$(tmux display-message -p '#{pid}' 2>/dev/null)"
```

Inside a container, `/proc/sys/kernel/random/boot_id` reflects the **host**
kernel. That is the property that made turanga's reboot observable from
inside trn, and it is why this works for the container hosts.

Carrying the tmux **server pid** rather than a `server_present` boolean buys
a second signal for free: a pid that changed between passes means the tmux
server was restarted (`tmux kill-server`, a systemd restart) even though the
machine never rebooted. Same consequence for sessions, same handling.

Called in `probe_with_timeout` immediately after `list_sessions`, in the
same place and with the same best-effort semantics as the existing
`read_oauth_account` call. Stored on `HostProbe` as
`identity: HostIdentity`.

### Schema — migration `034_host_boot_identity.sql`

```sql
ALTER TABLE hosts    ADD COLUMN boot_id         TEXT;
ALTER TABLE hosts    ADD COLUMN tmux_server_pid INTEGER;
ALTER TABLE sessions ADD COLUMN lost_reason     TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (34);
```

**Numbering:** `032` was claimed by upstream's client-tokens work
(`032_client_tokens.sql`) and `033` by the asset-layers work
(`033_asset_layers.sql`, `2026-09-17-asset-layers-and-profiles-design.md`),
both of which landed first — this spec renumbers to `034` accordingly.

Register it in the `MIGRATIONS` table in `crates/fleet-core/src/store/schema.rs`.

`sessions.lost_reason` is `NULL` for a live row and otherwise one of:

| value | meaning |
|---|---|
| `host_reboot` | `boot_id` differs from the stored one |
| `tmux_server_gone` | no tmux server, or `tmux_server_pid` changed |
| `missing` | this session alone vanished while its neighbours stayed live |

The upsert in `upsert_session_in_tx` clears `lost_reason` alongside
`lost_at` when a row comes back.

### The mass-loss branch

In `reconcile_write_one_host`, **before** the normal upsert + prune, compute
a verdict from the probe's `HostIdentity` against the stored `hosts` row:

```
identity is None (script failed / fake default)          -> None (normal pass)
host has no live rows to mark                            -> None (normal pass)
boot_id known AND stored boot_id known AND differ        -> host_reboot
tmux_server_pid is None (script ran, found no server)    -> tmux_server_gone
tmux_server_pid known AND stored known AND differ        -> tmux_server_gone
otherwise                                                -> None (normal pass)
```

Two guards make this fail safe, and both are required:

- **An unreadable identity never mass-marks a host.** Each comparison needs
  *both* sides known, and a failed script is `None` at the outer level, so a
  first probe or a host missing `tmux`/`/proc` behaves exactly as today.
- **A host with no live rows takes the normal path.** Otherwise a host that
  legitimately has no sessions and therefore no tmux server would report
  `tmux_server_gone` on *every* pass and skip `ghost_and_clean` forever,
  quietly disabling the prune there. With the guard, the branch only ever
  runs when there is something to lose.

On a verdict, the host takes a separate branch for that pass:

1. One statement sets `status='ghost'`, `lost_at=now`, `lost_reason=<verdict>`
   on **every live row of that host** — tmux-kind *and* pane-less
   (`bg`/`external`); a reboot kills those too. Preserved untouched:
   `tmux_name`, `project_id`, `worktree_id`, `worktree_key`,
   `claude_session_id`, `friendly_name`, `last_prompt`, `turn_seq`.
2. One `RowChange::SessionUpdated` per row, emitted after commit as usual.
3. One `lost` row per session into `session_events`, detail = the verdict.
4. A host-level `RowChange::HostSessionsLost { host_alias, reason, count }`
   — a struct variant following the existing `AssetInventoryCleared`
   precedent in `crates/fleet-core/src/events.rs` — so the UI surfaces the reboot as
   one thing rather than N independent disappearances.
5. `ghost_and_clean` is **skipped entirely** for that pass — the whole set is
   lost, there is nothing to prune. This is R2's "in one pass, rather than
   letting per-session reconcile race through them."
6. `hosts.boot_id` / `hosts.tmux_server_pid` are updated to the newly
   observed values, so the verdict fires once, not on every subsequent pass.

A normal pass (no verdict) still updates the stored identity and still runs
`ghost_and_clean`, whose Phase 1 now also stamps `lost_reason='missing'`.

### The reap exemption

`ghost_and_clean` Phase 2's pre-ghost SELECT gains an exclusion:

```sql
AND NOT (
  claude_session_id IS NOT NULL
  AND lost_reason IN ('host_reboot','tmux_server_gone')
  AND lost_at >= :now - :ttl
)
```

Consequences, deliberately:

- A single session vanishing while its neighbours live is `missing`, is not
  exempt, and keeps today's one-cycle reap. **A deliberate kill does not
  become a permanent ghost.** This is R1's "distinguish this from a genuine
  one-off kill".
- A mass-loss row with a resumable conversation survives reconcile
  indefinitely, until the user restores it or dismisses it.
- The TTL cap stops an abandoned host from growing ghosts without bound.

New setting `sessions.lost_ttl_secs`, registered in
`crates/fleet-core/src/service/settings.rs` `SPECS`, default `1209600` (14 days).
The store function takes the resolved TTL as a parameter; it does not read
settings itself.

`ghost_and_clean` is shared with `ghost_and_clean_bg_sessions` for pane-less
rows. The exemption applies there too — same rule, same reason. Restore, in
this iteration, offers only tmux-kind rows; a background agent's resume is
a different mechanism and is out of scope.

## R3 — `restore_host_sessions`

```
restore_host_sessions { host_alias, dry_run?: bool, session_ids?: [i64] }
```

Service entry point in `crates/fleet-core/src/service/sessions/` alongside the
existing lifecycle calls, with a Tauri command and an MCP tool wrapping it.

**Selection.** Lost rows (`lost_at IS NOT NULL`) on `host_alias`, of tmux
kind, carrying a non-NULL `claude_session_id`, narrowed to `session_ids`
when given. An id in `session_ids` that is not restorable is reported as a
skip with a reason, never a batch failure.

**`dry_run: true`** returns the plan and performs **zero** ssh calls and
**zero** writes — per entry: `session_id`, `tmux_name`, resolved `cwd`,
`claude_session_id`, `friendly_name`, and `action: "restore" | "skip"` with
a reason for a skip. This is directly testable and is what the UI confirm
dialog is seeded from.

**Execution.** Each entry drives the existing `recreate_session` path
(F3: it already resumes). Two new settings bound the blast radius of
starting N Claude CLIs on one box:

| setting | default | meaning |
|---|---|---|
| `restore.batch_size` | `4` | concurrent restores per call |
| `restore.stagger_ms` | `3000` | delay between successive launches |

Each session's `Result` is collected independently. A worktree that has gone
missing fails that one entry — `recreate_session` already returns
`E_REPAIR_REQUIRED` for anything its create-only self-repair cannot fix — and
the rest proceed. The call returns per-session success/failure; it never
fails the batch because one entry failed.

**Events.** `session_restored` / `session_restore_failed` into
`session_events` (detail = the error message on failure). Extends the
documented `kind` vocabulary in `013_session_events.sql`'s comment alongside
`lost`.

**First-run prompts are not answered.** A resumed session may stop on e.g.
"New MCP server found in this project: graft". The existing `pane_intel`
analyzer already classifies that as `trust_prompt`, so the session surfaces
as blocked with a `stuck_kind` on the next reconcile and the user decides.
This requires no new code — only the discipline of not adding an
auto-answer.

**Authority.** The service holds the `SshClient`, so the Tauri/UI path needs
no token at all. The MCP tool wraps the same service behind the existing
`require_host(caller, host_alias, …)` helper
(`crates/fleet-core/src/mcp/tools/support.rs`): the master token may restore any
host, a per-host token only its own. No operator agent ever needs to hold
the target host's token, which is the constraint the incident surfaced.

`restore_host_sessions` is a mutating tool: **not** added to
`READONLY_TOOLS` in `crates/fleet-core/src/mcp/guard.rs`.

**UI.** `HostDetail.svelte` gains a "Restore N lost sessions…" action, shown
only when the host has lost rows with resumable ids. It calls the command
with `dry_run: true`, renders the plan in a `ConfirmDialog`, and on confirm
re-calls without `dry_run`. Results patch the store through the usual row
events.

## R4 — Discovery when the rows are already gone

```
discover_lost_sessions { host_alias, limit?: i64 }   // read-only
```

For the case fleet is in *today*: no rows left at all. Read-only — it
mutates nothing and creates nothing.

**One host script**, following the established
`tmux::transcript_mtimes_script` pattern and run through the existing
`HostShell`: walk `~/.claude/projects/*/*.jsonl`, **skipping any path
containing `/subagents/`** (those belong to a parent session), and emit per
file its mtime, its path, and a bounded `tail -c` chunk. The JSON is parsed
in **Rust**, not on the host — no `jq` dependency, and transcript size is
irrelevant (`tail -c` on a 115 MB file is cheap). **No size cutoff**: the
incident confirmed a 115 MB transcript resumes fine.

**Ranking in Rust.** Read `cwd` and `gitBranch` from the last well-formed
lines of each chunk. Group by `cwd`, keep the newest transcript per `cwd`,
and rank by mtime **relative to the host's boot time** — which R2 now
records, so a transcript last written minutes before the boot ranks above
one from last week. Candidates that already have a row (live or lost) are
marked as such rather than dropped.

Each candidate carries: `cwd`, `git_branch`, `claude_session_id`,
`transcript_mtime`, `derived_tmux_name`, `existing_session_id`, and a
`rank_hint` describing its position relative to boot.

**Restoring a candidate is a separate, explicit call.** A stale transcript
is not proof a session was live, so discovery ranks and the user chooses —
exactly as the source spec asks.

### Known limitation: derived names (R4 only)

Per **F5**, a candidate's tmux name is *derived* from its `cwd` via the
projects/worktrees tables as `dev-<owner>-<repo>--<worktree>`. When the
original session was a *second* session on that worktree it had a generated
`dev-<owner>-<repo>--<adjective>-<noun>` name, which no transcript records.
For that case the restored session's name will differ from the original.

**The conversation is fully preserved; the label may not be.** The spec's
"reproduce the exact original name" holds on the R3 path — the surviving row
remembers `tmux_name` — but cannot be guaranteed on the R4 path. Discovery
therefore shows `derived_tmux_name` in the candidate list so the user sees
the name before confirming.

## R5 — Lifecycle logging

`tracing::info!` with `host_alias`, `tmux_name` and `claude_session_id` on
every lifecycle transition:

| transition | site |
|---|---|
| created | `apply_host_reconcile`'s post-commit flush (`RowChange::SessionCreated` carries the whole row) |
| lost | the mass-loss branch and `ghost_and_clean` Phase 1, with `lost_reason` |
| restored | the restore service, success and failure |
| deleted | `ghost_and_clean` Phase 2, with the reason it was not exempt |

Logging at the flush point rather than inside the SQL helpers keeps it in
one place and gives it the full `SessionRow`. The incident's log recorded
only MCP tool calls and tunnel warnings; after this, losing 8 sessions
leaves 8 INFO lines naming each one and why.

## Testing

**Store-level** (`crates/fleet-core/src/store/`, no host required):

- A host that stays reachable while its tmux server vanishes keeps its
  session rows, with `lost_at` set, `lost_reason='tmux_server_gone'`, and
  `claude_session_id` intact. *(acceptance criterion 1)*
- A changed `boot_id` marks every row of the host in one pass with
  `lost_reason='host_reboot'` and runs no prune.
- Phase 2 exempts a recoverable mass-loss row, still reaps a `missing` row
  on its usual one-cycle schedule, and does reap a mass-loss row older than
  `sessions.lost_ttl_secs`.
- A row with `lost_reason` set but `claude_session_id` NULL is not exempt.
- A first probe against a host with no stored identity produces no verdict.
- A host whose identity script FAILED (`None`) produces no verdict, while a
  host whose script ran and found no server does — the distinction the
  `Option<HostIdentity>` wrapper exists to protect.
- A host with a missing tmux server but zero live rows takes the normal
  path, so `ghost_and_clean` still runs there.

**Service-level** with a fake `TmuxExec` implementing `host_identity` and a
fake `HostShell`:

- `restore_host_sessions { dry_run: true }` plans exactly one restore per
  lost session and mutates nothing — asserted by comparing the full row set
  before and after, and by the fake recording zero tmux calls.
  *(acceptance criterion 2)*
- One entry whose worktree is missing fails alone; the others restore.
- `list_sessions { include_lost: true }` returns the lost sessions between
  the loss and the restore. *(acceptance criterion 4)*
- `discover_lost_sessions` skips `subagents/` paths, collapses duplicate
  `cwd`s to the newest transcript, and ranks against boot time.

**End-to-end, on a real host** — a manual acceptance step run before the PR,
on `local` or `claude-fleet-trn`: `tmux kill-server`, confirm the rows go
lost and not deleted, run the restore, confirm each session returns in its
original worktree with its conversation resumed.
*(acceptance criterion 3)*

**Docs.** `restore_host_sessions` and `discover_lost_sessions` are new MCP
tools *and* new Tauri commands, so `docs/control-api-reference.md` must be
regenerated or CI fails:

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

## Out of scope

- **Automatic restore after a reboot.** Silently starting several Claude
  CLIs on a host that just came back is not wanted. The lost sessions are
  surfaced; the user triggers the restore. A per-host opt-in setting is the
  most this should ever become, and is not built here.
- Restoring pane-less `bg` / `external` agent rows. They are marked lost and
  preserved, but resuming a background agent is a different mechanism.
- Any change to the `send-keys` vs pane-command create path (see **F4**).

## Delivery

Two PRs. The split is not cosmetic: the first stops the bleeding and is
independently valuable, and the second is easier to build once the first has
been recording boot times for a while.

1. **Safety net** — findings, migration 034, `host_identity`, the mass-loss
   branch, the reap exemption, R5 logging. After this, no session is lost to
   a reboot again.
2. **Recovery** — `restore_host_sessions`, `discover_lost_sessions`, the
   `HostDetail` UI, docs regeneration.
