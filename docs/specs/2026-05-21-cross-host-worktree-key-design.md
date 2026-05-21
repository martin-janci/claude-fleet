# claude-fleet — Worktree-aware related-session matching (cross-host)

**Status:** Draft
**Author:** Martin via Claude
**Date:** 2026-05-21
**Supersedes:** the "cross-host related-session worktree mapping" open-risk note in `2026-05-20-cross-host-sessions-and-transfer-design.md` (iter 3) — which was based on an incorrect premise (see Background).

## Background — correcting the iter-3 premise

The iter-3 spec flagged that cross-host same-repo sessions wouldn't link as "related" because a local session gets `worktree_id` while a remote gets NULL. **That premise was wrong.** Verified against the code and live DB:

- `reconcile` resolves `project_id` from each session's cwd but **never sets `worktree_id`** — it's NULL for every reconciled session (the `ON CONFLICT` clause also clobbers it to NULL on every re-probe). Live DB: all sessions have `worktree_id = NULL`.
- `list_related_sessions` therefore takes its `(?2 IS NULL AND worktree_id IS NULL)` branch for every session, matching **all same-`project_id` sessions regardless of worktree**.

Consequences:
- **Cross-host already links** at the repo level (both NULL worktree_id + same project_id).
- The real characteristic is **worktree-blind / over-broad matching**: two different worktrees of the same repo (even same-host) are currently treated as related.

## Goal

Make related-session matching **worktree-aware and portable across hosts**: two sessions are related iff they share the same repo AND the same worktree *name*. This tightens the current over-broad matching (different worktrees stop falsely linking) while preserving cross-host linking (which already works at repo level).

The portable identity is the worktree **name** (e.g. `main`, `feature-auth`), derivable from a session's cwd on any host — not the local `worktree_id` (a filesystem-row id that doesn't survive across hosts).

## Non-goals

- Removing the now-vestigial `sessions.worktree_id` column (out of scope; harmless).
- Tracking remote worktrees as rows in the `worktrees` table.
- Any change to the worktree *tree* display in the Sidebar (that uses the `worktrees` table, unaffected).

## Design

### Worktree-key derivation

A pure function in `src-tauri/src/commands/sessions.rs`:

```rust
/// Derive a portable worktree name from a session's working directory.
/// Reuses the `…/projects/github.com/<owner>/<repo>` convention (same regex as
/// extract_owner_repo). Returns the worktree name, host-path-independent:
///   - <repo>/.claude/worktrees/<name>[/…]  → "<name>"
///   - <repo>  (root) or any other subdir   → "main"
///   - path without a github.com repo segment → None (orphan)
fn worktree_key_for_path(path: &str) -> Option<String>
```

Identical output for local (`/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/feat`) and remote (`/home/mjanci/projects/github.com/martin-janci/claude-fleet`) paths. The `"main"` default matches the local `worktrees` table's naming of the main checkout.

### Migration 006

```sql
-- 006_session_worktree_key.sql
ALTER TABLE sessions ADD COLUMN worktree_key TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (6);
```

- Nullable; NULL for orphans / legacy rows until the next reconcile populates them.
- `SessionRow` (Rust + TS) gains `worktree_key: Option<String>` / `worktree_key: string | null`.
- `health.rs` schema assertion → 6.

### Reconcile populates it

Reconcile flows through `ReconcileSession` / `apply_host_reconcile` (added in the Win-B transaction wrap). Changes:
- `ReconcileSession` gains `worktree_key: Option<String>`.
- Both reconcile callers compute it via `worktree_key_for_path(&sess.path)` in the read phase (alongside the existing `find_project_id_for_path`).
- `upsert_session_in_tx` writes it: add `worktree_key` to the INSERT column list and `ON CONFLICT DO UPDATE SET worktree_key = excluded.worktree_key`.
- The public `upsert_session` is **left unchanged** — reconcile is the only path that knows the cwd; direct callers (tests, `set_session_kind`, etc.) don't set it and leave it untouched on conflict. This is an intentional divergence from the `_in_tx` SQL-parallel; the existing maintenance comment is extended to note it.

Because a session with a resolved `project_id` always has a parseable repo segment, it always gets at least `"main"` — so `project_id IS NOT NULL ⟹ worktree_key IS NOT NULL` after reconcile.

### Matching switches to the key (both sites)

Related-matching happens in two places; both change from `worktree_id` to `worktree_key`:

1. **Backend `list_related_sessions`** (`store.rs`):
   ```sql
   SELECT … FROM sessions
   WHERE project_id = ?1
     AND worktree_key IS NOT NULL
     AND worktree_key = ?2
     AND id <> ?3
   ORDER BY host_alias ASC, tmux_name ASC
   ```
   Source lookup reads `(project_id, worktree_key)`; orphans (`project_id` NULL) early-return `[]` as today.

2. **Frontend** (the hot path actually driving the UI):
   - `SessionRow` gains `worktree_key: string | null`.
   - `Sidebar.svelte` `relatedCountById`: grouping key changes from `` `${project_id}:${worktree_id ?? 'null'}` `` to `` `${project_id}:${worktree_key ?? 'null'}` `` — and only sessions with a non-null `worktree_key` participate (null keys group to nothing meaningful; they're transient pre-reconcile).
   - `SessionDetails.svelte` related-panel `$derived`: match siblings on `project_id` AND `worktree_key` (non-null, equal) instead of `worktree_id`.

## Error handling / edge cases

| Case | Behaviour |
|---|---|
| Legacy rows (worktree_key NULL before first reconcile) | Match nothing transiently; populated on app-start reconcile / refresh. Safe. |
| Orphan session (no project) | `worktree_key` NULL; early-returns `[]` (unchanged). |
| Path is a non-worktree subdir of the repo | Keyed as `"main"` (working in the main checkout's tree). |
| Same repo, different worktree, same host | Now correctly NOT related (previously falsely related). |
| Same repo, same worktree name, different hosts | Related (the cross-host goal). |

## Testing strategy

### Rust (current 111)
- `worktree_key_for_path`: unit tests for root→"main", `.claude/worktrees/<name>`→"<name>", remote path→same result as local, non-repo path→None.
- `list_related_sessions`: two sessions same project + same worktree_key → related; same project + different worktree_key → NOT related; cross-host (different host_alias) same key → related; orphan → [].
- migration 006: column exists, default NULL; schema_version 6; `health.rs` asserts 6.
- reconcile sets worktree_key (via `apply_host_reconcile` with a `ReconcileSession` carrying a key → row has it).

### Vitest (current 157)
- Sidebar: two sessions same project + same worktree_key → 🔗1 badge each; different worktree_key → no badge. (Extend the existing related-badge test.)
- SessionDetails: related panel lists same-key siblings, excludes different-key.
- `SessionRow` fixtures gain `worktree_key`.

### Live verify
- Two local sessions in the same worktree → 🔗 link; move one to a different worktree (`.claude/worktrees/x`) → they stop linking.
- A local + a mefistos session in the same repo's `main` → 🔗 link cross-host.

## Slices

- **M1 — Backend:** migration 006 + `SessionRow.worktree_key` + `worktree_key_for_path` + `ReconcileSession`/`upsert_session_in_tx` wiring + `list_related_sessions` query + health bump + Rust tests.
- **M2 — Frontend:** `SessionRow` TS type + Sidebar `relatedCountById` regroup + SessionDetails related `$derived` + vitest + fixtures.
- **M3 — Live verify + push.**

~1 day; each slice independently committable.

## Self-review

- **Placeholder scan:** no TBD/TODO; migration SQL + query + derivation rules are concrete.
- **Internal consistency:** `worktree_key` defined once (data model), used in derivation, reconcile, both match sites, and tests. The `project_id ⟹ worktree_key` invariant is stated and relied on by the `IS NOT NULL` match.
- **Scope check:** one focused change, 3 slices. `worktree_id` removal explicitly out of scope.
- **Ambiguity check:** the `"main"` default for repo-root/other-subdir is explicit; legacy-NULL transient behaviour is defined; the public-vs-`_in_tx` upsert divergence is called out as intentional.
