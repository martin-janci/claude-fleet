//! The wire-contract revision: one additive integer the hub's `/events`
//! hello frame carries (`contract`) so a client can tell whether the row
//! shapes and tool results it depends on are the ones actually on the wire.
//!
//! # Why this exists
//!
//! A hub-client desktop deserialises hub tool results straight into the same
//! `fleet-core` row structs the hub serialised them from
//! (`src-tauri/src/backend/contract.rs` explains why that symmetry is a
//! hole: a renamed optional field does not fail to parse, it silently
//! defaults). Before this field existed the hub's version was logged and
//! nothing more — there was no way for a client to tell "an older hub, whose
//! rows I still understand" from "a hub whose rows changed shape under me".
//!
//! `CONTRACT_REVISION` is that signal. It is not the crate version and not
//! the app version (both change on every release, including ones that touch
//! nothing a client reads); it moves only when a client's assumptions about
//! the wire would actually break.
//!
//! # When to bump this
//!
//! Bump [`CONTRACT_REVISION`] for a change that **removes or renames a row
//! field, or changes what an existing field means**, on any type a client
//! deserialises off `/events` or a tool result — the set
//! `src-tauri/src/backend/hub_contract.golden.json` pins.
//!
//! Do **not** bump it for an additive change: a new field (row structs carry
//! `#[serde(default)]` precisely so an older client tolerates one), a new
//! row type, or a new event kind. Those are exactly what a client outside
//! this revision already tolerates — an unknown field is ignored, an unknown
//! event name is dropped by `known_event_name`. Bumping for an additive
//! change would make a perfectly compatible client refuse a hub for no
//! reason.
//!
//! A client compares this against the range of revisions it understands
//! (`MIN_HUB_CONTRACT`..=`MAX_HUB_CONTRACT` in the desktop's
//! `src-tauri/src/backend/contract.rs`) and, outside that range, does not
//! trust the hub's rows at all rather than risk showing a stuck or lost
//! session as healthy.
//!
//! # Revision history
//!
//! - **1** — the mechanism's own introduction (#148): no prior revision to
//!   compare against, so this is the bootstrap value every hub and this
//!   build started at together.
//! - **2** — `move_session` answers a tagged `MoveOutcome`
//!   (`{"kind": "moved" | "preview", ...}`) instead of a bare `MoveReport`,
//!   and honours a `dry_run` argument. An older hub's answer has no `kind`,
//!   which this build can no longer parse for a real move (`E_PARSE` after
//!   the move already happened); and an older hub's `MoveSessionParams`
//!   silently ignores an unknown `dry_run` field and runs a real move where
//!   this build asked for a read-only preview. Both are exactly what this
//!   mechanism exists to refuse instead of risking.
pub const CONTRACT_REVISION: u32 = 2;
