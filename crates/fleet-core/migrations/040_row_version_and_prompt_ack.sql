-- `row_version`: a per-row counter bumped by every UPDATE, so a payload the
-- frontend receives (a command's return value, a row event) can be ordered
-- against the row it already holds. `last_activity_at` cannot do that job:
-- only reconcile writes it, so two writes inside one tick compare equal and
-- whichever arrives last wins. The trigger fires for `INSERT … ON CONFLICT DO
-- UPDATE` too (an upsert's update arm is an UPDATE). Recursive triggers are
-- off by default, and the WHEN clause keeps it a no-op even if they were on.
ALTER TABLE sessions ADD COLUMN row_version INTEGER NOT NULL DEFAULT 0;

CREATE TRIGGER IF NOT EXISTS sessions_row_version_bump
AFTER UPDATE ON sessions
FOR EACH ROW
WHEN NEW.row_version = OLD.row_version
BEGIN
  UPDATE sessions SET row_version = OLD.row_version + 1 WHERE id = NEW.id;
END;

-- `prompt_submit_seq`: how many UserPromptSubmit hooks this row has recorded.
-- A send that wants to know "did the REPL take it?" reads it before the
-- send and waits for it to move. Monotonic, so second-granularity timestamps
-- cannot confuse a hook that lands in the same second as the send. Not on
-- the wire: read through `Store::prompt_ack_state`.
ALTER TABLE sessions ADD COLUMN prompt_submit_seq INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (40);
