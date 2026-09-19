-- Host boot identity (host-reboot safety net). `boot_id` is the kernel boot
-- id (the HOST kernel's, even from inside a container); `tmux_server_pid` is
-- the pid of the host's tmux server, NULL when none is running. A change in
-- either between probes marks the host's sessions lost instead of deleting
-- them. `lost_reason` says why a ghost row is lost:
-- host_reboot | tmux_server_gone | missing | killed (fleet killed the session itself).
ALTER TABLE hosts ADD COLUMN boot_id TEXT;
ALTER TABLE hosts ADD COLUMN tmux_server_pid INTEGER;
ALTER TABLE sessions ADD COLUMN lost_reason TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (36);
