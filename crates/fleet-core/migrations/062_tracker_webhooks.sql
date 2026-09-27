-- Work graph M13.4f (decision D13 / D28): a tracker's webhook secret.
--
-- One row per tracker whose admin turned webhook nudges on
-- (`fleet-hub tracker webhook <id>`). The secret signs the tracker's
-- deliveries (HMAC-SHA256); it is read only by
-- `Store::resolve_tracker_webhook_secret` and never returned by any read
-- path. No row: the tracker's webhook route answers 404.
CREATE TABLE IF NOT EXISTS tracker_webhooks (
  tracker_id INTEGER PRIMARY KEY REFERENCES trackers(id) ON DELETE CASCADE,
  secret     TEXT    NOT NULL,
  created_at INTEGER NOT NULL,
  rotated_at INTEGER
);
INSERT OR IGNORE INTO schema_version (version) VALUES (62);
