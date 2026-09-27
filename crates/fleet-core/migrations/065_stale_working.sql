-- Live-instance analysis 2026-09-27, lifecycle F2: two `working` rows had
-- had no Stop, no transcript growth and no pane output for ~40 h, and
-- nothing could demote them. The tick now turns such a row `idle` after
-- `reconcile.stale_working_secs`, and this stamp says it did (attention
-- reason `stale_working`). Cleared by the next UserPromptSubmit / Stop /
-- StopFailure / SessionEnd / Notification hook, and by a pane that shows a
-- live turn; the cached `claude agents` status alone does not lift it.
ALTER TABLE sessions ADD COLUMN stale_working_at INTEGER;

-- `stale_working_at` rides the wire row, so migration 063's
-- `sessions_row_version_bump` (which names every column it watches) is
-- rebuilt with it: a change to the stamp is a client-visible change and
-- must bump `row_version`. Same trigger as 063 plus the one line.
DROP TRIGGER IF EXISTS sessions_row_version_bump;

CREATE TRIGGER sessions_row_version_bump
AFTER UPDATE ON sessions
FOR EACH ROW
WHEN NEW.row_version = OLD.row_version
 AND (
      NEW.id IS NOT OLD.id
   OR NEW.tmux_name IS NOT OLD.tmux_name
   OR NEW.host_alias IS NOT OLD.host_alias
   OR NEW.project_id IS NOT OLD.project_id
   OR NEW.worktree_id IS NOT OLD.worktree_id
   OR NEW.created_at IS NOT OLD.created_at
   OR NEW.last_activity_at IS NOT OLD.last_activity_at
   OR NEW.status IS NOT OLD.status
   OR NEW.notes IS NOT OLD.notes
   OR NEW.account_uuid IS NOT OLD.account_uuid
   OR NEW.kind IS NOT OLD.kind
   OR NEW.reviews_session_id IS NOT OLD.reviews_session_id
   OR NEW.worktree_key IS NOT OLD.worktree_key
   OR NEW.lost_at IS NOT OLD.lost_at
   OR NEW.claude_session_id IS NOT OLD.claude_session_id
   OR NEW.claude_status IS NOT OLD.claude_status
   OR NEW.effort_level IS NOT OLD.effort_level
   OR NEW.pr_url IS NOT OLD.pr_url
   OR NEW.current_activity IS NOT OLD.current_activity
   OR NEW.context_pct IS NOT OLD.context_pct
   OR NEW.stuck_kind IS NOT OLD.stuck_kind
   OR NEW.friendly_name IS NOT OLD.friendly_name
   OR NEW.safe_kill_state IS NOT OLD.safe_kill_state
   OR NEW.safe_kill_nonce IS NOT OLD.safe_kill_nonce
   OR NEW.safe_kill_detail IS NOT OLD.safe_kill_detail
   OR NEW.safe_kill_requested_at IS NOT OLD.safe_kill_requested_at
   OR NEW.idle_since IS NOT OLD.idle_since
   OR NEW.stuck_since IS NOT OLD.stuck_since
   OR NEW.last_playbook_at IS NOT OLD.last_playbook_at
   OR NEW.last_prompt IS NOT OLD.last_prompt
   OR NEW.started_at IS NOT OLD.started_at
   OR NEW.last_turn_at IS NOT OLD.last_turn_at
   OR NEW.ci_status IS NOT OLD.ci_status
   OR NEW.turn_seq IS NOT OLD.turn_seq
   OR NEW.last_stop_at IS NOT OLD.last_stop_at
   OR NEW.parent_session_id IS NOT OLD.parent_session_id
   OR NEW.tags IS NOT OLD.tags
   OR NEW.transcript_path IS NOT OLD.transcript_path
   OR NEW.last_hook_at IS NOT OLD.last_hook_at
   OR NEW.repair_backoff_sig IS NOT OLD.repair_backoff_sig
   OR NEW.repair_backoff_at IS NOT OLD.repair_backoff_at
   OR NEW.usage_input_tokens IS NOT OLD.usage_input_tokens
   OR NEW.usage_output_tokens IS NOT OLD.usage_output_tokens
   OR NEW.usage_cache_write_tokens IS NOT OLD.usage_cache_write_tokens
   OR NEW.usage_cache_read_tokens IS NOT OLD.usage_cache_read_tokens
   OR NEW.usage_cost_micros IS NOT OLD.usage_cost_micros
   OR NEW.usage_model IS NOT OLD.usage_model
   OR NEW.usage_offset_bytes IS NOT OLD.usage_offset_bytes
   OR NEW.usage_updated_at IS NOT OLD.usage_updated_at
   OR NEW.usage_source IS NOT OLD.usage_source
   OR NEW.usage_last_msg_id IS NOT OLD.usage_last_msg_id
   OR NEW.usage_last_msg_usage IS NOT OLD.usage_last_msg_usage
   OR NEW.lost_reason IS NOT OLD.lost_reason
   OR NEW.tmux_pane_id IS NOT OLD.tmux_pane_id
   OR NEW.awaiting_rebind_at IS NOT OLD.awaiting_rebind_at
   OR NEW.context_tokens IS NOT OLD.context_tokens
   OR NEW.context_window IS NOT OLD.context_window
   OR NEW.context_source IS NOT OLD.context_source
   OR NEW.context_at IS NOT OLD.context_at
   OR NEW.context_stale IS NOT OLD.context_stale
   OR NEW.model IS NOT OLD.model
   OR NEW.pending_input IS NOT OLD.pending_input
   OR NEW.prompt_submit_seq IS NOT OLD.prompt_submit_seq
   OR NEW.stop_block_streak IS NOT OLD.stop_block_streak
   OR NEW.current_branch IS NOT OLD.current_branch
   OR NEW.current_branch_at IS NOT OLD.current_branch_at
   OR NEW.pr_signals IS NOT OLD.pr_signals
   OR NEW.pr_signals_at IS NOT OLD.pr_signals_at
   OR NEW.last_touch_at IS NOT OLD.last_touch_at
   OR NEW.stale_working_at IS NOT OLD.stale_working_at
 )
BEGIN
  UPDATE sessions SET row_version = OLD.row_version + 1 WHERE id = NEW.id;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (65);
