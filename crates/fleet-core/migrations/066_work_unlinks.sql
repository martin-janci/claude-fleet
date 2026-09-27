-- Work graph label hygiene (D34): a person's "Clear work" holds against the
-- unchanged state signal that made the link (resolver rule R9u).
--
-- `work_link { action: unlink }` deletes a live link. When the link's
-- target is what the session's CURRENT state says — its branch, its pull
-- request's head branch or a closing reference — the next resolver run
-- would make the same link again from the same signal (R3 confirmed in a
-- trusted project, R3b a suggestion otherwise), silently undoing the
-- person's correction. A PERSON's unlink therefore writes one row per such
-- signal here; detection (`service::work::detect`) drops a state candidate
-- whose (participant, target, signal, value) matches a row. A different
-- value (another branch, another pull request) is not suppressed; the same
-- value again is. Event candidates (prompts, URLs, the PR's text, trailers)
-- are never suppressed, and an agent's unlink writes nothing.
--
-- signal  'branch': value is a branch name — the session's branch, or the
--                   pull request's head branch;
--         'pr':     value is the pull request (its URL, else `head:<branch>`),
--                   for its closing references.
--
-- Anchored on the participant like the links, so a move carries it and a
-- fork does not (a fork copies confirmed links only, never a rejection).
-- Rows go when the participant retires (the trigger) or is deleted
-- (cascade), and with the item.
CREATE TABLE IF NOT EXISTS work_unlinks (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  participant_id INTEGER NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
  item_id        INTEGER REFERENCES work_items(id) ON DELETE CASCADE,
  ref_key        TEXT,
  signal         TEXT    NOT NULL CHECK (signal IN ('branch', 'pr')),
  value          TEXT    NOT NULL,
  at             INTEGER NOT NULL,
  CHECK (item_id IS NOT NULL OR ref_key IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_work_unlinks_participant ON work_unlinks(participant_id);

CREATE TRIGGER IF NOT EXISTS trg_work_unlinks_drop_on_retire
AFTER UPDATE OF retired_at ON participants
WHEN OLD.retired_at IS NULL AND NEW.retired_at IS NOT NULL
BEGIN
  DELETE FROM work_unlinks WHERE participant_id = OLD.id;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (66);
