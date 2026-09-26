-- Hub store latency, task 7: the hub's `authorize` keeps the bearer-token
-- tables in memory instead of reading them under the writer's lock on every
-- request. `auth_epoch` is what keeps that cache honest: every write that can
-- change what a token resolves to bumps it IN THE WRITING TRANSACTION, and
-- the cache compares it (one single-row read on a read-only connection) on
-- every request, rebuilding when it moved. Triggers live in the database
-- file, so a write from another process — `fleet-hub peer remove`,
-- `host-token-mode`, `agent-token --rotate` open state.db directly — bumps it
-- exactly like one from the daemon: a revoked token is refused on the very
-- next request, whoever revoked it.
--
-- `host_tokens`: every insert, update and delete (all four columns feed the
-- resolved caller or the constant-time match). `client_tokens`: every insert
-- and delete, and every update EXCEPT one that only moves `last_seen_at` —
-- `authorize` stamps that liveness column at most once a minute per client,
-- and rebuilding the cache for it would be pure churn. A test pins both
-- tables' columns, so a new column cannot slip past this list unreviewed.
CREATE TABLE IF NOT EXISTS auth_epoch (
  id     INTEGER PRIMARY KEY CHECK (id = 1),
  epoch  INTEGER NOT NULL
);
INSERT OR IGNORE INTO auth_epoch (id, epoch) VALUES (1, 0);

CREATE TRIGGER IF NOT EXISTS auth_epoch_host_tokens_insert
  AFTER INSERT ON host_tokens
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_host_tokens_update
  AFTER UPDATE ON host_tokens
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_host_tokens_delete
  AFTER DELETE ON host_tokens
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

CREATE TRIGGER IF NOT EXISTS auth_epoch_client_tokens_insert
  AFTER INSERT ON client_tokens
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_tokens_update
  AFTER UPDATE ON client_tokens
  WHEN OLD.id IS NOT NEW.id
    OR OLD.name IS NOT NEW.name
    OR OLD.token_sha256 IS NOT NEW.token_sha256
    OR OLD.mode IS NOT NEW.mode
    OR OLD.created_at IS NOT NEW.created_at
    OR OLD.revoked_at IS NOT NEW.revoked_at
    OR OLD.trusted_at IS NOT NEW.trusted_at
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_tokens_delete
  AFTER DELETE ON client_tokens
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

INSERT OR IGNORE INTO schema_version (version) VALUES (60);
