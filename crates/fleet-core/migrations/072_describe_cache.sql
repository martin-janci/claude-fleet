-- Work graph, describe cache: ONE item's whole description, fetched on
-- demand by `work { action: describe }` and served while it is younger than
-- `work.describe_cache_secs`.
--
-- Its own table, never a `work_items` column, and never in `meta`: the
-- 2000-char excerpt a sync writes stays the only description the row itself
-- carries, so an excerpt and a full copy can never disagree about which is
-- authoritative — each read path has exactly one source. Nothing here
-- reaches an event frame, a session row or the phone projection, so it costs
-- no replay-ring pressure (the reason `DESCRIPTION_MAX_CHARS` stays 2000).
--
-- Third-party text at rest, so every way the text can go is covered: the
-- work retention pass sweeps it by age (`RetentionTable::Descriptions`, with
-- a floor), this FK takes it when an item goes, `Store::remove_tracker`
-- clears it for a disconnected tracker's items (which are KEPT, marked
-- unavailable, so no cascade reaches them), and a sync that changes an item's
-- description drops the row it just made stale.
CREATE TABLE IF NOT EXISTS work_item_descriptions (
  item_id    INTEGER PRIMARY KEY REFERENCES work_items(id) ON DELETE CASCADE,
  body       TEXT    NOT NULL,
  chars      INTEGER NOT NULL,
  fetched_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_work_item_descriptions_fetched
  ON work_item_descriptions(fetched_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (72);
