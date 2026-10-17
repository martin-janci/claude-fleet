# Autofix: detect broken states, explain them, offer a fix

Date: 2026-10-17 · Status: design approved in chat, spec under review

## Changes after planning (2026-10-17)

The core plan (PR 1) departs from the text below in these ways:

- States are `open | verifying | cleared | apply_failed | dismissed` (no
  `applying`). A `verifying` row still reported after `AUTOFIX_VERIFY_SECS`
  (600 s) becomes `apply_failed`: "the fix ran, but the problem is still
  there". Automatic retries are spaced by `AUTOFIX_RETRY_SECS` (600 s), up to
  `autofix.max_attempts` (default 3, range 0..10).
- No `autofix_audit` table: every apply goes through
  `Store::record_admin_event_or_log` with `action: "autofix.apply"`.
- Settings in PR 1: `autofix.auto_safe` (true), `autofix.max_attempts` (3),
  `autofix.detector.operator_unbound` (true), `autofix.detector.host` (true).
  A switched-off detector's findings clear on the next pass.
  `autofix.interval_secs` and `autofix.mcp_entry_probe_secs` do not exist:
  the loop period is a fixed 60 s, and the probe (detector 2) belongs to PR 2.
- Detector 4: `host.provision_stale` is Safe with `auto: false` (the
  reprovision loop owns it); `host.hooks_silent` is Safe with `auto: true`;
  `host.agent_behind` is NeedsApproval and runs `update::update_now` with
  target `agent:<alias>`, then pokes the agent's updater (a poke counts only
  when it prints `poked` and exits 0); `host.claude_behind` is
  explanation-only (fleet has no Claude Code update path). Unreachable hosts
  are skipped; hidden hosts never appear (`health::hosts_health` skips them).
- MCP tool `autofix` is `Access::PersonDevice`, so it is not served to
  per-host tokens or the master. `list` returns live rows only; `scan` syncs
  and never auto-applies; `apply` / `dismiss` need a trusted full device and
  the operator (`ux-agent`) gets `E_FORBIDDEN`.
- `scan` returns all live rows after syncing, not only the new ones.
- Each automatic apply re-checks eligibility (pause, dismissal, budget) under
  the same guard that marks the attempt; a manual apply on a `verifying` row
  is refused for `AUTOFIX_INFLIGHT_SECS` (60 s) after the attempt.
- Detector 1 vs PR #932: #932 binds at birth and backfills; detector 1
  catches later regressions. Autofix adds no backfill.
- Migration 195 collides with PR #932's 195; whichever merges second
  renumbers with `scripts/renumber-migrations.sh`.
- Detectors 2 and 3 and the surfaces (chat card, panel, feed) are PR 2.

## Problem

On 2026-10-17 the fleet operator (Control, tmux `fleet-operator` on mefistos)
could see no sessions at all, and `whoami` returned `E_NOTFOUND`. It took a
person reading `view_scope.rs`, `auth.rs` and `operator.rs` to find the
cause: `ensure_operator_on` mints the `ux-agent` client token with no
`person_id`, and a client with no person gets a refusing `ViewScope`. Nothing
in the fleet noticed, and nothing could have explained it to the person
looking at an agent that "sees nothing".

There are more states like this. Each one is detectable from data the hub
already has (or can cheaply probe), and each has a known fix. Today they
surface in five different shapes, or not at all:

| Shape today | Where | Fix attached? |
|---|---|---|
| `stuck_kind` + playbooks | `pane_intel.rs`, `playbooks.rs` | automatic for `press_enter`/`oom` (toggles off by default); `Notify` only for the rest |
| `attention::Reason` | `attention.rs` | no |
| `HostHealthRow` booleans | `health.rs` | no |
| `OperatorStatus.blocked` + `blockedCopy` | `operator.rs`, `src/lib/operator.ts` | yes, one button, operator only |
| host checklist | `host_check.rs`, `src/lib/host_check.ts` | a reprovision hint |

The host's `~/.claude.json` MCP entry is not checked by anything
(`provision.rs` doc :123–140 documents the gap; a test pins it).

## Goal

One subsystem, **autofix**, that:

1. detects known broken states with small, pure detectors;
2. explains each in plain language: what is wrong, why it matters, what the
   fix changes, whether it can be undone, and the evidence;
3. applies **safe** fixes by itself and says so; waits for the person's
   approval on everything else;
4. re-runs the detector after a fix and reports whether the state cleared;
5. shows the same findings in four places: an MCP tool, a `fix` card in
   Control's chat, a panel section, and the operator's event digest.

## Non-goals

- Replacing playbooks or workspace repair. Autofix lists their states and
  delegates to them; their executors and settings stay.
- Free-form "the agent decides what to fix". Every fix is code in a detector,
  with a fixed set of changes.
- Translating the UI. Finding copy is English like the rest of the app;
  Control explains it in the person's language.

## Names

`repair` (workspace repair), `playbook` (stuck playbooks), `doctor`
(`ag doctor`, Claude's `/doctor`), `proposal` (setting proposals),
`escalation` and `confirm` are taken. This subsystem is **autofix**:
module `service/autofix/`, store `store/autofix.rs`, MCP tool `autofix`,
chat block kind `fix`, settings `autofix.*`, loop `autofix`.

## Architecture

```
reconcile tick ──► autofix runner ──► detectors (pure) ──► findings
                         │                                    │
                         │  Safe + auto_safe on               ▼
                         ├──────────────► apply ──► re-scan ──► cleared / apply_failed
                         │
                         └─ NeedsApproval ──► open finding ──► MCP / fix card / panel / feed
                                                                   │
                                          person: Fix ─► apply ─► re-scan
```

### Detector

```rust
pub trait Detector: Send + Sync {
    fn id(&self) -> &'static str;               // "operator_unbound"
    fn scan(&self, cx: &ScanCx<'_>) -> Result<Vec<Finding>, IpcError>;
    async fn apply(&self, cx: &ApplyCx<'_>, f: &Finding) -> Result<(), IpcError>;
}
```

- `scan` is pure over `ScanCx` (store snapshot, host rows, health rows,
  probe results). No I/O in `scan`; probes that need a host are gathered
  before by the runner (see detector 2), the `playbooks::plan` split.
- `apply` does the change. The runner, not the detector, re-scans afterwards.

```rust
pub struct Finding {
    pub kind: &'static str,          // detector id + variant, e.g. "host_mcp_entry.header"
    pub subject: Subject,            // Host(alias) | Session(id) | Token(name) | Hub
    pub severity: Severity,          // Blocking | Degraded | Info
    pub title: String,               // one line
    pub explanation: String,         // why it matters, plain language
    pub evidence: Vec<String>,       // facts, never secrets ("sha256 prefix a1b2… ≠ host token")
    pub fix: Option<Fix>,
}
pub struct Fix {
    pub summary: String,             // "Bind the ux-agent token to <owner>"
    pub changes: Vec<String>,        // what is written, where
    pub safety: Safety,              // Safe | NeedsApproval
    pub reversible: bool,
    pub action: Option<ClientAction>,// for no-code fixes: login | open_host | open_settings
}
```

### Store

Migration `195_autofix.sql` (renumber with `scripts/renumber-migrations.sh`
if main takes it first):

```sql
CREATE TABLE autofix_findings (
  id            INTEGER PRIMARY KEY,
  kind          TEXT NOT NULL,
  subject       TEXT NOT NULL,          -- "host:mefistos", "session:23149", "token:ux-agent", "hub"
  host_alias    TEXT,                   -- for scoping a per-host caller
  severity      TEXT NOT NULL,
  body          TEXT NOT NULL,          -- JSON: title, explanation, evidence, fix
  state         TEXT NOT NULL,          -- open | verifying | cleared | apply_failed | dismissed
  first_seen_at INTEGER NOT NULL,
  last_seen_at  INTEGER NOT NULL,
  dismissed_until INTEGER,
  attempts      INTEGER NOT NULL DEFAULT 0,
  last_attempt_at INTEGER,
  last_error    TEXT
);
CREATE UNIQUE INDEX autofix_live ON autofix_findings(kind, subject)
  WHERE state IN ('open','verifying','apply_failed','dismissed');
```

Applies are audited through `record_admin_event` (`action: "autofix.apply"`); there is no audit table.

A finding is an **episode**: the same `(kind, subject)` seen again updates
`last_seen_at`; when a scan stops reporting it, the row moves to `cleared`.
A later recurrence opens a new row.

### Runner

- In the reconcile tick, behind `loops::gate("autofix")`, every 60 s (fixed), stops when automation is paused.
- Upserts findings, clears the ones no detector reported.
- Auto-applies a finding only when **all** hold: `fix.safety == Safe`,
  `autofix.auto_safe` is on (default **on**), the detector's own switch
  `autofix.detector.<id>` is on (default on), and backoff allows it: at most
  one attempt per episode per 10 min, `autofix.max_attempts` (default 3) per
  episode. After that it stays `apply_failed` and waits for the person.
- After every apply, re-scans that detector only. Reported → `apply_failed`
  with the detector's evidence. Not reported → `cleared`.
- Every apply is recorded with `record_admin_event` (`action: "autofix.apply"`).

### Access

- Not served to per-host tokens in v1 (`Access::PersonDevice`).
- `list` / `explain`: what the caller may see. A person sees findings on hosts and
  sessions their `ViewScope` reaches, plus `hub` findings when fleet-admin.
- `apply` / `dismiss`: **fleet-admin or the personal owner only**, the gate
  `decide_setting_proposals` uses. The operator's token and host tokens get
  `E_FORBIDDEN`. They may post a `fix` card instead.

## Detectors (v1)

| # | id / kind | Detects | Fix | Safety |
|---|---|---|---|---|
| 1 | `operator_unbound` | the live `ux-agent` client token has `person_id IS NULL`, so the operator sees no sessions and `whoami` fails | `set_client_person("ux-agent", personal_owner)` | NeedsApproval (auth) |
| 2a | `host_mcp_entry.header` | the host's `~/.claude.json` `mcpServers["claude-fleet"]` lacks `X-Fleet-Pane`, or its `url` ≠ the hub's `mcp_url()` for that host | rewrite the entry via `merge_mcp_entry` with the host's stored token (`get_host_token`) | Safe |
| 2b | `host_mcp_entry.token` | the entry's bearer hash ≠ the host token's hash; evidence names whose it is: another host, `ux-agent`, a revoked token, or unknown | same rewrite, which replaces the bearer with this host's own token | NeedsApproval (auth) |
| 2c | `host_mcp_entry.missing` | no `claude-fleet` entry at all | same rewrite | Safe |
| 3a | `stuck.reconnect` | `stuck_kind = reconnect` | send the reconnect keys | Safe |
| 3b | `stuck.trust_prompt` | `stuck_kind = trust_prompt` | accept the trust prompt | NeedsApproval (security) |
| 3c | `stuck.auth_menu` | `stuck_kind = auth_menu` | none: `ClientAction::login` | n/a |
| 3d | `stuck.press_enter`, `stuck.oom` | existing playbook states | delegates to the playbook executor | Safe only when that playbook's toggle is on, else NeedsApproval |
| 4a | `host.provision_stale`, `host.hooks_silent` | `HostRow.provision_stale`, `HostHealthRow.hooks_silent` | `provision_content_only` (fleet's own files only) | Safe (`provision_stale` is `auto: false`, owned by the reprovision loop; `hooks_silent` is `auto: true`) |
| 4b | `host.agent_behind`, `host.claude_behind` | `HostHealthRow` flags | `agent_behind`: `update_now` on `agent:<alias>` then poke; `claude_behind`: explanation only | NeedsApproval (restarts the agent) / n/a |

### Detector 2: the probe

The runner asks each reachable host, at most every
`autofix.mcp_entry_probe_secs` (default 600), for a summary of its global
entry, computed **on the host** so no token crosses the wire:

```sh
f=~/.claude.json
jq -c '.mcpServers["claude-fleet"] // null
       | if . == null then null
         else {url, has_pane: (.headers["X-Fleet-Pane"] != null)} end' "$f"
jq -j '.mcpServers["claude-fleet"].headers.Authorization // ""
       | sub("^Bearer ";"")' "$f" | sha256sum | cut -c1-64
```

The second line pipes the bearer straight into `sha256sum` on the host. The
raw value is never printed, logged or returned. Only the hash leaves the host.
The exact command is built with `shq` and lives next to `merge_mcp_entry`.
The hub compares the hash with `sha256(host token)`, `sha256` of live client
tokens and `operator.token_sha`, and with revoked rows, to fill the evidence.

The operator's own `.mcp.json` (project scope) takes precedence over the
global entry, so rewriting the global entry never changes the operator's
identity.

### Detector 1 and the direct fix

PR #932 binds the token at birth and backfills the live one; detector 1
catches later regressions (a manual `set_device_person`, a restore from a
backup, a hub whose owner was set after the birth). Autofix adds no backfill
of its own.

## Surfaces

### MCP tool `autofix`

`action`: `list` (open findings, filters `host_alias`, `state`), `explain
{id}`, `apply {id}`, `dismiss {id, until?}`, `scan` (run detectors now; returns
the new findings). The description goes into `docs/control-api-reference.md`
(`REGEN_DOCS=1 cargo fleet-test -- reference_is_current`).

### `fix` chat card (`fleet.ui/1`)

`{ "type": "fix", "finding_id": 42 }`. The card renders the finding from the
hub by id, never from the block, so an agent cannot invent a fix or reword
what it changes. Buttons: **Fix** (confirm dialog listing `changes`; desktop
or trusted device only, like the `setting` card), **Later** (dismiss 24 h),
**Details**. A cleared or missing finding renders as a quiet "already
resolved" line. Schema regenerated with `REGEN_FORM_DOCS`.

### Panel

- A "Problems & fixes" section in the Needs-you view (`ControlViewsPanel`),
  one row per open finding, rendered by `FixCard.svelte` (shared with the chat
  card). Safe fixes applied in the last 24 h show as a collapsed "fixed
  automatically" list.
- `HostHealthChip` shows a count of open findings for that host.
- `AgentPanel` keeps `blockedCopy`; when detector 1 has an open finding, the
  panel shows it under the operator status.

### Operator feed

`operator_feed` adds lines to its `[fleet events]` digest: `problem: <title>
(<subject>) — fix needs your approval` and `fixed automatically: <title>`.

## Testing

- **Detectors**: one red test per variant on a store in the broken state, and
  a green test on the healthy state. Detector 1's red test is the
  2026-10-17 case: a live `ux-agent` row with `person_id NULL`.
- **Fixes**: apply → re-scan → `cleared`; a fix whose re-scan still reports
  the state → `apply_failed` with evidence.
- **Runner**: Safe applies only with `auto_safe` and the detector switch on;
  NeedsApproval never auto-applies; paused automation applies nothing;
  backoff and `max_attempts` hold; a vanished finding becomes `cleared`;
  recurrence opens a new episode.
- **Access**: operator token and host tokens get `E_FORBIDDEN` on `apply` and
  `dismiss`; a host token's `list` never shows another host.
- **Probe**: the probe command never prints a token (assert on the command
  string and on a fixture `~/.claude.json`).
- **Frontend**: vitest for `FixCard`, the findings store, and the `fix` block
  renderer's resolved / missing states.

## Delivery

Two PRs from one spec:

1. **Core**: migration, store, detector trait and registry, runner and tick
   wiring, settings, MCP tool, detectors 1 and 4, audit and events, docs
   (`docs/architecture.md` section, `docs/control-api-reference.md`,
   settings reference).
2. **Surfaces and host detectors**: `fix` chat card, panel section and chip,
   operator feed lines, detector 2 (probe) and detector 3 (stuck sessions).

## Open risks

- **Detector 2 on a host whose `~/.claude.json` does not parse**: the probe
  reports `parse_error`; the finding has no fix (we never overwrite a file we
  cannot read) and says so.
- **Sending keys to a stuck pane (3a/3b)** reuses the playbook executor's
  send-keys path and its controller exclusion; the controller is never sent
  keys.
- **Auto-safe default on** changes hosts' files without asking. It is
  limited to fleet's own entries and files, every change is audited and
  announced, and one setting turns it off.
