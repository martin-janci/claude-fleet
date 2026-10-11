-- Tracker comments, posted back (owner decision 2026-10-11, superseding the
-- "no comment" half of D29): a comment written in fleet may also be posted
-- to the task's tracker ticket, through the write-back outbox (migration
-- 061) as op `comment`.
--
-- A comment write's `url` is `fleet-comment:<work_item_comments.id>` (the
-- unique key stays meaningful: one post per fleet comment per tracker), its
-- `item_key` the ticket's key and its `title` empty.
--   body         the text posted: the comment, a blank line, and fleet's
--                signature. NULL for every other op.
--   external_id  the item's tracker id (a GitHub node id, a Linear uuid, an
--                Asana gid) for a provider that addresses an item by it.
--   remote_id    the id the tracker gave what was written (its comment id),
--                once sent: what hides that comment from the tracker's own
--                list while the fleet comment it came from is live.
--   remote_url   where the tracker shows it, when it says.
ALTER TABLE tracker_writes ADD COLUMN body TEXT;
ALTER TABLE tracker_writes ADD COLUMN external_id TEXT;
ALTER TABLE tracker_writes ADD COLUMN remote_id TEXT;
ALTER TABLE tracker_writes ADD COLUMN remote_url TEXT;
CREATE INDEX IF NOT EXISTS idx_tracker_writes_comment
  ON tracker_writes(url) WHERE op = 'comment';

INSERT OR IGNORE INTO schema_version (version) VALUES (170);
