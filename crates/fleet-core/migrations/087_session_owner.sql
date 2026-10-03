-- Multi-user M1, T3 (plan docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md,
-- spec docs/superpowers/specs/2026-09-30-multi-user-gap-analysis.md §4.3): a
-- SESSION learns whose it is.
--
-- Migration 086 gave the hub its people; this one gives every session row an
-- owner and a visibility, and records — durably, outliving the row — which
-- person a Claude conversation belonged to.
--
-- What this script does, statement by statement (named rather than counted,
-- 086's convention):
--
--   ALTER TABLE sessions            `owner_person_id`: whose session this is
--   ALTER TABLE sessions            `visibility`: `private` | `unclaimed`
--   idx_sessions_owner              the per-person session read
--   CREATE TABLE conversation_owners  who a conversation belonged to, for
--                                   ever — survives the session's reaping
--   trg_conversation_owner_on_session_insert   fill it from `sessions`,
--   trg_conversation_owner_on_session_update   first writer wins
--   DROP/CREATE sessions_row_version_bump      082's trigger, re-issued to
--                                   watch the two new SessionRow fields
--
-- There is deliberately NO `UPDATE sessions` here. The attribution of a
-- pre-M1 row to the hub's one person is `Store::backfill_session_owner`,
-- run by `store/schema.rs::migrate` after the migrations AND after
-- `repair_skipped_main_migrations()`, for the reason migration 080 wrote
-- down: any UPDATE of `sessions` compiles the row-version trigger, which
-- names `lost_reason` — a column that on a conversations-branch database
-- exists only once the repair has added it. An inline UPDATE here aborts
-- that upgrade with `no such column: lost_reason`.

-- Whose session this is. Deliberately NO foreign key to `people`, the
-- migration-066 rationale 086 repeats for `client_tokens.person_id`:
-- deleting a person must leave the row owned by an id nothing has (fail
-- closed — `ViewScope::owns` answers "no" for every caller), never widen it
-- to everybody and never hand it to somebody else by cascade.
--
-- Nullable, and that is why it is NOT what any fence keys on: a nullable
-- integer is *absent* on the event stream, not null (spec §3.7 —
-- `BroadcastEventBus::emit` runs `strip_nulls` before the frame enters the
-- replay ring), and "absent" reads as "no restriction". `visibility` below
-- is the NOT NULL key the fence can safely read.
ALTER TABLE sessions ADD COLUMN owner_person_id INTEGER;

-- `private` for anything a person starts, `unclaimed` for the safe holding
-- state: a row reconcile found on a host, or a pre-M1 row fleet did not
-- create, which nobody can speak for (spec §4.3, the `'unclaimed'` table).
--
-- The CHECK admits those two values and NOTHING else, which is the point of
-- writing it: a third value then costs a migration instead of arriving
-- through a hand-edited database or an UPDATE nobody reviewed. In
-- particular there is no `'org'` — team sharing under another name, removed
-- from M1 by the owner's decision because the only referent the schema has
-- for "the org can see it" is `client_tokens.org_id`, which an admin binds
-- their own device to under `Access::Master`, with no grant touched and no
-- owner consent (spec §4.3, *`visibility` has exactly two values in M1*).
-- `'org'` returns in M2 with memberships and a defined reader.
ALTER TABLE sessions ADD COLUMN visibility TEXT NOT NULL DEFAULT 'unclaimed'
  CHECK (visibility IN ('private', 'unclaimed'));

-- The per-person session read: every scoped list of sessions starts from
-- "the ones this person owns". Not partial — an owner-less row is exactly
-- what the unclaimed count and the claim path look for, and a partial index
-- cannot serve `owner_person_id IS NULL`.
CREATE INDEX IF NOT EXISTS idx_sessions_owner ON sessions(owner_person_id);

-- Who a Claude CONVERSATION belonged to, for as long as the hub lives.
--
-- `new_session { resume_claude_session_id }` resurrects a transcript
-- precisely when the session row is GONE — that is what makes it the
-- interesting half of the takeover (spec §5.2). A check against live rows
-- therefore cannot close it, and neither can one against lost rows:
-- `Store::delete_session` deletes the `sessions` row outright, and
-- `conversations` — the only other table holding a `claude_session_id` — is
-- `REFERENCES sessions(id) ON DELETE CASCADE` (037), so a reaped row takes
-- its conversation ids with it.
--
-- Hence a table that outlives the session, the shape `work_unlinks`
-- (migration 070) established for "a fact that must outlive the row it was
-- about":
--
--   * no foreign key to `sessions` — the row must survive its session, so
--     nothing may cascade it away;
--   * no foreign key to `people` either, the 066 rationale above.
--
-- It is NOT a second source of truth for who owns a LIVE session: that is
-- `sessions.owner_person_id`. This answers only "who did this conversation
-- belong to", for a row that no longer exists.
--
-- Nothing in M1 deletes from this table. It is one row per conversation, so
-- it grows with conversations and not with traffic — but "durable" cannot
-- mean "never pruned" without someone deciding so, and nobody has:
-- retention (a person is deleted; an owner wants the record forgotten)
-- belongs with `store/work_retention.rs`'s windows and is M2's.
CREATE TABLE IF NOT EXISTS conversation_owners (
  claude_session_id TEXT PRIMARY KEY,
  owner_person_id   INTEGER NOT NULL,
  first_seen_at     INTEGER NOT NULL
);

-- Filled from TRIGGERS on `sessions`, not from a Rust call site, and for
-- the reason 045 gives for `trg_participant_on_session_insert`:
-- `claude_session_id` is written by `store/reconcile.rs`'s upsert AND by
-- `store/sessions.rs::set_claude_session_id` (and `upsert_bg_session`
-- writes it at INSERT time), so a call at each site is a call a future
-- writer can skip. These cannot be skipped.
--
-- `INSERT OR IGNORE`: the FIRST writer wins. A later re-attribution of the
-- session — a claim, a move, an operator's correction — must not rewrite
-- who the conversation originally belonged to, which is the whole question
-- the resume gate asks.
--
-- Both triggers name only `claude_session_id` (there since 014) and
-- `owner_person_id` (added above) plus the table above, so they compile
-- under the same rules as the row-version trigger below.
CREATE TRIGGER IF NOT EXISTS trg_conversation_owner_on_session_insert
AFTER INSERT ON sessions
WHEN NEW.claude_session_id IS NOT NULL AND NEW.owner_person_id IS NOT NULL
BEGIN
  INSERT OR IGNORE INTO conversation_owners
    (claude_session_id, owner_person_id, first_seen_at)
  VALUES (NEW.claude_session_id, NEW.owner_person_id,
          CAST(strftime('%s', 'now') AS INTEGER));
END;

CREATE TRIGGER IF NOT EXISTS trg_conversation_owner_on_session_update
AFTER UPDATE OF claude_session_id, owner_person_id ON sessions
WHEN NEW.claude_session_id IS NOT NULL AND NEW.owner_person_id IS NOT NULL
BEGIN
  INSERT OR IGNORE INTO conversation_owners
    (claude_session_id, owner_person_id, first_seen_at)
  VALUES (NEW.claude_session_id, NEW.owner_person_id,
          CAST(strftime('%s', 'now') AS INTEGER));
END;

-- `owner_person_id` and `visibility` are both `SessionRow` fields, so
-- 065's `sessions_row_version_bump` (rebuilt by 082) is rebuilt once more
-- to watch them: same trigger as 082 plus the two lines. SQLite has no
-- ALTER TRIGGER, so the whole body is re-issued — and
-- `store/schema.rs::the_sessions_columns_are_the_ones_the_row_version_trigger_knows`
-- fails for any `sessions` column the trigger neither watches nor lists in
-- its `ROW_VERSION_UNWATCHED`.
DROP TRIGGER IF EXISTS sessions_row_version_bump;

CREATE TRIGGER sessions_row_version_bump
AFTER UPDATE ON sessions
FOR EACH ROW
WHEN NEW.row_version = OLD.row_version
 AND (
      NEW.id IS NOT OLD.id
   OR NEW.tmux_name IS NOT OLD.tmux_name
   OR NEW.host_alias IS NOT OLD.host_alias
   OR NEW.project_id IS NOT OLD.project_id
   OR NEW.worktree_id IS NOT OLD.worktree_id
   OR NEW.created_at IS NOT OLD.created_at
   OR NEW.last_activity_at IS NOT OLD.last_activity_at
   OR NEW.status IS NOT OLD.status
   OR NEW.notes IS NOT OLD.notes
   OR NEW.account_uuid IS NOT OLD.account_uuid
   OR NEW.kind IS NOT OLD.kind
   OR NEW.reviews_session_id IS NOT OLD.reviews_session_id
   OR NEW.worktree_key IS NOT OLD.worktree_key
   OR NEW.lost_at IS NOT OLD.lost_at
   OR NEW.claude_session_id IS NOT OLD.claude_session_id
   OR NEW.claude_status IS NOT OLD.claude_status
   OR NEW.effort_level IS NOT OLD.effort_level
   OR NEW.pr_url IS NOT OLD.pr_url
   OR NEW.current_activity IS NOT OLD.current_activity
   OR NEW.context_pct IS NOT OLD.context_pct
   OR NEW.stuck_kind IS NOT OLD.stuck_kind
   OR NEW.friendly_name IS NOT OLD.friendly_name
   OR NEW.safe_kill_state IS NOT OLD.safe_kill_state
   OR NEW.safe_kill_nonce IS NOT OLD.safe_kill_nonce
   OR NEW.safe_kill_detail IS NOT OLD.safe_kill_detail
   OR NEW.safe_kill_requested_at IS NOT OLD.safe_kill_requested_at
   OR NEW.idle_since IS NOT OLD.idle_since
   OR NEW.stuck_since IS NOT OLD.stuck_since
   OR NEW.last_playbook_at IS NOT OLD.last_playbook_at
   OR NEW.last_prompt IS NOT OLD.last_prompt
   OR NEW.started_at IS NOT OLD.started_at
   OR NEW.last_turn_at IS NOT OLD.last_turn_at
   OR NEW.ci_status IS NOT OLD.ci_status
   OR NEW.turn_seq IS NOT OLD.turn_seq
   OR NEW.last_stop_at IS NOT OLD.last_stop_at
   OR NEW.parent_session_id IS NOT OLD.parent_session_id
   OR NEW.tags IS NOT OLD.tags
   OR NEW.transcript_path IS NOT OLD.transcript_path
   OR NEW.last_hook_at IS NOT OLD.last_hook_at
   OR NEW.repair_backoff_sig IS NOT OLD.repair_backoff_sig
   OR NEW.repair_backoff_at IS NOT OLD.repair_backoff_at
   OR NEW.usage_input_tokens IS NOT OLD.usage_input_tokens
   OR NEW.usage_output_tokens IS NOT OLD.usage_output_tokens
   OR NEW.usage_cache_write_tokens IS NOT OLD.usage_cache_write_tokens
   OR NEW.usage_cache_read_tokens IS NOT OLD.usage_cache_read_tokens
   OR NEW.usage_cost_micros IS NOT OLD.usage_cost_micros
   OR NEW.usage_model IS NOT OLD.usage_model
   OR NEW.usage_offset_bytes IS NOT OLD.usage_offset_bytes
   OR NEW.usage_updated_at IS NOT OLD.usage_updated_at
   OR NEW.usage_source IS NOT OLD.usage_source
   OR NEW.usage_last_msg_id IS NOT OLD.usage_last_msg_id
   OR NEW.usage_last_msg_usage IS NOT OLD.usage_last_msg_usage
   OR NEW.lost_reason IS NOT OLD.lost_reason
   OR NEW.tmux_pane_id IS NOT OLD.tmux_pane_id
   OR NEW.awaiting_rebind_at IS NOT OLD.awaiting_rebind_at
   OR NEW.context_tokens IS NOT OLD.context_tokens
   OR NEW.context_window IS NOT OLD.context_window
   OR NEW.context_source IS NOT OLD.context_source
   OR NEW.context_at IS NOT OLD.context_at
   OR NEW.context_stale IS NOT OLD.context_stale
   OR NEW.model IS NOT OLD.model
   OR NEW.pending_input IS NOT OLD.pending_input
   OR NEW.prompt_submit_seq IS NOT OLD.prompt_submit_seq
   OR NEW.stop_block_streak IS NOT OLD.stop_block_streak
   OR NEW.current_branch IS NOT OLD.current_branch
   OR NEW.current_branch_at IS NOT OLD.current_branch_at
   OR NEW.pr_signals IS NOT OLD.pr_signals
   OR NEW.pr_signals_at IS NOT OLD.pr_signals_at
   OR NEW.last_touch_at IS NOT OLD.last_touch_at
   OR NEW.stale_working_at IS NOT OLD.stale_working_at
   OR NEW.pr_evidence IS NOT OLD.pr_evidence
   OR NEW.pr_checked_at IS NOT OLD.pr_checked_at
   OR NEW.owner_person_id IS NOT OLD.owner_person_id
   OR NEW.visibility IS NOT OLD.visibility
 )
BEGIN
  UPDATE sessions SET row_version = OLD.row_version + 1 WHERE id = NEW.id;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (87);
