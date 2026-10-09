-- 137: the indexes behind `runs { list }` (Orbit Fleet redesign step 8.3,
-- docs/ux/2026-10-08-orbit-fleet-redesign/transition-plan.md): one
-- newest-first list over `tasks`, `orchestration_events`, `decision_runs`,
-- `aux_usage` and `routine_runs` (`store::runs`). Indexes only; no table
-- changes.
--
-- Already there and reused: `decision_runs_at` (069), `idx_aux_usage_at` and
-- `idx_aux_usage_mission` (127), `idx_orch_events` (mission, at) (115),
-- `idx_tasks_worker` / `idx_tasks_requester` (020), `idx_tasks_item` (110),
-- `routine_runs_by_routine` (131).
--
-- idx_tasks_created            a task run starts at its dispatch: the
--                              time-ordered scan and since/until.
-- idx_orch_events_kind_at      only the mission's actions and brakes are
--                              runs (`step`, `refused`, `budget`,
--                              `no_progress`), not its audit rows.
-- decision_runs_org_at         the org filter on Jev runs.
-- decision_runs_subject        a Jev run about one session.
-- idx_aux_usage_org_at         the org filter on `claude -p` runs.
-- idx_aux_usage_conversation   a summary run's session, through the
--                              conversation it read.
-- idx_sessions_claude_session  the same link from the other end: the
--                              session that holds a summary's conversation.
-- routine_runs_started         a routine fire's time-ordered scan
--                              (`routine_runs_by_routine` leads with the
--                              routine).
-- routine_runs_session         the session a fire started.
--
-- New objects only, `IF NOT EXISTS`, safe to re-run.
CREATE INDEX IF NOT EXISTS idx_tasks_created ON tasks(created_at);
CREATE INDEX IF NOT EXISTS idx_orch_events_kind_at ON orchestration_events(kind, at);
CREATE INDEX IF NOT EXISTS decision_runs_org_at ON decision_runs(org_id, at);
CREATE INDEX IF NOT EXISTS decision_runs_subject ON decision_runs(subject_kind, subject_id);
CREATE INDEX IF NOT EXISTS idx_aux_usage_org_at ON aux_usage(org_id, at);
CREATE INDEX IF NOT EXISTS idx_aux_usage_conversation
  ON aux_usage(claude_session_id) WHERE claude_session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sessions_claude_session
  ON sessions(claude_session_id) WHERE claude_session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS routine_runs_started ON routine_runs(started_at);
CREATE INDEX IF NOT EXISTS routine_runs_session
  ON routine_runs(session_id) WHERE session_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (137);
