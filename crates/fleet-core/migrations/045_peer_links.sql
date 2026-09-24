-- Hub↔hub federation (cycle 3). One row per link on each hub. A dialer knows
-- the listener's URL and holds a `peer` client token for it; a listener
-- knows which of its client tokens the dialer holds. `fleet_id` is learned at
-- the first exchange (the handshake) and pinned from then on.
CREATE TABLE IF NOT EXISTS peer_links (
  id               INTEGER PRIMARY KEY,
  fleet_id         TEXT,
  role             TEXT    NOT NULL,          -- 'dialer' | 'listener'
  url              TEXT,                      -- dialer only
  token            TEXT,                      -- dialer only; state.db is 0600
  client_id        INTEGER,                   -- listener only -> client_tokens
  after            INTEGER NOT NULL DEFAULT 0,-- dialer: highest peer message id stored here
  pending_rejects  TEXT,                      -- dialer: JSON rejections not yet reported
  state            TEXT    NOT NULL DEFAULT 'retrying',
  last_exchange_at INTEGER,
  last_error       TEXT,
  created_at       INTEGER NOT NULL,
  revoked_at       INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_peer_links_live_fleet
  ON peer_links(fleet_id) WHERE fleet_id IS NOT NULL AND revoked_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_peer_links_client
  ON peer_links(client_id) WHERE client_id IS NOT NULL;

-- A foreign endpoint is a participant of kind 'remote' with its full address.
ALTER TABLE participants ADD COLUMN address TEXT;
ALTER TABLE participants ADD COLUMN peer_link_id INTEGER;
CREATE UNIQUE INDEX IF NOT EXISTS idx_participants_address
  ON participants(address) WHERE address IS NOT NULL;

-- A remote end stores 0 in from_session_id / to_session_id (NOT NULL since
-- migration 015; no session has id 0); its participant is the true end.
ALTER TABLE session_messages ADD COLUMN remote_fleet_id TEXT;
ALTER TABLE session_messages ADD COLUMN remote_message_id INTEGER;
-- Outbound to a peer: 'pending' | 'accepted' | 'undeliverable'. NULL = local.
ALTER TABLE session_messages ADD COLUMN peer_state TEXT;
ALTER TABLE session_messages ADD COLUMN peer_wake INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_messages ADD COLUMN peer_from_addr TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_session_messages_remote
  ON session_messages(remote_fleet_id, remote_message_id)
  WHERE remote_fleet_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_session_messages_peer_pending
  ON session_messages(to_participant_id, id) WHERE peer_state = 'pending';

INSERT OR IGNORE INTO schema_version (version) VALUES (45);
