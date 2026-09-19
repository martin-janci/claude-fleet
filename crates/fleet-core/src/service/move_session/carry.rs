//! The carry engine of `move_session`: script builders and output parsers
//! that take a worktree's state — unpushed commits, staged, modified and
//! untracked files, small git-ignored files — from the source host to the
//! target without origin. Pure: no I/O, no `async`; `mod.rs` runs the
//! scripts. See `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`.

/// `settings` key: largest git bundle (MiB) a move relays.
pub const SETTING_MAX_BUNDLE_MB: &str = "move.max_bundle_mb";
pub const DEFAULT_MAX_BUNDLE_MB: u64 = 500;
/// `settings` key: largest single git-ignored entry (KiB) a move carries.
pub const SETTING_IGNORED_ENTRY_KB: &str = "move.ignored_entry_kb";
pub const DEFAULT_IGNORED_ENTRY_KB: u64 = 1024;
/// `settings` key: total git-ignored payload (MiB) a move carries.
pub const SETTING_IGNORED_TOTAL_MB: &str = "move.ignored_total_mb";
pub const DEFAULT_IGNORED_TOTAL_MB: u64 = 20;
