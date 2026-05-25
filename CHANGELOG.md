# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Note: release-please/CI is not in use; versions and this changelog are maintained
by hand on each build. Entries before 0.2.4 were plain version bumps and were not
recorded individually.

## [0.2.4] - 2026-05-25

### Added
- **Guided first-run onboarding**: one-time welcome dialog, a get-started
  checklist card atop the sidebar, local-prereq checks, tunnel-status surfacing,
  MCP port/token/copy with `bind_error` reporting, and a "Replay setup guide"
  entry in Settings. Backed by new `check_local_prereqs` / `tunnel_status`
  commands and onboarding service/store with pure step derivation.
- **Contextual first-use hints**: a `HintLayer` rendering viewport-clamped hint
  bubbles over tagged UI anchors, driven by a hint registry, plus a Settings
  toggle to show/reset feature hints.
- **Auto-slugify** for free-form worktree names when creating sessions.
- `TunnelSupervisor::snapshot` for surfacing tunnel status.

### Changed
- `fleet-friendly-name` skill now uses deterministic triggers.

### Fixed
- Hints: gate opens for existing users; corrected session-actions anchor; bubble
  re-measure on open; bubble z-index kept below modals.
- Onboarding: use the real `provisioned` field instead of the reachable proxy;
  welcome dialog dismisses on Escape.

### Documentation
- User-facing docs overhaul: README rewrite with a routing structure, a docs
  index, and new Getting Started, Concepts, and Troubleshooting guides; refreshed
  and cross-linked the Control API guide.

[0.2.4]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.4
