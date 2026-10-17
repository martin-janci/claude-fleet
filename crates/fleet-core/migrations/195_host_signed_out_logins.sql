-- Login health (Control review F6): the host's Claude logins known to be
-- signed out, as a JSON array of {"profile": null | "<name>", "since": <unix>}.
-- `profile` null is the host's own login; `since` is when fleet first saw it
-- signed out (a usage check that found the login expired or its token
-- rejected, or a headless run Claude refused for want of a login). NULL or
-- an empty array: every login is signed in as far as fleet knows. Placement
-- (`service::placement`) leaves such a login out, and Control's chat shows
-- one card per entry.
ALTER TABLE hosts ADD COLUMN signed_out_logins TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (195);
