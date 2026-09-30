-- `work_unlinks.item_id` cascades from `work_items` (070) but had no index,
-- so deleting a work item scanned the table. Partial: most rows name a
-- ref key, not an item.
CREATE INDEX IF NOT EXISTS idx_work_unlinks_item ON work_unlinks(item_id) WHERE item_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (85);
