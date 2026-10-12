-- Identity sub-project 1 (spec §3): device tokens expire. The idle clock of
-- every existing token starts now, so the upgrade signs nobody out.
-- INSERT OR IGNORE: a re-run keeps the first anchor.
INSERT OR IGNORE INTO settings (key, value) VALUES ('auth.device_token_ttl_since', CAST(strftime('%s','now') AS TEXT));
-- The expiry policy is part of what a token resolves to: changing it moves
-- the auth epoch, so the cached rows are re-read with the new policy.
CREATE TRIGGER IF NOT EXISTS auth_epoch_settings_device_token_insert AFTER INSERT ON settings
WHEN NEW.key LIKE 'auth.device_token_%' BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_settings_device_token_update AFTER UPDATE ON settings
WHEN NEW.key LIKE 'auth.device_token_%' BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_settings_device_token_delete AFTER DELETE ON settings
WHEN OLD.key LIKE 'auth.device_token_%' BEGIN UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1; END;
INSERT OR IGNORE INTO schema_version (version) VALUES (182);
