-- Work graph M4.6 (docs/superpowers/plans/2026-09-24-work-graph-m4-detection.md):
-- the opt-in classification nudge fires at most once per conversation.
--
-- conversations.classify_nudged_at
--                when the nudge was handed to this conversation (unix
--                seconds); NULL while it has not been. Written only by the
--                UserPromptSubmit delivery that carried it.
ALTER TABLE conversations ADD COLUMN classify_nudged_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (56);
