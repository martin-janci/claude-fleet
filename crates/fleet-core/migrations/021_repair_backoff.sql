-- Automatic workspace repair on the reconcile tick (service::repair_tick).
--
-- repair_backoff_sig  signature of the row (host, project, worktree row/key,
--                     branch, expected directories) at the moment an
--                     automatic repair was refused (E_REPAIR_REQUIRED and the
--                     other non-transient refusals). The tick skips the
--                     session while the signature is unchanged, and clears it
--                     once the directory is present again. NULL = no backoff.
-- repair_backoff_at   unix secs of that refusal.
ALTER TABLE sessions ADD COLUMN repair_backoff_sig TEXT;
ALTER TABLE sessions ADD COLUMN repair_backoff_at INTEGER;

INSERT OR IGNORE INTO schema_version (version) VALUES (21);
