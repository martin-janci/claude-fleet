-- Sub-project 4 (host agent): a host is reached either over SSH (as today)
-- or through an outbound fleet-agent WebSocket connection. `transport`
-- records which, per host row; `"ssh"` | `"agent"` are the only valid
-- values (enforced by Store::set_host_transport, not by the column).
-- See docs/superpowers/plans/2026-09-18-host-agent.md.
ALTER TABLE hosts ADD COLUMN transport TEXT NOT NULL DEFAULT 'ssh';

INSERT OR IGNORE INTO schema_version (version) VALUES (34);
