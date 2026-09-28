-- Declarative pages P5 (design §5, D-P4): an agent's settings change is a
-- proposal a person reviews, and every write of a registered setting leaves
-- a record of who made it.
--
-- setting_proposals: one proposed value for one registered key. `value` is
-- the stored (normalised) form, `before` the effective value when it was
-- proposed. A newer proposal for the same key supersedes a pending one.
-- A person applies or rejects it (`decided_by`); nothing here is ever
-- written to `settings` except through `service::settings::set_by`.
--
-- state   'pending' | 'applied' | 'rejected' | 'superseded'
-- source  'agent' (the control API) | 'person' | 'system'
CREATE TABLE IF NOT EXISTS setting_proposals (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  at            INTEGER NOT NULL,
  key           TEXT    NOT NULL,
  value         TEXT    NOT NULL,
  before        TEXT    NOT NULL,
  why           TEXT,
  source        TEXT    NOT NULL,
  source_detail TEXT,
  state         TEXT    NOT NULL DEFAULT 'pending'
                CHECK (state IN ('pending', 'applied', 'rejected', 'superseded')),
  decided_at    INTEGER,
  decided_by    TEXT
);
CREATE INDEX IF NOT EXISTS idx_setting_proposals_pending
  ON setting_proposals(state, key);

-- setting_audit: every write of a registered setting, newest last. Settings
-- hold no secrets (a secret is never registered), so before / after are the
-- values themselves. `before` is NULL when the key was unset (its default).
CREATE TABLE IF NOT EXISTS setting_audit (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  at            INTEGER NOT NULL,
  key           TEXT    NOT NULL,
  before        TEXT,
  after         TEXT    NOT NULL,
  actor         TEXT    NOT NULL,
  actor_detail  TEXT,
  proposal_id   INTEGER
);
CREATE INDEX IF NOT EXISTS idx_setting_audit_key ON setting_audit(key, id);

INSERT OR IGNORE INTO schema_version (version) VALUES (72);
