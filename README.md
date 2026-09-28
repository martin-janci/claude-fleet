# update-channels

The signed release channels of claude-fleet (`<track>.json` +
`<track>.json.minisig`), written only by CI: `release.yml` after a release is
published, `update-channels.yml` for edits and the weekly re-sign. Never edit
by hand — a document whose signature does not verify is not a channel.
Design: `docs/superpowers/specs/2026-09-28-update-channel-design.md` §5.
