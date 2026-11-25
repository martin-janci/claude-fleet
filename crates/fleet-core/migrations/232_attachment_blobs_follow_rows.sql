-- A work item's attachment bytes live in `work_attachment_blobs`, keyed by
-- content hash with no foreign key. Only `delete_attachment` removed one;
-- the done-ticket retention sweep deletes `work_items`, whose attachments
-- CASCADE away, and every blob they named stayed in state.db (up to 16 MiB
-- each, copied into every daily backup) with nothing able to reach it.
--
-- A trigger, so every path that deletes an attachment row (the cascade
-- included) takes its blob when no live attachment still names it.
CREATE TRIGGER IF NOT EXISTS trg_attachment_blob_on_row_delete
AFTER DELETE ON work_item_attachments
BEGIN
  DELETE FROM work_attachment_blobs
   WHERE sha256 = old.sha256
     AND NOT EXISTS (SELECT 1 FROM work_item_attachments
                      WHERE sha256 = old.sha256 AND deleted_at IS NULL);
END;

-- The blobs earlier sweeps already orphaned, once.
DELETE FROM work_attachment_blobs
 WHERE sha256 NOT IN (SELECT sha256 FROM work_item_attachments WHERE deleted_at IS NULL);

INSERT OR IGNORE INTO schema_version (version) VALUES (232);
