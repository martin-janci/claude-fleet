-- Identity sub-project 1 (spec §3): Keycloak-style admin events, and the
-- auth-epoch triggers person_identities never had.
--
-- admin_events is append-only: UPDATE always aborts; DELETE aborts unless
-- the retention sweep has opened `admin_events_gc` inside its own
-- transaction (Store::sweep_admin_events), so nothing else can rewrite
-- history through the store.
CREATE TABLE IF NOT EXISTS admin_events (
  id              INTEGER PRIMARY KEY,
  ts              INTEGER NOT NULL,
  actor_person_id INTEGER,
  actor_device_id INTEGER,
  action          TEXT NOT NULL,
  resource_type   TEXT NOT NULL,
  resource_id     TEXT NOT NULL,
  representation  TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_admin_events_ts ON admin_events(ts);
CREATE INDEX IF NOT EXISTS idx_admin_events_resource ON admin_events(resource_type, resource_id);

CREATE TABLE IF NOT EXISTS admin_events_gc (id INTEGER PRIMARY KEY CHECK (id = 1), open INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO admin_events_gc (id, open) VALUES (1, 0);

CREATE TRIGGER IF NOT EXISTS admin_events_no_update BEFORE UPDATE ON admin_events
BEGIN SELECT RAISE(ABORT, 'admin_events is append-only'); END;
CREATE TRIGGER IF NOT EXISTS admin_events_no_delete BEFORE DELETE ON admin_events
WHEN (SELECT open FROM admin_events_gc WHERE id = 1) = 0
BEGIN SELECT RAISE(ABORT, 'admin_events is append-only'); END;

-- An identity link decides which person a sign-in pairs a device as, so a
-- write to it re-resolves every cached caller (the migration-107 shape).
-- The update trigger names the columns that change what a link resolves
-- to: `last_login_at`, stamped on every sign-in, does not churn the cache.
CREATE TRIGGER IF NOT EXISTS auth_epoch_person_identities_insert
  AFTER INSERT ON person_identities
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

CREATE TRIGGER IF NOT EXISTS auth_epoch_person_identities_update
  AFTER UPDATE OF issuer, subject, person_id ON person_identities
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

CREATE TRIGGER IF NOT EXISTS auth_epoch_person_identities_delete
  AFTER DELETE ON person_identities
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (182);
