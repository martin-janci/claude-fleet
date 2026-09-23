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
//! Do **not** bump it for an additive change **an older client is built to
//! absorb**: a new field (row structs carry `#[serde(default)]` precisely so
//! an older client tolerates one), a new row type, or a new event kind. An
//! unknown field is ignored and an unknown event name is dropped by
//! `known_event_name`, so bumping for those would make a perfectly
//! compatible client refuse a hub for no reason.
//!
//! An addition an older client **cannot** absorb is not one of those. Two
//! shapes of it have bitten this build, and both belong in the "bump" list:
//!
//! * a new variant of an enum a client deserialises — serde fails the whole
//!   payload on a tag it does not know, so one new `ConvItem` kind costs an
//!   older desktop the entire `Conversation` rather than one line;
//! * a command the desktop now routes to a hub **tool that did not exist
//!   before** (an older hub's router answers "unknown tool", which no
//!   `#[serde(default)]` can soften).
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
//! - **3** — `move_session` takes a `when` argument (`now` | `idle` |
//!   `cancel`) and can answer `MoveOutcome::Waiting` or `WaitCancelled` in
//!   addition to `Moved`/`Preview`. An older hub ignores `when` entirely: for
//!   `idle` that is harmless (it sees no such field and refuses a busy
//!   source exactly as it always has), but for `cancel` it is not — the old
//!   hub sees an ordinary move request and MOVES the session, so cancelling
//!   a wait would perform the very move it was meant to stop. That is the
//!   one case this bump exists to refuse instead of risking.
//! - **4** — *additive enum variants, and a brand-new tool*. Two changes in
//!   one release that an older client cannot absorb, which is why the rule
//!   above grew the paragraph it did.
//!
//!   `ConvItem` gained `bash` and `harness` kinds. It is internally tagged,
//!   so a client that has never heard of `bash` does not skip that item — it
//!   fails to deserialise the **whole** `Conversation`, and the Conversation
//!   tab shows a parse error where a session's history used to be. (From
//!   this revision on, `ConvTurn::items` degrades an unreadable item to one
//!   placeholder line instead; that is what keeps revision 5 from costing a
//!   revision-4 client anything. Revision 3 and earlier have no such
//!   tolerance, so this bump is what tells them to stand off.)
//!
//!   `session_activity` is also new here — a tool the desktop now routes to
//!   the hub, and an older hub's router does not serve it. The caller
//!   swallows that failure, so the live-activity indicator the tool exists
//!   to drive silently never appears while the app retries every two
//!   seconds. A version skew this build cannot see is exactly what this
//!   number is for.
pub const CONTRACT_REVISION: u32 = 4;
