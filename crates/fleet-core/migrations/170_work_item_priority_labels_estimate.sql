-- A task's priority, labels and estimate, shown in the task detail and
-- filled from trackers.
--
-- `work_items.priority`       urgent | high | medium | low, NULL = none. A
--                             tracker's own scale is normalised by its
--                             adapter (Jira's priority name, Linear's 1-4).
-- `work_items.labels`         a JSON array of strings, like `assignees`:
--                             at most 20, trimmed, each once, in order.
--                             NULL = none.
-- `work_items.estimate`       how big the work is, 0-1000, in
--                             `estimate_unit`. NULL = no estimate.
-- `work_items.estimate_unit`  points | hours; set exactly when `estimate` is.
--
-- A person sets them on a native item (`work_link { edit }`); a tracker's
-- sync writes its own and they stay read-only on its tickets.
ALTER TABLE work_items ADD COLUMN priority TEXT;
ALTER TABLE work_items ADD COLUMN labels TEXT;
ALTER TABLE work_items ADD COLUMN estimate REAL;
ALTER TABLE work_items ADD COLUMN estimate_unit TEXT;

INSERT OR IGNORE INTO schema_version (version) VALUES (170);
