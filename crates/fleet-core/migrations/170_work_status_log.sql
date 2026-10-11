-- Status history for persona dashboards (design 2026-10-11, step 3): one row
-- each time an item's stored status category changes, and one when it is
-- created. `work_items.status_changed_at` keeps only the last change, so a
-- burn-up, a cycle time or "what moved this week" cannot be rebuilt from it;
-- this table starts collecting the day it ships, which is why it lands before
-- the dashboards that read it.
--
-- Triggers write it, so no call site has to remember (the same reasoning as
-- `trg_work_journal_conversation_closed`, 047). `at` is the tracker's own
-- change time when the write carried a new one, else the time of the write.
-- `set_by` copies `status_set_by` (NULL: a sync or the default; `person`;
-- `derived`). Rows go with their item (`ON DELETE CASCADE`), so retention's
-- sweep of done tracker items takes their history too; nothing else grows it
-- but status changes.
CREATE TABLE IF NOT EXISTS work_status_log (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id      INTEGER NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  at           INTEGER NOT NULL,
  category     TEXT    NOT NULL,
  status_name  TEXT,
  set_by       TEXT
);
CREATE INDEX IF NOT EXISTS idx_work_status_log_item ON work_status_log(item_id, at);
CREATE INDEX IF NOT EXISTS idx_work_status_log_at ON work_status_log(at);

CREATE TRIGGER IF NOT EXISTS trg_work_status_log_insert
AFTER INSERT ON work_items
BEGIN
  INSERT INTO work_status_log(item_id, at, category, status_name, set_by)
  VALUES (NEW.id, COALESCE(NEW.status_changed_at, NEW.created_at, CAST(strftime('%s', 'now') AS INTEGER)),
          NEW.status_category, NEW.status_name, NEW.status_set_by);
END;

CREATE TRIGGER IF NOT EXISTS trg_work_status_log_update
AFTER UPDATE OF status_category ON work_items
WHEN NEW.status_category IS NOT OLD.status_category
BEGIN
  INSERT INTO work_status_log(item_id, at, category, status_name, set_by)
  VALUES (NEW.id,
          CASE WHEN NEW.status_changed_at IS NOT OLD.status_changed_at AND NEW.status_changed_at IS NOT NULL
               THEN NEW.status_changed_at
               ELSE CAST(strftime('%s', 'now') AS INTEGER) END,
          NEW.status_category, NEW.status_name, NEW.status_set_by);
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (170);
