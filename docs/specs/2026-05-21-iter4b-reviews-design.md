# claude-fleet iter 4b — Reviews-as-a-feature design

**Status:** Draft
**Author:** Martin via Claude
**Date:** 2026-05-21
**Sibling specs:** `2026-05-20-cross-host-sessions-and-transfer-design.md` (iter 3), `2026-05-20-iter4a-responsiveness-design.md` (iter 4a)

## Goal

Let the user trigger a code review of a working session's output, on demand, from inside claude-fleet. A review is itself an interactive `claude` tmux session — spawned in the same worktree as the source session, seeded with a multipass review prompt — that the user watches and steers in the embedded terminal. This reuses claude-fleet's existing session machinery (spawn, attach, reconcile, send-prompt) rather than introducing a separate review engine.

## Decisions (locked during brainstorming)

- **Trigger:** manual, on-demand. No auto-triggers in v1.
- **Mechanism:** spawn an interactive `claude` tmux session (not headless `claude -p`, not the API). The review is a session you watch.
- **Location:** same host + same worktree as the source session. The reviewer reads the exact current git state, including uncommitted changes.
- **Seed prompt:** an editable default multipass template, shown in a dialog before launch so the user can tweak it per review.
- **Verdict:** lives in the terminal scrollback. No structured-verdict extraction in v1.

## Non-goals (explicitly out of scope for v1)

- Auto-triggering reviews (on commit, on idle, on schedule).
- Scraping / parsing / storing a structured verdict.
- A fleet-wide "needs attention" review dashboard.
- A dedicated review-history view.
- Named review profiles (Quick / Thorough / Security). The single editable template covers v1.
- Reviews in a fresh isolated worktree. v1 reviews in-place.

These all build cleanly on the v1 foundation; deferring keeps the first version shippable.

## Architecture & data flow

```
SessionDetails ("Review" button)
        │
        ▼
  ReviewDialog (modal)
   - shows source session
   - editable textarea (default multipass template)
   - "Start review"
        │
        ▼
  spawn_review(source_session_id, prompt)  [Tauri command]
   1. read source session row → (host_alias, worktree path, project_id, worktree_id)
   2. derive a review tmux name: "<source_tmux_name>--review-<shortid>"
   3. spawn tmux session on source's host, cwd = source's worktree, running `cl`
   4. record the new session: kind='review', reviews_session_id=<source.id>
   5. send the review prompt via the existing send_prompt path (literal send-keys + Enter)
   6. return the new review SessionRow
        │
        ▼
  Frontend mergeSession(reviewRow) → Sidebar shows it (🔍 badge),
  user clicks it → attaches in TerminalView → watches/steers the review live
```

The reviewer `claude` gathers the diff agentically (`git diff`, `git log`, reads files) — claude-fleet does not pre-compute or embed the diff.

## Data model — migration 005

```sql
-- 005_session_reviews.sql
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'work';
ALTER TABLE sessions ADD COLUMN reviews_session_id INTEGER REFERENCES sessions(id);
INSERT OR IGNORE INTO schema_version (version) VALUES (5);
```

- `kind`: `'work'` (default) or `'review'`. Sessions discovered via reconcile that claude-fleet didn't spawn as reviews stay `'work'`.
- `reviews_session_id`: for a review session, the `id` of the session it reviews. NULL for work sessions.
- `SessionRow` (Rust + TS) gains `kind: String` / `kind: string` and `reviews_session_id: Option<i64>` / `reviews_session_id: number | null`.
- `health.rs` schema assertion bumps to 5.

**Preservation invariant (mirrors iter 3's `account_uuid`):** reconcile must not clobber `kind` / `reviews_session_id` on re-probe. The reconcile write path reads the existing row's values first and falls back to defaults only for genuinely new rows. `upsert_session` gains the two new params; the reconcile caller passes the preserved values.

## Backend changes

### Migration + Store (`src-tauri/src/store.rs`, `migrations/005_session_reviews.sql`)
- New migration file + `if v < 5` block in `migrate()`.
- `SessionRow` struct + all `SELECT` projections that build it gain the two columns.
- `upsert_session` signature gains `kind: &str` and `reviews_session_id: Option<i64>`.
- New helper `get_session_kind_and_review(host_alias, tmux_name) -> Result<Option<(String, Option<i64>)>>` for the reconcile preservation read (or fold into the existing `get_session_account` pattern — a single "read existing review/account metadata" helper is cleaner).
- New event: `kind`/`reviews_session_id` ride along inside the existing `session:created` / `session:updated` payloads (they're part of `SessionRow`), so no new event type is needed.

### `spawn_review` command (`src-tauri/src/commands/sessions.rs`)
```rust
#[derive(Deserialize)]
pub struct SpawnReviewArgs {
    pub source_session_id: i64,
    pub prompt: String,
    pub call_id: Option<u64>,   // for cancellation, consistent with new_session
}

#[tauri::command]
pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> { ... }
```
Flow:
1. Snapshot the source session row under a brief lock → host_alias, worktree path, project_id, worktree_id. (The worktree path comes from the worktrees table via worktree_id, or the source session's recorded cwd.)
2. Off-lock: spawn the tmux session (`exec_for(host).new_session(review_name, worktree_path, "cl")`), then `reconcile_one_host` to register it, then send the prompt via the same literal-send-keys path `send_prompt` uses.
3. Under a brief lock: set `kind='review'`, `reviews_session_id=source.id` on the new row (upsert with the review metadata).
4. Return the review `SessionRow`.

Reuses: `exec_for`, `new_session`'s spawn logic, `reconcile_one_host`, `send_prompt`'s `build_send_commands` + `shell_quote_str`.

### Worktree path resolution
The source session has `worktree_id`. Look up the worktree's `path` to use as the review session's cwd. If `worktree_id` is NULL (orphan / cross-host), fall back to the source session's recorded working directory if available, else the project base_path, else error `E_INVALID` ("cannot determine source worktree path").

## Frontend changes

### Types (`src/lib/sessions.ts`)
- `SessionRow` gains `kind: string` and `reviews_session_id: number | null`.
- New wrapper: `spawnReview(sourceSessionId, prompt, signal?) → Result<SessionRow>` using `invokeCmdAbortable` (so the spawn — which may include a remote tmux create — is cancellable, consistent with iter 4a). On success, `mergeSession(reviewRow)`.

### ReviewDialog (`src/lib/ReviewDialog.svelte`, new)
- Props: `{ source: SessionRow, onClose: () => void }`.
- Shows the source session (host badge + tmux_name).
- Editable textarea pre-filled with `DEFAULT_REVIEW_PROMPT` (a module constant).
- "Start review" button (disabled while spawning); "Cancel" stays live (AbortController, like iter 4a's dialogs).
- On start: `spawnReview(source.id, prompt, controller.signal)`; on success, optionally `selectSession(reviewRow)` to focus it; `onClose()`.

`DEFAULT_REVIEW_PROMPT` (a starting point; user edits freely):
> "Review the work in this worktree. Run `git diff` and `git log` against the base branch to see what changed. Pass 1 — correctness: does the code do what it should, any bugs? Pass 2 — code quality: clarity, structure, tests. Pass 3 — risk: anything dangerous, security-sensitive, or destructive? Be specific with file:line references. End with an overall verdict: approve / approve-with-fixes / needs-rework."

### SessionDetails (`src/lib/SessionDetails.svelte`)
- Add a "🔍 Review" button to the action row (next to "→ Send prompt"), opening ReviewDialog with `source={session}`.
- If `session.kind === 'review'` and `session.reviews_session_id` is set: show a "Reviewing: `<source tmux_name>`" row with click-to-switch (resolve the source row from the `$sessions` store by id).
- If the session has reviews pointing at it (other sessions where `reviews_session_id === session.id`): show them in a "Reviews" list, reusing the Related-sessions panel pattern (computed `$derived` from `$sessions`, no extra IPC).

### Sidebar (`src/lib/Sidebar.svelte`)
- Review sessions render a 🔍 badge in the session row (distinct from the 🔗N related badge). Compute via `s.kind === 'review'` — O(1), no new index needed.

## Error handling

| Scenario | Behaviour |
|---|---|
| Source session has no resolvable worktree path | `E_INVALID`, dialog shows the message, no session spawned. |
| tmux spawn fails on the host | `E_TMUX` / `E_SSH` surfaced in the dialog; nothing registered. |
| Prompt send fails after spawn | Review session exists (registered) but un-seeded; return the row with a non-fatal warning so the user can type the prompt manually in the terminal. |
| User cancels mid-spawn | `E_CANCELLED`; if the tmux session was already created, it's left in place (the user can kill it) — document this. |
| Source session is itself a review | Allowed (review-of-a-review); no special handling. |

## Testing strategy

### Rust (current 104 passing)
- Migration 005: `kind` defaults to `'work'`, `reviews_session_id` nullable; schema_version → 5.
- `upsert_session` round-trips `kind` + `reviews_session_id`.
- Reconcile preserves `kind='review'` + `reviews_session_id` on re-probe (mirror the existing account_uuid preservation test).
- `spawn_review` registers a row with `kind='review'` + correct `reviews_session_id` (using a mock TmuxExec so no real tmux/ssh).
- `health.rs` asserts schema_version 5.

### Vitest (current 150 passing)
- `spawnReview` wrapper merges the returned review row into the store.
- ReviewDialog: renders source + default prompt, "Start review" disabled while spawning, calls `spawn_review` with the edited prompt.
- SessionDetails: shows "Reviewing: X" for a review session; shows the Reviews list for a source session.
- Sidebar: 🔍 badge appears for `kind === 'review'` rows.

### Live verify
- Trigger a review on a local session → review session spawns in the same worktree, seeded prompt fires, reviewer starts running `git diff`.
- Cross-host: review a mefistos session → review spawns on mefistos in the same worktree.
- Kill flow: killing a review session works via the existing kill path; killing the source doesn't orphan the review's link (it just dangles — acceptable, the FK is nullable on the source side via the reviews_session_id pointer).

## Milestones / slices

**M1 — Data model:** migration 005 + SessionRow (Rust+TS) + upsert_session params + reconcile preservation + health bump. (~half day)

**M2 — `spawn_review` backend:** command + worktree-path resolution + reuse of spawn/reconcile/send-prompt + tests. (~half day)

**M3 — ReviewDialog + spawnReview wrapper:** dialog component, default template constant, abortable spawn, store merge. (~half day)

**M4 — SessionDetails + Sidebar surfacing:** Review button, "Reviewing: X" row, Reviews list, 🔍 badge. (~half day)

**M5 — Live verify + push.** (interactive)

Each milestone is independently committable. Estimated ~2 days total.

## Open risks

1. **Two claudes in one worktree.** The reviewer and an actively-writing source session share the filesystem + git index. Review is read-only by nature, but `git` operations (e.g. the reviewer running `git status`) touch `.git`. Low risk in practice (the user triggers a review at a checkpoint), but worth a one-line note in the ReviewDialog ("reviews the worktree's current state").
2. **Seeding timing.** `cl` (claude) needs a moment to start its TUI before `send-keys` will land in the prompt. The existing `send_prompt` assumes a running REPL; for a freshly-spawned session we may need a short delay or a readiness check before sending. M2 must handle this (a brief sleep, or poll the pane for the prompt indicator).
3. **Review name collisions.** `<source>--review-<shortid>` must be unique per host. Use a short random/time-based id; on the rare collision, tmux new-session fails and we retry with a new id.
4. **Worktree path for cross-host.** Remote worktree paths differ from local. The source session's recorded cwd (from tmux reconcile) is the host-correct path — prefer it over reconstructing from the worktrees table, which holds local paths.

## Self-review

- **Placeholder scan:** no TBD/TODO; the default prompt and migration SQL are concrete; every component names its file.
- **Internal consistency:** `kind` + `reviews_session_id` defined once in the data model, used consistently in backend, types, and UI. Events ride inside the existing `SessionRow` payloads (no new event types) — consistent with M3's design.
- **Scope check:** one feature, one spec, 5 milestones. Non-goals explicitly fenced.
- **Ambiguity check:** worktree-path resolution order is spelled out (worktree_id path → source cwd → project base_path → error); seeding-timing risk is flagged for M2; cancellation-mid-spawn behaviour is defined.
