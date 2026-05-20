# Account model (SSH iteration 2)

**Date:** 2026-05-20
**Author:** brainstorming dialog (M.J. + Claude)
**Status:** Design (awaiting user review → implementation plan)

## Goal

Make "claude account" a first-class entity in claude-fleet so users can see *which account is logged in on which host*. Supports two motivating modes:

- **Mode A** — different claude accounts on different machines (e.g., personal on the laptop, work on mefistos).
- **Mode B** — one account spread across multiple machines (same login on mac + mefistos).

This is iteration 2 of three. Iter 1 (SSH multi-host foundations) is shipped; iter 3 (cross-host session tracking + prompt transfer) depends on the account model this iteration delivers.

## Scope

In:

- New `accounts` table normalized off `hosts` via FK
- Probe extension: read remote `~/.claude.json`'s `oauthAccount` and upsert
- Tauri command `list_accounts`
- Frontend `accounts.ts` store
- UI surfacing in three places: Settings dialog Account column, host pill tooltip, SessionDetails Account row
- AddHostPicker shows detected account before the user confirms `Add`

Out:

- Cross-host "this worktree had a session on host A under account X and host B under account Y" memory — iter 3
- Prompt transfer between accounts — iter 3
- Account-aware filtering in NewSessionDialog (host picker doesn't grey-out by account) — iter 3 if needed
- Auto-refresh account on every reconcile — out per user choice; only manual `↻` in Settings triggers a re-probe
- Cleanup of `accounts` rows whose hosts have all been removed — iter 3 (low value until ref-count matters)
- Per-account UI filter on the sidebar — not yet needed

## Architecture

```
┌──────────────────────────────────────┐
│  ~/.claude.json (local)              │
│    .oauthAccount                     │
│      .accountUuid       ◄────┐       │
│      .emailAddress           │       │
│      .displayName            │       │ probe (1 RTT)
│      .organizationName       │       │ extends iter-1 probe
│      .organizationUuid       │       │
│      .seatTier               │       │
└──────────────────────────────┼───────┘
                               │
            ssh host 'cat …json | jq -c .oauthAccount'
                               │
                               ▼
                      ┌──────────────────┐
                      │  accounts table  │
                      │   uuid PK        │◄──── hosts.account_uuid FK
                      │   email, name…   │      (nullable)
                      └──────────────────┘
```

Two new modules' worth of code, mostly wiring:

- `migrations/003_accounts.sql` — schema
- `src-tauri/src/store.rs` — `AccountRow` struct + 4 CRUD helpers
- `src-tauri/src/commands/hosts.rs` — `probe` extended; new `list_accounts` command
- `src/lib/accounts.ts` — frontend store + IPC wrapper
- Component updates: `Sidebar.svelte` (tooltip), `SettingsDialog.svelte` (Account column), `AddHostPicker.svelte` (probe preview gate), `SessionDetails.svelte` (Host + Account rows)

No changes to PTY / tmux / sessions paths from iter 1.

## Data model

Migration 003:

```sql
CREATE TABLE IF NOT EXISTS accounts (
  uuid                TEXT PRIMARY KEY,
  email               TEXT,
  display_name        TEXT,
  organization_name   TEXT,
  organization_uuid   TEXT,
  seat_tier           TEXT,
  last_seen_at        INTEGER
);

ALTER TABLE hosts ADD COLUMN account_uuid TEXT REFERENCES accounts(uuid);

INSERT OR IGNORE INTO schema_version (version) VALUES (3);
```

`uuid` (Anthropic's stable `accountUuid` from oauthAccount) is the primary key. Other six columns are nullable mirrors of the JSON fields. `last_seen_at` is unix ts of the most recent successful probe — useful for staleness display in iter 3 even if iter 2 doesn't surface it.

`hosts.account_uuid` is nullable: a host without claude installed or logged in keeps `account_uuid = NULL` and renders as `account: —` in the UI.

`store.rs` exposes (in addition to existing host helpers):

```rust
pub struct AccountRow {
    pub uuid: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization_name: Option<String>,
    pub organization_uuid: Option<String>,
    pub seat_tier: Option<String>,
    pub last_seen_at: Option<i64>,
}

impl Store {
    fn list_accounts(&self) -> Result<Vec<AccountRow>, rusqlite::Error>;
    fn upsert_account(&self, account: &AccountRow) -> Result<(), rusqlite::Error>;
    fn get_account_by_uuid(&self, uuid: &str) -> Result<Option<AccountRow>, rusqlite::Error>;
    fn set_host_account(&self, alias: &str, uuid: Option<&str>) -> Result<(), rusqlite::Error>;
}
```

`set_host_account` accepts `Option<&str>` because a probe that finds no account explicitly sets `account_uuid = NULL` rather than leaving the previous value stale.

## Probe v2

The iter-1 probe script was:

```bash
tmux -V 2>/dev/null || true; echo ---; claude --version 2>/dev/null || true
```

Iter-2 appends a third section reading `~/.claude.json.oauthAccount`:

```bash
tmux -V 2>/dev/null || true
echo ---
claude --version 2>/dev/null || true
echo ---
( cat "$HOME/.claude.json" 2>/dev/null | jq -c .oauthAccount 2>/dev/null \
  || python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])).get("oauthAccount") or {}))' "$HOME/.claude.json" 2>/dev/null \
  || true )
```

Single round trip; one shared marker (`---`) splits the three sections. `probe_lenient` continues to absorb any failures into `(false, None, None, None)`.

Rust-side parsing:

```rust
#[derive(serde::Deserialize, Default)]
struct OauthAccount {
    #[serde(rename = "accountUuid")]
    uuid: Option<String>,
    #[serde(rename = "emailAddress")]
    email: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "organizationName")]
    organization_name: Option<String>,
    #[serde(rename = "organizationUuid")]
    organization_uuid: Option<String>,
    #[serde(rename = "seatTier")]
    seat_tier: Option<String>,
}

fn parse_oauth_account(line: &str) -> Option<OauthAccount> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return None;
    }
    serde_json::from_str::<OauthAccount>(trimmed).ok().filter(|a| a.uuid.is_some())
}
```

If `uuid` is missing the result is treated as None (no account). Anthropic could rename fields one day — only the six we read matter, and `Option<String>` makes each independently fault-tolerant.

`probe_local` reads `~/.claude.json` directly via `std::fs::read_to_string` + the same `parse_oauth_account`. No subprocess needed.

## Account linking flow

In `commands/hosts.rs`:

```rust
pub fn add_host(args: AddHostArgs, store, ssh) -> Result<HostRow, IpcError> {
    let result = probe(&ssh, &args.ssh_alias)?;  // strict — errors propagate
    let s = store.lock()...;
    s.insert_host(&args.alias, Some(&args.ssh_alias))?;
    if let Some(acc) = &result.account {
        s.upsert_account(acc)?;
        s.set_host_account(&args.alias, Some(&acc.uuid))?;
    } else {
        s.set_host_account(&args.alias, None)?;
    }
    s.update_host_probe(&args.alias, result.reachable, ...)?;
    list_one(...)
}
```

Same shape for `probe_host` (uses `probe_lenient`).

`list_hosts` does NOT change shape — the frontend joins via the accounts store rather than the backend doing a SQL JOIN. Reasoning: TS-side JOIN is trivial (Map lookup by uuid), and keeps the IPC payload narrow. Two separate stores stay independent and easy to test.

## Frontend

New `src/lib/accounts.ts`:

```typescript
export interface AccountRow {
  uuid: string;
  email: string | null;
  display_name: string | null;
  organization_name: string | null;
  organization_uuid: string | null;
  seat_tier: string | null;
  last_seen_at: number | null;
}

export const accounts = writable<AccountRow[]>([]);
export async function loadAccounts(): Promise<Result<AccountRow[]>>;
```

Mirrors `hosts.ts` shape exactly — same `loadFoo()` pattern, same fallback on Result Err. No CRUD wrappers beyond load — accounts are read-only from the user's POV; the backend manages them via probe.

UI components consume via a `$derived` map keyed by uuid:

```ts
const accountByUuid = $derived(
  new Map($accounts.map((a) => [a.uuid, a]))
);
function accountLabel(host: HostRow): string {
  if (!host.account_uuid) return '—';
  const acc = accountByUuid.get(host.account_uuid);
  if (!acc) return host.account_uuid; // we have the FK but not the row yet
  const email = acc.email ?? acc.uuid;
  return acc.seat_tier ? `${email} (${acc.seat_tier})` : email;
}
```

`HostRow` interface gains `account_uuid: string | null`.

## UI changes

**AddHostPicker.svelte** — split the "click → add" flow into "click → probe → confirm". After `discoverHosts` lists aliases, clicking a row sends a *probe-only* request, then surfaces a confirmation dialog showing:

```
Add host: mefistos
  Hostname:    192.168.1.50
  tmux:        3.6a ✓
  claude:      2.1.144 ✓
  account:     m.janci@32bit.sk (Max) · "32bit s.r.o."
              (or)
  account:     —  (claude not logged in)

  [ Cancel ]  [ Add ]
```

Implementation note: probe is currently inside `add_host`. To enable the preview, expose a separate `probe_ssh_alias` Tauri command that returns probe results WITHOUT persisting. `add_host` keeps its existing behavior (probe + persist atomically). The picker calls `probe_ssh_alias` for preview, then `add_host` on confirm. Slight redundancy in network calls (preview probe is repeated by add_host) is acceptable for iter 2; a future cleanup could pass the cached probe result through. Tests verify preview shows account, NULL renders as `—`.

**SettingsDialog.svelte** hosts table — inject a new `<td class="account">` column between `claude` and `Status`:

```
Alias       tmux    claude    Account                       Status     ...
local       3.5a    2.1.145   m.janci@32bit.sk (Max)        online     [↻] [🚫]
mefistos    3.6a    2.1.144   m.janci@32bit.sk (Max)        online     [↻] [👁] [×]
mac         3.5b    2.1.130   martin@personal.com (Pro)     offline    ...
papayapos   3.6a    2.1.144   —                             online     ...
```

Identical account on multiple rows is intentional and serves as Mode B's visual signal. CSS keeps the column collapsible (`text-overflow: ellipsis`) for long emails.

**Sidebar host pill tooltip** — extend the existing `title` attribute:

```svelte
title="{h.alias}{vers}{account}"
```

where `account` is `\n${email} (${seatTier})` if known, `\n(no account)` dimmed if NULL. Visual on hover; no other layout change.

**SessionDetails.svelte** — meta-grid gains two rows ABOVE the existing Project row:

```
Host:        mefistos
Account:     m.janci@32bit.sk (Max)
Project:     martin-janci/claude-fleet
Created:     1h ago
Last activity: just now
```

Resolved by looking up `$hosts` by alias → account_uuid → `$accounts` by uuid. If host is gone (stale row, race), Host shows `session.host_alias` as fallback and Account shows `—`.

**NewSessionDialog** — unchanged. The host picker doesn't filter by account; user picks a host, and the host's logged-in account is whatever the latest probe recorded. Iter 3 may add account-aware behaviors here.

## Error handling

| Scenario                                                | Behavior                                                                                            |
|---------------------------------------------------------|-----------------------------------------------------------------------------------------------------|
| `~/.claude.json` missing on remote                      | `parse_oauth_account` returns None; host gets `account_uuid = NULL`                                 |
| `jq` AND `python3` both absent                          | bash script's third section returns empty; same as missing file                                     |
| JSON parseable but missing `accountUuid`                | parse_oauth_account filters to None; account row NOT created                                        |
| JSON parseable but extra unknown fields                 | serde ignores them; only the 6 known fields stored                                                  |
| User re-logs into same account                          | upsert hits same uuid PK; non-uuid fields refreshed; hosts.account_uuid stays same                  |
| User logs into different account on same host           | new accounts row created; `set_host_account` updates host's FK; old account row stays (not deleted) |
| Host removed                                            | `delete_host` already cascades sessions; accounts row stays (other hosts may reference it)          |
| Probe times out mid-fetch                               | `probe_lenient` absorbs error → `(false, None, None, None)`; UI shows host offline, account `—`     |
| Account info changes mid-session                        | Existing sessions/PTYs not affected; UI updates next time user clicks `↻`                            |

## Test plan

Pure-logic:

- `parse_oauth_account` — 4 cases: full JSON, missing optional fields, malformed JSON, `{}` / `null` / empty
- `Store::upsert_account` — insert, conflict-update keeps PK, non-uuid fields refresh
- `Store::set_host_account` — Some(uuid) sets, None clears, FK is correctly nullable
- `Store::list_accounts` ordering — UUID asc (stable for tests)
- Migration 003 — adds `accounts` table + `hosts.account_uuid` column; idempotent re-run

Component / integration:

- `accounts.ts` — load, error, store population (5 tests mirror hosts.ts)
- `Sidebar.svelte` — tooltip includes account email + tier for a host with account
- `Sidebar.svelte` — tooltip shows `(no account)` when host.account_uuid is null
- `SettingsDialog.svelte` — Account column cell shows `email (tier)`
- `SettingsDialog.svelte` — Account column cell shows `—` for null
- `AddHostPicker.svelte` — probe preview surfaces account before Add
- `AddHostPicker.svelte` — preview shows `—` for hosts without claude
- `SessionDetails.svelte` — Account row renders correctly
- `SessionDetails.svelte` — Account `—` when host has no account_uuid

Live verification (manual):

1. Probe local — should detect own `m.janci@32bit.sk` and show in Settings ✓
2. (When mefistos online) Probe mefistos — same account picked up; Mode B confirmed
3. Login to claude on remote as different account (if user has alt) — Re-probe → new account row; old row stays
4. Logout of claude on remote — Re-probe → host.account_uuid set to NULL; UI shows `—`

## Implementation slices

8 commits in sequence:

1. **Migration 003 + AccountRow + 4 store helpers** with unit tests
2. **Probe v2** — extend bash script + add `parse_oauth_account` parser + integration into `probe()` + `probe_local()`, 4 unit tests for parser
3. **Account linking in commands** — `add_host` + `probe_host` upsert account and set host FK; new `list_accounts` Tauri command; new `probe_ssh_alias` (preview-only) command for AddHostPicker
4. **Frontend `accounts.ts`** — store + IPC wrapper + tests
5. **Sidebar tooltip** — append account info to host pill `title`
6. **SettingsDialog Account column** — table header + cell + CSS
7. **AddHostPicker preview gate** — split click into probe → confirm two-step
8. **SessionDetails Host + Account rows** — meta-grid update

Each commit ends with `cargo test + pnpm vitest run` green.

## Open risks

- **`accountUuid` stability across re-auth**: assumed stable (Anthropic OAuth convention). If not, users will accumulate duplicate accounts rows (harmless, but messy). First live test confirms or busts this assumption.
- **`~/.claude.json` schema churn**: Anthropic could rename `oauthAccount` → `account` or move it under `auth.oauth.account`. Mitigation: serde tolerates missing fields; if the entire path is gone, all probes return account = None and the app gracefully shows `—` until we patch the parser.
- **Privacy**: `~/.claude.json` is sensitive (also contains tokens / message history). We never read anything outside `.oauthAccount`. Test fixtures use synthetic data only.
- **Probe latency increase**: third section adds one `cat | jq` invocation per probe. Negligible (<50ms over established ControlMaster).

## Non-goals

Iteration 2 explicitly does NOT:

- Add account-keyed memory for "which session belongs to which (host, account, worktree)" — that's iter 3
- Allow the user to override / rename / hide accounts — accounts are auto-managed from the source-of-truth (`~/.claude.json`)
- Filter the host picker by account in NewSessionDialog
- Reconcile accounts on every session list (only on manual `↻`)
- Surface organization-level groupings in the sidebar (organization_name is stored for iter 3 use but not displayed in iter 2 beyond a hover detail)
