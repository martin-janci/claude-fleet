-- Every session row gets its durable identity the moment it exists.
--
-- Migration 043 introduced `participants` and backfilled one per session,
-- but after that only `insert_message` minted one (lazily, through
-- `ensure_participant_for_session`). A session that never sent or received
-- a message therefore had no identity, and everything anchored on the
-- participant rather than on the reusable `sessions.id` — a move's re-point
-- today, work links next — silently skipped it. `sessions` rows are inserted
-- from three places (reconcile's upsert, `upsert_session`, the bg path), so
-- this is a trigger, not a call at each site: the same reasoning as 044's
-- `trg_read_cursors_on_session_delete`, and a fourth insert site cannot miss
-- it either.
--
-- `WHEN NOT EXISTS`: a live participant already bound to this id (only a
-- pre-existing orphan could be — `sweep_retired_participants` retires those)
-- must not collide with the unique partial index on `session_id`. A reused
-- id is fine: `delete_session` and every reap retire the old participant and
-- clear its `session_id`, so the new row gets a fresh identity rather than
-- inheriting the dead session's mail.
--
-- A move now always finds a participant on its target row as well; the
-- collision merge in `repoint_participant` folds that fresh, empty identity
-- into the source's and retires it, exactly as it did when a message had
-- reached the target first.
CREATE TRIGGER IF NOT EXISTS trg_participant_on_session_insert
AFTER INSERT ON sessions
WHEN NOT EXISTS (SELECT 1 FROM participants WHERE session_id = NEW.id)
BEGIN
  INSERT INTO participants (kind, session_id, created_at)
  VALUES ('session', NEW.id, strftime('%s','now'));
END;

-- Every session created since 043 that never messaged anyone.
INSERT INTO participants (kind, session_id, created_at)
  SELECT 'session', id, strftime('%s','now') FROM sessions
  WHERE id NOT IN (SELECT session_id FROM participants WHERE session_id IS NOT NULL);

INSERT OR IGNORE INTO schema_version (version) VALUES (45);
