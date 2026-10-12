-- Per-org update policy (update-channel design §7.3/§9, slice S9). One row
-- per (org, component) overrides the fleet's `update.*` settings for the
-- targets of that org: a paired client bound to it (`client_tokens.org_id`)
-- and an agent host in it (`hosts.org_id`). A NULL column keeps the fleet's
-- value.
--
-- mode           manual | notify | automatic (a phone: manual | notify)
-- minimum        the org's policy floor: below it → update_required
-- update_window  HH:MM-HH:MM in UTC for automatic installs; '' = any time
-- pin_version    the org's desired version; a target's own pin still wins,
--                and it wins over the fleet-wide component pin
-- pin_mandatory  the pin is required, not merely offered
CREATE TABLE IF NOT EXISTS update_org_policy (
  org_id         INTEGER NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
  component      TEXT    NOT NULL,
  mode           TEXT,
  minimum        TEXT,
  update_window  TEXT,
  pin_version    TEXT,
  pin_mandatory  INTEGER NOT NULL DEFAULT 0,
  reason         TEXT,
  set_by         TEXT    NOT NULL,
  set_at         INTEGER NOT NULL,
  PRIMARY KEY (org_id, component)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (152);
