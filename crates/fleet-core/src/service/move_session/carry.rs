//! The carry engine of `move_session`: script builders and output parsers
//! that take a worktree's state — unpushed commits, staged, modified and
//! untracked files, small git-ignored files — from the source host to the
//! target without origin. Pure: no I/O, no `async`; `mod.rs` runs the
//! scripts. See `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`.
