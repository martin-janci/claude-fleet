//! The Claude-side state of a moved session: its per-session directory
//! (subagent transcripts, tool results, title) and the project's memory.
//! Pure, like `carry.rs`: policies, script builders, parsers — `mod.rs` runs
//! the scripts. Both halves only ever ADD to the target.
//! See `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`.

/// `settings` key: largest per-session directory (MiB) a move carries.
pub const SETTING_MAX_SESSION_STATE_MB: &str = "move.max_session_state_mb";
pub const DEFAULT_MAX_SESSION_STATE_MB: u64 = 200;
