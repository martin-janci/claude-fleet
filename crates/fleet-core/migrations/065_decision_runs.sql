-- Jev evaluation (decisions D35 / D37): the decision record and the
-- decision model's credential. See `docs/decisions.md`.
--
-- decision_runs   one row per decision the envelope (`service::decide`)
--                 was asked for, INCLUDING every fallback (the flag off, an
--                 org without consent, a timeout...), so a shadow comparison
--                 sees coverage. NEVER raw text: ids and vocabulary words,
--                 numbers, and an HMAC fingerprint of the redacted input
--                 keyed by a local secret. Swept by `decide.retention_days`.
CREATE TABLE IF NOT EXISTS decision_runs (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  at               INTEGER NOT NULL,
  feature          TEXT    NOT NULL,
  -- The org the subject belonged to when decided; no FK, so an org's
  -- removal keeps the history's number.
  org_id           INTEGER,
  subject_kind     TEXT    NOT NULL,
  subject_id       TEXT    NOT NULL,
  -- The feature's configured mode when decided (`off` for a refusal while
  -- it was off).
  mode             TEXT    NOT NULL CHECK (mode IN ('off', 'shadow', 'assist')),
  provider         TEXT    NOT NULL,
  model_version    TEXT,
  question_version TEXT    NOT NULL,
  input_fp         TEXT,
  candidates       TEXT    NOT NULL DEFAULT '[]',
  answer           TEXT,
  probabilities    TEXT,
  confidence       REAL,
  fallback         TEXT CHECK (fallback IS NULL OR fallback IN (
                     'not_owner', 'flag_off', 'org_off', 'mode_off', 'no_key',
                     'breaker_open', 'budget', 'timeout', 'http_error',
                     'rate_limited', 'invalid_answer', 'low_confidence')),
  baseline_answer  TEXT,
  -- 1 when a request was sent to the provider (the breaker counts these).
  called           INTEGER NOT NULL DEFAULT 0,
  latency_ms       INTEGER,
  input_tokens     INTEGER NOT NULL DEFAULT 0,
  cost_microusd    INTEGER NOT NULL DEFAULT 0,
  followup         TEXT CHECK (followup IS NULL OR followup IN (
                     'confirmed', 'rejected', 'corrected', 'ignored')),
  followup_at      INTEGER,
  corrected_to     TEXT
);
CREATE INDEX IF NOT EXISTS decision_runs_at ON decision_runs(at);
CREATE INDEX IF NOT EXISTS decision_runs_feature_at ON decision_runs(feature, at);
CREATE INDEX IF NOT EXISTS decision_runs_provider_called
  ON decision_runs(provider, called, id);

-- decision_secrets   `jev_api_key`: the TypeSafe API key (a stored value
--                    and/or an env:/file: reference), read ONLY by
--                    `Store::resolve_decision_credential`; `fp_key`: the
--                    local HMAC key of `decision_runs.input_fp`, generated
--                    once, never sent anywhere, and kept when the API key
--                    is cleared so fingerprints stay comparable.
CREATE TABLE IF NOT EXISTS decision_secrets (
  name           TEXT PRIMARY KEY CHECK (name IN ('jev_api_key', 'fp_key')),
  value          TEXT,
  credential_ref TEXT,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER
);

INSERT OR IGNORE INTO schema_version (version) VALUES (65);
