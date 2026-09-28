-- A paired client the operator lets manage the asset catalog from its own
-- window (`fleet-hub client grant <name> assets`): the hub's `catalog_admin`
-- tool answers it as it answers the master. NULL (the default) is every
-- client paired before this and every fresh pairing: fleet admin stays the
-- master's unless the operator says otherwise, per client.
--
-- `catalog_admin` reads the column live on every call, so no cached caller
-- carries it; the epoch trigger below is kept anyway, so the rule of
-- migration 060 (every column but `last_seen_at` bumps the epoch) holds for
-- the whole table.
ALTER TABLE client_tokens ADD COLUMN assets_admin_at INTEGER;

CREATE TRIGGER IF NOT EXISTS auth_epoch_client_tokens_assets_admin
  AFTER UPDATE OF assets_admin_at ON client_tokens
  WHEN OLD.assets_admin_at IS NOT NEW.assets_admin_at
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (72);
