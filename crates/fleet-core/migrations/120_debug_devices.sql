-- Debug devices (docs/debug-devices.md): Android phones and emulators, iOS
-- simulators and devices attached to a host, inventoried by a scan of that
-- host and used from any session the device is visible to.
-- dev_key   stable per host: the adb serial of a phone, `avd:<name>` of an
--           emulator, the UDID of a simulator or an iOS device
-- serial    what the host's tools address it by now; NULL while a stopped
--           emulator has none
-- state     online | offline | unauthorized | booted | shutdown | missing
-- shared    1: sessions on other hosts (of the same org) may use it too
-- claimed_* an advisory lease: while it holds, only its holder uses the
--           device (a person may release it)
CREATE TABLE IF NOT EXISTS debug_devices (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  host_alias    TEXT    NOT NULL,
  dev_key       TEXT    NOT NULL,
  platform      TEXT    NOT NULL CHECK (platform IN ('android', 'ios')),
  kind          TEXT    NOT NULL CHECK (kind IN ('physical', 'emulator', 'simulator')),
  serial        TEXT,
  name          TEXT    NOT NULL,
  model         TEXT,
  os_version    TEXT,
  state         TEXT    NOT NULL,
  label         TEXT,
  shared        INTEGER NOT NULL DEFAULT 0,
  claimed_by    TEXT,
  claim_note    TEXT,
  claimed_until INTEGER,
  first_seen_at INTEGER NOT NULL,
  last_seen_at  INTEGER NOT NULL,
  UNIQUE (host_alias, dev_key)
);

-- When each host was last scanned, so a list rescans only stale hosts.
CREATE TABLE IF NOT EXISTS debug_device_scans (
  host_alias TEXT PRIMARY KEY,
  scanned_at INTEGER NOT NULL,
  error      TEXT
);

-- A removed host takes its devices and its scan record with it.
CREATE TRIGGER IF NOT EXISTS debug_devices_host_deleted AFTER DELETE ON hosts BEGIN
  DELETE FROM debug_devices WHERE host_alias = OLD.alias;
  DELETE FROM debug_device_scans WHERE host_alias = OLD.alias;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (120);
