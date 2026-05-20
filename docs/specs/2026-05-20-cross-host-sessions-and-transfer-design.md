# Cross-host session memory + prompt transfer (SSH iteration 3)

**Date:** 2026-05-20
**Author:** brainstorming dialog (M.J. + Claude)
**Status:** Design (awaiting user review → implementation plan)

## Goal

Two intertwined features built on the iter-1 multi-host + iter-2 account model foundations:

1. **Cross-host session memory** — when the user has worktree X in active development across multiple hosts (Mode B) or multiple accounts (Mode A), claude-fleet remembers which tmux session belongs to which `(host, account, worktree)` tuple. UI surfaces siblings via SessionDetails "Related sessions" panel + sidebar 🔗N badge.

2. **Prompt transfer** — user can compose a plain-text prompt in a Composer dialog, pick one or more target sessions (defaulting to related-for-this-worktree), and the app delivers the text into each target via `tmux send-keys + Enter`. Fire-and-forget; no audit log in iter 3.

This is iteration 3 of 3 in the multi-host expansion.

## Scope

In:

- New `sessions.account_uuid` column (cached at session creation; preserved across re-probes)
- `reconcile_sessions` captures `host.account_uuid` ONLY for newly upserted sessions; existing rows untouched
- New Tauri command `related_sessions(session_id)` returning siblings sharing `(project_id, worktree_id)`
- Sidebar 🔗N badge per session row (passive indicator, shows count of related sessions)
- SessionDetails "Related sessions" sub-panel listing siblings with click-to-switch
- New Tauri command `send_prompt(host_alias, tmux_name, prompt)` issuing `tmux send-keys -t name -l <text>` + Enter (local: direct; remote: via SshClient)
- New component `PromptComposer.svelte` opened from SessionDetails "→ Send prompt" button: targets checklist (default = related; toggle to expand to all fleet sessions) + textarea + Send action
- Sequential per-target send loop with inline error display in composer
- 8 task slices, each ending with a commit
- Live verify against local + (when reachable) mefistos

Out:

- Conversation export / last-prompt prefill from source claude (option b/c from prompt-transfer brainstorm). User types fresh text only.
- "Wait-until-idle" probe before send-keys (if claude is streaming, the prompt may interrupt — same risk as manual paste, accepted)
- Audit log of transfers in `handoffs` table (deferred; transfers are fire-and-forget for iter 3)
- Multi-target parallel send (sequential is fast enough at iter-3 fleet sizes)
- Sidebar restructure to show worktrees explicitly (user explicitly rejected in P3T6; we keep flat project→sessions tree with badge addition only)
- Auto-update of `sessions.account_uuid` if host re-auths (preservation invariant — session keeps the account it was CREATED under)
- Account-aware filter in NewSessionDialog or session search
- Account-keyed worktree session lookup ("show me my work for X under work-account") — possible follow-up
- Cleanup of orphan accounts rows whose hosts AND sessions are all gone — possible follow-up

## Architecture

Three primitives, all built on existing data:

```
                    ┌──────────────────────┐
                    │  reconcile_sessions  │
                    └──────────────────────┘
                              │
                              ▼ on UPSERT of NEW row
                    captures hosts.account_uuid
                              │
                              ▼
┌──────────────────────────────────────────────────┐
│  sessions                                        │
│    id, tmux_name, host_alias, project_id,        │
│    worktree_id, account_uuid (NEW), …            │
└──────────────────────────────────────────────────┘
       │                          │
       │ project_id,              │ tmux_name
       │ worktree_id              │ host_alias
       ▼                          ▼
┌─────────────────────┐    ┌──────────────────────┐
│ related_sessions    │    │ send_prompt          │
│ (other rows with    │    │ (tmux send-keys      │
│  same project+wt)   │    │  local or via ssh)   │
└─────────────────────┘    └──────────────────────┘
       │                          ▲
       │ feeds                    │ targets from
       ▼                          │
┌─────────────────────┐    ┌──────────────────────┐
│ SessionDetails      │───▶│ PromptComposer       │
│   Related panel     │    │   targets + text     │
│ Sidebar 🔗N badge   │    └──────────────────────┘
└─────────────────────┘
```

No new long-lived processes, no new IPC streams. Send is per-target Tauri command call. Composer iterates serially.

## Data model

Migration 004 (`src-tauri/migrations/004_session_account.sql`):

```sql
ALTER TABLE sessions ADD COLUMN account_uuid TEXT REFERENCES accounts(uuid);
INSERT OR IGNORE INTO schema_version (version) VALUES (4);
```

Nullable FK. NULL means "session was created on a host without a logged-in claude account" (the host probed but had no oauthAccount). Such sessions still work; just no account context.

`SessionRow` gains a field:

```rust
pub struct SessionRow {
    // ... existing fields ...
    pub account_uuid: Option<String>,
}
```

`Store::upsert_session` signature extended to take `account_uuid: Option<&str>` (eighth arg after the existing seven). Caller decides:
- For brand-new tmux sessions (not in DB yet): pass `host.account_uuid` snapshot
- For existing tmux sessions (already in DB): pass the existing `sessions.account_uuid` to preserve it

`Store::list_sessions_for_host` SELECT widens to include the new column. Existing tests get one new field in their fixtures.

## Reconciliation logic

`commands::sessions::reconcile_sessions` per-host loop currently does:

```rust
for sess in &live {
    s.upsert_session(&sess.name, &host.alias, project_id, None,
                     sess.created, sess.last_activity, "running")?;
}
```

After iter 3:

```rust
for sess in &live {
    // Preserve existing account_uuid if session was already in DB; capture
    // host's CURRENT account_uuid only for newly discovered sessions.
    let existing = s.get_session_account(&host.alias, &sess.name)?;  // helper
    let account_uuid = existing.or_else(|| host.account_uuid.clone());
    s.upsert_session(&sess.name, &host.alias, project_id, None,
                     sess.created, sess.last_activity, "running",
                     account_uuid.as_deref())?;
}
```

New helper `Store::get_session_account(host, tmux_name) -> Result<Option<String>>` returns the persisted account_uuid (or None if row doesn't exist yet). This is the preservation knob: existing rows keep their original account_uuid even if the host re-auths.

**Trade-off acknowledged:** if a user kills a session and creates a new one with the same name on the same host after re-authing, the new session will pick up the OLD account_uuid because the DB row from the previous instance is still there until reconcile prunes it. The reconcile flow does prune (`delete_sessions_not_in`) before upserting fresh rows in a single pass — verify the pruning happens BEFORE the upsert loop. Plan: explicit step in Task 2 to confirm the order.

## Related sessions API

New Tauri command (in `commands/sessions.rs`):

```rust
#[derive(Deserialize)]
pub struct RelatedSessionsArgs {
    pub session_id: i64,
}

#[tauri::command]
pub fn related_sessions(
    args: RelatedSessionsArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<Vec<SessionRow>, IpcError> {
    let s = store.lock()...;
    s.list_related_sessions(args.session_id).map_err(IpcError::from)
}
```

Backing Store helper `list_related_sessions(session_id) -> Result<Vec<SessionRow>>`:

- Find the source session's `(project_id, worktree_id)` from DB
- SELECT sessions WHERE project_id=? AND ((? IS NULL AND worktree_id IS NULL) OR worktree_id=?) AND id<>?
- ORDER BY host_alias ASC, tmux_name ASC

Frontend approach: rather than a separate IPC call per SessionDetails render, the frontend computes related-count and the related list directly from the existing `$sessions` store via a `$derived`. This avoids extra round trips and keeps badge updates instant when sessions list refreshes. The Rust command exists for completeness / future cross-process callers but the UI uses the in-memory store.

```typescript
function relatedFor(session: SessionRow, all: SessionRow[]): SessionRow[] {
  if (session.project_id === null) return [];
  return all.filter(
    (s) =>
      s.id !== session.id &&
      s.project_id === session.project_id &&
      s.worktree_id === session.worktree_id,
  );
}
```

`session.project_id === null` (orphans) → no relateds. Pragmatic: orphans likely have no shared identity.

## UI changes

### SessionDetails: Related panel

After the existing meta `<dl>` and action button row, new section:

```svelte
{#if related.length > 0}
  <section class="related" data-testid="related-sessions">
    <h3>Related sessions ({related.length})</h3>
    <ul>
      {#each related as r (r.id)}
        <li>
          <button class="related-row" onclick={() => selectSession(r)}>
            <span class="host-badge">[{r.host_alias}]</span>
            <span class="account">{accountText(accountFor(r))}</span>
            <span class="status-dot status-{r.status}" title={r.status}></span>
            <span class="sess-name">{r.tmux_name}</span>
            <span class="age">{formatRelative(r.last_activity_at)}</span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}
```

`accountFor(r)` looks up `$accounts.find(a => a.uuid === r.account_uuid)`. `accountText` is the same formatter already in SessionDetails (M1 from iter-2 review — duplication still tolerated for now; iter 4 can extract a shared util).

Click on a row swaps `selectedSession` and TerminalView re-attaches.

### Sidebar: 🔗N badge

Each session row in the project-grouped tree (and orphan section) gets a small badge after the status dot, before the [host] badge:

```svelte
{#if relatedCount(sess) > 0}
  <span class="related-badge" data-testid="related-badge" title="{relatedCount(sess)} related session(s)">🔗{relatedCount(sess)}</span>
{/if}
```

Helper `relatedCount(sess)` shares the same filter as `relatedFor` above. Passive indicator only — no click handler. User clicks the session row as usual, then sees the Related panel in SessionDetails.

CSS keeps the badge small (0.65rem, var(--fg-muted), max-width content). For N=0 the badge does not render.

### PromptComposer.svelte (new component)

Opened by SessionDetails action button `→ Send prompt`:

```svelte
<button class="ghost" onclick={openComposer} data-testid="send-prompt-from-details">
  → Send prompt
</button>
```

Composer modal layout (in-app, same backdrop pattern as other modals):

```
┌─────────────────────────────────────────┐
│ Send prompt to session(s)               │
├─────────────────────────────────────────┤
│ Targets:                                │
│  [✓] [mefistos] m.janci   dev-foo       │
│  [ ] [local]    m.janci   dev-foo       │
│  ─── Show all fleet sessions ─── [ ]    │
│  (if toggled, expands to others)        │
├─────────────────────────────────────────┤
│ Prompt:                                 │
│ ┌─────────────────────────────────────┐ │
│ │ < textarea, 8 rows, monospace >     │ │
│ └─────────────────────────────────────┘ │
│                                         │
│ [error per-target list, if any]         │
│                                         │
│ [ Cancel ]                  [ Send → ]  │
└─────────────────────────────────────────┘
```

Behavior:

- Default targets = relateds for the source session (all pre-checked except source itself, which is excluded from the list — sending a prompt to yourself would be a no-op + the user could just type)
- Toggle "Show all fleet sessions" expands to include other non-related sessions (across all projects), unchecked by default
- Textarea: monospace, no markdown parsing, raw string sent as typed
- Send button disabled when no target checked OR prompt is empty
- During send: button label changes to "Sending…", disabled; per-target spinner next to each target row
- On per-target failure: inline `✗ <error message>` next to that target; other targets continue
- All targets done (success or fail): button changes to "Done" for 1s, then modal auto-closes if all succeeded, stays open with error list if any failed (user can retry)

Component file count stays manageable: PromptComposer is ~180 LOC including styles.

## send_prompt backend

`commands::sessions::send_prompt`:

```rust
#[derive(Deserialize)]
pub struct SendPromptArgs {
    pub host_alias: String,
    pub tmux_name: String,
    pub prompt: String,
}

#[tauri::command]
pub fn send_prompt(
    args: SendPromptArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let cmds = build_send_commands(&args.tmux_name, &args.prompt);
    if args.host_alias == "local" {
        run_local(&cmds)
    } else {
        run_remote(&ssh, &args.host_alias, &cmds)
    }
}
```

Helpers:

```rust
/// Returns the two tmux invocations needed: send-keys -l <text>, then
/// send-keys Enter.
fn build_send_commands(tmux_name: &str, prompt: &str) -> Vec<String> {
    vec![
        format!("tmux send-keys -t {} -l {}", shell_quote(tmux_name), shell_quote(prompt)),
        format!("tmux send-keys -t {} Enter", shell_quote(tmux_name)),
    ]
}

fn run_local(cmds: &[String]) -> Result<(), IpcError> {
    for cmd in cmds {
        let out = std::process::Command::new("bash").args(["-c", cmd]).output()?;
        if !out.status.success() {
            return Err(IpcError::new("E_TMUX",
                String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
    }
    Ok(())
}

fn run_remote(ssh: &Arc<SshClient>, host: &str, cmds: &[String]) -> Result<(), IpcError> {
    for cmd in cmds {
        let quoted = shell_quote(cmd);
        let out = ssh.run(host, &["bash", "-lc", &quoted], Duration::from_secs(10))?;
        if !out.status.success() {
            return Err(IpcError::new("E_TMUX",
                String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
    }
    Ok(())
}
```

The `-l` flag on tmux send-keys means LITERAL — every byte goes to the pane verbatim, no key-name translation (so a prompt containing the word "Enter" wouldn't fire a real Enter). The second command sends a real Enter to submit the prompt.

`shell_quote` is the existing helper from `tmux.rs` (M1/M2 from iter-2 review — extract to shared util is iter-4 cleanup; for now we re-use via path).

Frontend wrapper in `sessions.ts`:

```typescript
export async function sendPrompt(
  hostAlias: string, tmuxName: string, prompt: string,
): Promise<Result<void>> {
  return invokeCmd<void>('send_prompt', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, prompt },
  });
}
```

## Error handling

| Scenario                                              | Behavior                                                                    |
|-------------------------------------------------------|-----------------------------------------------------------------------------|
| Source session is orphan (project_id=null)            | Related panel hidden; sidebar badge not shown; composer offers fleet-mode   |
| Source session has no worktree (worktree_id=null)     | Related = sessions sharing project, with worktree_id also null              |
| Target host offline mid-send                          | per-target E_SSH error; other targets continue; composer surfaces inline    |
| Target tmux session no longer exists                  | tmux send-keys returns non-zero; E_TMUX with stderr; composer shows         |
| Target shell (claude exited)                          | send-keys lands in bash; bash interprets text as command. User warned via ⚠ for sessions with status != "running" |
| Empty prompt or no targets                            | Send button disabled                                                        |
| Very long prompt (>10KB)                              | tmux send-keys handles fine via `-l`; no chunking needed                    |
| Multi-line prompt                                     | `-l` preserves newlines; second send-keys Enter still submits as one prompt |
| User-altered host.account_uuid mid-session            | Existing sessions keep their captured account_uuid (preservation invariant) |

## Test plan

Pure-logic unit tests (cargo):

- `Migration 004` adds the column, idempotent
- `Store::upsert_session` accepts account_uuid; null and non-null both round-trip
- `Store::get_session_account` returns None for missing, Some(uuid) for present
- `Store::list_related_sessions` filters by (project, worktree) correctly; excludes self; isolates unrelated
- `build_send_commands` returns exactly 2 strings, both correctly quoted with `-l` for text and `Enter` for submit
- `shell_quote` over a prompt containing single quotes, newlines, dollar signs

Component tests (vitest):

- SessionDetails Related panel renders for sessions with siblings
- SessionDetails Related panel hidden when no siblings or for orphans
- Sidebar 🔗N badge appears for sessions with siblings; absent for solo
- PromptComposer defaults to related targets; toggle expands list
- PromptComposer Send button disabled when empty
- PromptComposer surfaces per-target error inline

Integration (manual):

- Local: open two local sessions in same project+worktree; verify badge appears; click one → Related panel shows the other → click → switches; in source, → Send prompt → target receives via `tmux capture-pane`
- Cross-host (when mefistos reachable): same flow but one target is on mefistos
- Account-cross: if user has separate accounts on local vs mefistos, verify Related panel shows both with distinct Account labels
- Account preservation: re-probe a host whose account_uuid changes (simulate by editing `~/.claude.json`); existing sessions keep their old account_uuid in the panel display

## Implementation slices

| # | Commit |
|---|--------|
| 1 | `store: migration 004 (sessions.account_uuid) + SessionRow update` |
| 2 | `sessions: reconcile captures host account on new sessions (preserves existing)` |
| 3 | `sessions: related_sessions command + frontend wrapper` |
| 4 | `Sidebar: 🔗N related-count badge per session row` |
| 5 | `SessionDetails: Related sessions panel` |
| 6 | `sessions: send_prompt command (tmux send-keys + Enter)` |
| 7 | `PromptComposer: targets picker + textarea + Send action` |
| 8 | `iter 3 live verify + final review` |

Each commit ends with `cargo test --lib + pnpm vitest run` green.

## Open risks

- **Race: claude streaming when prompt arrives** — tmux send-keys may interleave with claude's output. Accepted (same risk as user pasting). Future could query claude REPL idle state.
- **Account preservation vs replay** — if user deletes a tmux session, immediately creates one with the same name after re-authing, the session inherits the OLD account_uuid because reconcile's delete-then-upsert flow is single-pass. Plan Task 2 explicitly verifies the prune-before-upsert order.
- **Multi-line prompt quoting** — `shell_quote` handles single quotes via `'\''`. Multi-line text via `-l` is fine. Verify with a 3-line test prompt containing `it's $HOME` style content.
- **Targets list growth** — for a fleet with 30+ sessions, "Show all fleet sessions" toggle renders 30 checkboxes. Acceptable for iter 3. Pagination/search if iter 4 surfaces complaints.
- **Concurrent sends** — sequential per-target; no race protection needed at our scale. If a user double-clicks Send during a long send, the button is disabled during in-flight so the second click is dropped.

## Non-goals (re-affirmed)

Iteration 3 does NOT:

- Export claude conversation state from source (no JSONL parsing)
- Wait for claude REPL idle before sending
- Persist a transfer history (`handoffs` table stays untouched)
- Re-introduce worktree visibility in the sidebar (user explicitly rejected in P3T6)
- Auto-refresh `sessions.account_uuid` on host re-auth (preservation is the design choice)
- Auto-cleanup unused `accounts` rows when no host or session references them
