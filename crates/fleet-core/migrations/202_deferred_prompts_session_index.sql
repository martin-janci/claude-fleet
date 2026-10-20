-- Every prompt typed into a session now leaves a `deferred_prompts` row
-- (migration 133), queued or not: the ledger of one prompt per idle moment
-- (`service::sessions::deferred`). The only index so far,
-- `deferred_prompts_pending` (155), is partial on pending rows, so the
-- idle-moment check (`delivered_at >= ?` for the session), the per-session
-- listing and the ON DELETE CASCADE from `sessions` each scanned the table's
-- whole history. This one covers them.
CREATE INDEX IF NOT EXISTS deferred_prompts_session
  ON deferred_prompts (session_id, delivered_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (202);
