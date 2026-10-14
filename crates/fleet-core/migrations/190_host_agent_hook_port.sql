-- The hook relay: the loopback port an agent host's fleet-agent relays
-- Claude Code hooks on, from its last `hello` (`hook_port`). Provisioning
-- writes `http://127.0.0.1:<port>/hook` into that host's hooks instead of
-- the hub's public URL, which Claude Code refuses when it resolves to a
-- private address. NULL = no relay (an SSH host, an older agent, or one
-- whose relay is off or could not bind).
ALTER TABLE hosts ADD COLUMN agent_hook_port INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (190);
