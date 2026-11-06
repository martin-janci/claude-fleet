-- Routine seen items: what a routine's runs already handled, so the next
-- run skips what has not changed since and stays quiet when nothing did.
-- The rules are in `service::routines::seen`.
--
--   routine_seen  one row per item a run of the routine reported in its
--                 reply's `<routine-seen>` block: `item` its stable name
--                 (`pr owner/repo#12`), `state` what the run saw (`head=abc1234
--                 comments=3 checks=pass`), `seen_at` the last run that
--                 reported it, `changed_at` when its state last differed.
--                 `run_id` is the run that reported it last; no foreign key,
--                 so a swept run leaves the item as it is. A row not
--                 reported for 30 days is dropped; a routine keeps at most
--                 500 (the oldest-seen go first).
CREATE TABLE IF NOT EXISTS routine_seen (
  routine_id    INTEGER NOT NULL,
  item          TEXT NOT NULL,
  state         TEXT NOT NULL DEFAULT '',
  first_seen_at INTEGER NOT NULL,
  seen_at       INTEGER NOT NULL,
  changed_at    INTEGER NOT NULL,
  run_id        INTEGER,
  PRIMARY KEY (routine_id, item)
);

CREATE INDEX IF NOT EXISTS routine_seen_by_age ON routine_seen (routine_id, seen_at);

-- routines
--   track_seen  1: its runs are shown what earlier runs handled and report
--               what they handled (the default, every routine included);
--               0: a person turned it off.
ALTER TABLE routines ADD COLUMN track_seen INTEGER NOT NULL DEFAULT 1;

-- Its runs' replies are now read for the seen block too: a run that
-- finished unread stays unread while its routine tracks seen items
-- (`Store::finish_routine_run`). The runs already finished stay as they
-- are: no earlier run was told to write one.

INSERT OR IGNORE INTO schema_version (version) VALUES (212);
