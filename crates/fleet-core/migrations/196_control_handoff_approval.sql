-- 196: how each of Control's handoffs was let through (review 2026-10-17,
-- F1/F2). `approval` is
--   person        a person approved its confirm card and fleet ran it (F1)
--   pre_approved  the autonomy position let it through without a card (F2)
--   NULL          no card was needed, or a receipt from before 196
-- Control's chips and receipts say "pre-approved" for the second, so a call
-- nobody approved one by one is never shown as if somebody had.
--
-- An ADD COLUMN, guarded in MIGRATIONS (`control_handoffs_has_approval`).
ALTER TABLE control_handoffs ADD COLUMN approval TEXT
  CHECK (approval IS NULL OR approval IN ('person', 'pre_approved'));

INSERT OR IGNORE INTO schema_version (version) VALUES (196);
