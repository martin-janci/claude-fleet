-- Host load (mission-control hardening, part 3): CPU utilisation and memory
-- pressure beside the existing health sample, and the two judgments the
-- auto-restore tick, the operator feed and a status-bar chip read.
--
-- cpu_busy_ticks / cpu_total_ticks: the raw `/proc/stat` counters of the
--   last sample (Linux), kept only so the next sample can take the delta.
--   NULL on macOS, which reports a percent directly.
-- cpu_pct: CPU busy percent over the last interval (Linux: the counter
--   delta; macOS: summed `ps` %CPU over the online CPUs). NULL = unknown.
-- mem_pressure: 0-100. Linux: PSI `some avg10` of /proc/pressure/memory
--   (the share of the last 10 s some task stalled on memory); macOS: the
--   kernel's pressure level (normal 0, warn 50, critical 100). NULL =
--   unknown.
-- mem_low / cpu_saturated: judged when the sample is written, against
--   `health.mem_low_pct` and `health.cpu_busy_pct` (CPU over the line on
--   two samples in a row). Readers ignore them once the sample is stale.
ALTER TABLE hosts ADD COLUMN cpu_busy_ticks INTEGER;
ALTER TABLE hosts ADD COLUMN cpu_total_ticks INTEGER;
ALTER TABLE hosts ADD COLUMN cpu_pct REAL;
ALTER TABLE hosts ADD COLUMN mem_pressure REAL;
ALTER TABLE hosts ADD COLUMN mem_low INTEGER NOT NULL DEFAULT 0;
ALTER TABLE hosts ADD COLUMN cpu_saturated INTEGER NOT NULL DEFAULT 0;
INSERT OR IGNORE INTO schema_version (version) VALUES (193);
