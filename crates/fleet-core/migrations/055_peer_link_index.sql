-- Federation review G17: `pending_outbox`, `has_pending_outbox`,
-- `mark_peer_accepted`, `handover_upto` and `sweep_peer_outbox` (via
-- `insert_outbound_remote`'s later reads) all join
-- `participants ON participants.peer_link_id = peer_links.id` to find a
-- link's outbox rows. Without an index that join is a full table scan of
-- `participants` on every poll of every dialer loop and every listener
-- exchange. Partial (`WHERE peer_link_id IS NOT NULL`): the column is NULL
-- for every non-remote participant, the overwhelming majority of rows.
CREATE INDEX IF NOT EXISTS idx_participants_peer_link
  ON participants(peer_link_id) WHERE peer_link_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (55);
