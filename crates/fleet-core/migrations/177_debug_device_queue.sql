-- The line waiting for a claimed debug device (docs/debug-devices.md,
-- *Queue*): `claim { queue: true }` on a device someone else holds joins it.
-- When the claim is released or lapses, the first in line gets the claim.
-- holder     who waits, as a claim records it (`host:<alias>#<session>`, ...)
-- note       what the claim it gets will say it is for
-- queued_at  when it joined; first come, first served, and an entry older
--            than a day leaves the line
CREATE TABLE IF NOT EXISTS debug_device_queue (
  device_id INTEGER NOT NULL REFERENCES debug_devices (id) ON DELETE CASCADE,
  holder    TEXT    NOT NULL,
  note      TEXT,
  queued_at INTEGER NOT NULL,
  PRIMARY KEY (device_id, holder)
);

-- A forgotten device takes its line with it (also without foreign keys on).
CREATE TRIGGER IF NOT EXISTS debug_device_queue_device_deleted AFTER DELETE ON debug_devices BEGIN
  DELETE FROM debug_device_queue WHERE device_id = OLD.id;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (177);
