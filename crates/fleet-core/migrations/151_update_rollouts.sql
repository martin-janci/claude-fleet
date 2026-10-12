-- Staged rollouts (update-channel design §7.4, slice S9). One active
-- rollout per component at most: `ended_at` NULL. A target is in wave `w`
-- iff `fleet_update::in_cohort(target, version, waves[w])`, so a cohort is
-- stable per release and differs across releases.
--
-- waves              JSON array of cumulative percents, ending at 100 ([5,25,100])
-- wave               index into waves of the wave now open
-- wave_started_at    unix secs the current wave opened; it advances after
--                    `update.rollout_wave_secs` when its failure ratio is
--                    below halt_failure_ratio, and pauses itself otherwise
-- paused_at          NULL while running; paused_reason says why (an
--                    operator, or the halt rule with its ratio)
-- outcome            NULL while active; 'completed' | 'aborted'
CREATE TABLE IF NOT EXISTS update_rollouts (
  id                 INTEGER PRIMARY KEY,
  component          TEXT    NOT NULL,
  version            TEXT    NOT NULL,
  waves              TEXT    NOT NULL DEFAULT '[100]',
  wave               INTEGER NOT NULL DEFAULT 0,
  wave_started_at    INTEGER NOT NULL,
  paused_at          INTEGER,
  paused_reason      TEXT,
  halt_failure_ratio REAL    NOT NULL DEFAULT 0.2,
  created_at         INTEGER NOT NULL,
  ended_at           INTEGER,
  outcome            TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_update_rollouts_active
  ON update_rollouts(component) WHERE ended_at IS NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (151);
