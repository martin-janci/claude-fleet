-- A wizard someone left half-way, kept on the hub so it resumes on another
-- device (Orbit Fleet M15 step G7.2): start Add a project on the phone,
-- finish it on the desktop. Generalised from the add-host wizard's
-- `host_setups` (migration 135), whose rows move here as kind `add_host`
-- and whose table goes. The rules are in `service::wizard_state`.
--
-- wizard_state
--   kind        which wizard: add_host | add_project | add_account |
--               new_session | link_peer | form
--   key         which one of that kind: the SSH alias an add-host wizard
--               adds, '' for a wizard a person runs one of at a time
--   person_id   whose it is (the caller's person); NULL = fleet's own (a
--               standalone desktop, a single-person hub)
--   label       what the resume line names ("mercury", "acme/api")
--   step        the step it was left on, from 1
--   answers     what the person picked, JSON object; never a secret
--   checks      the add-host wizard's last live checks, JSON list
--   device      the device that saved it last ("Martin's Pixel"); NULL =
--               the hub's own desktop
CREATE TABLE IF NOT EXISTS wizard_state (
  kind        TEXT    NOT NULL,
  key         TEXT    NOT NULL DEFAULT '',
  person_id   INTEGER,
  label       TEXT,
  step        INTEGER NOT NULL DEFAULT 1,
  answers     TEXT    NOT NULL DEFAULT '{}',
  checks      TEXT    NOT NULL DEFAULT '[]',
  device      TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS wizard_state_one
  ON wizard_state (kind, key, COALESCE(person_id, 0));

INSERT OR IGNORE INTO wizard_state
  (kind, key, person_id, label, step, answers, checks, device, created_at, updated_at)
  SELECT 'add_host', ssh_alias, NULL, alias, step, answers, checks, NULL, created_at, updated_at
  FROM host_setups;

DROP TABLE IF EXISTS host_setups;

INSERT OR IGNORE INTO schema_version (version) VALUES (162);
