-- The permission / question dialog a blocked session is showing, as JSON
-- ({kind, question, options[{n,label,selected}]}), or NULL when the pane shows
-- none. Derived by the reconcile pass from the same pane read that fills
-- current_activity; a phone turns the options into buttons.
ALTER TABLE sessions ADD COLUMN pending_input TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (40);
