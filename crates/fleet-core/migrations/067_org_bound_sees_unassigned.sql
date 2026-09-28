-- Work graph M14.1b, decision D31: whether a paired client bound to an org
-- (066's client_tokens.org_id) also sees unassigned work and sessions (no
-- org), as a host does — 1, the default — or only rows assigned to its org
-- (0). Edited through `work_admin`'s org edit (`update_org`,
-- `bound_sees_unassigned`). Its own migration, like 053's `auto_tidy`: an
-- `orgs` column is re-added when the table is rebuilt.
ALTER TABLE orgs ADD COLUMN bound_sees_unassigned INTEGER NOT NULL DEFAULT 1;

-- The switch changes what the org's bound clients see: flipping it
-- invalidates every cached caller, like a re-binding (066).
CREATE TRIGGER IF NOT EXISTS auth_epoch_orgs_bound_sees_unassigned
  AFTER UPDATE OF bound_sees_unassigned ON orgs
  WHEN OLD.bound_sees_unassigned IS NOT NEW.bound_sees_unassigned
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (67);
