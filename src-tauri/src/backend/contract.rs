//! The wire contract between this desktop and the hub it is a window onto.
//!
//! # Why this file exists
//!
//! In remote mode every row the UI renders is `serde_json::from_str`-ed out of
//! a hub's tool result into the very same `fleet-core` struct the hub
//! serialised it from. That symmetry is what makes [`super::remote`] a
//! deserialisation rather than a translation — and it is also the hole.
//!
//! Remote mode puts `#[serde(default)]` on roughly forty-six `Option` and
//! `Vec` fields, because the hub's list tools run results through
//! `ok_json_compact`, which strips every null key recursively; without the
//! attribute a row with any null column fails to parse. The cost is that
//! **a renamed optional field stops being an error and becomes a default**.
//!
//! The round-trip test (`a_null_stripped_session_row_survives_
//! the_round_trip`) cannot see that. It serialises with the struct and
//! deserialises with the same struct, so a rename moves both ends together
//! and the test stays green while the desktop shows wrong data. Every
//! round-trip test has this blind spot; it is not a flaw in that test.
//!
//! So this module pins the **concrete strings that appear on the wire**. The
//! expected names are literals here and a golden file next to this one — they
//! do not move when a struct field is renamed, which is exactly the point. A
//! rename fails a test instead of silently showing a ghost session as live.
//!
//! # The three that matter most
//!
//! For most fields a wrong default is visibly wrong: the status pill goes
//! blank, a badge disappears, grouping breaks. Three are different, because
//! for all three **absent means "everything is fine"**, so a rename turns a
//! problem into a clean bill of health with nothing in the log:
//!
//! - `lost_at` — `None` means "not lost", so a ghost session renders as live.
//! - `stuck_kind` — `None` means "not stuck", so a session wedged on an auth
//!   menu or an OOM looks healthy, and the stuck playbooks key off it.
//! - `context_pct` — `None` means no reading, so the context-red warning
//!   never fires.
//!
//! They get their own tests below, and
//! [`tests::a_renamed_optional_field_defaults_silently_which_is_the_whole_point`]
//! demonstrates the failure rather than asserting it cannot happen.
//!
//! # Regenerating the golden file
//!
//! ```text
//! REGEN_HUB_CONTRACT=1 cargo fleet-test -- the_hubs_field_names_are_the_ones_the_desktop_reads
//! ```
//!
//! Read the diff before committing it. A key that changed name is the hub
//! renaming a field the desktop reads; a key that vanished is a field the
//! desktop will silently default from here on.

use serde::Serialize;

/// Where the golden lives, relative to this file.
pub const GOLDEN_PATH: &str = "src/backend/hub_contract.golden.json";

/// The env var that rewrites [`GOLDEN_PATH`] instead of asserting against it.
pub const REGEN_ENV: &str = "REGEN_HUB_CONTRACT";

/// The top-level keys a value actually serialises to, sorted.
///
/// Sorted rather than declaration-ordered on purpose: reordering fields in a
/// struct is not a wire change, and a test that fails for it would be noise
/// that teaches people to regenerate the golden without reading it.
pub fn wire_keys<T: Serialize>(value: &T) -> Vec<String> {
    match serde_json::to_value(value).expect("a row type must serialise") {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            keys
        }
        other => panic!("expected a JSON object on the wire, got {other}"),
    }
}

// ── the hub's wire-contract revision ────────────────────────────────────────
//
// The golden file above pins the *names*. This pins whether this build
// trusts the hub it is talking to at all — see `fleet_core::wire_contract`
// for what the number means and when it moves. `super::events::pump` is the
// call site: it reads a hub's `ready` frame, classifies its `contract`
// against the range below, and — outside that range — reports a distinct
// [`super::connection::HubConnection`] state and ends the connection without
// resyncing or applying a single row from it.

/// The lowest hub wire-contract revision this build still trusts.
///
/// A hub that sends no `contract` field at all — every hub released before
/// this mechanism existed, or one still on revision 1 or 2 — is read as
/// revision `0`, `1` or `2` respectively ([`hub_contract_revision`]), all
/// now below this minimum: `move_session` taking a `when` argument (revision
/// 3, see [`fleet_core::wire_contract::CONTRACT_REVISION`]'s revision
/// history) is the day a hub ships that would silently perform a real move
/// for `when: cancel` instead of ending a wait. From here on such a hub is
/// refused instead of trusted with a silent default — raise this again the
/// next time that happens.
///
/// Raised to 4 for revision 4: the `ConvItem` kinds `bash` and `harness`, and
/// `session_activity` becoming a hub tool. A revision-3 hub does not serve
/// `session_activity` at all, so a desktop paired with one polled it every
/// two seconds and threw every answer away — no live indicator, no error, a
/// failing round-trip per panel per tick. Refusing that hub with an honest
/// skew banner is the whole point of the bump; leaving it `InRange` would
/// keep the silent version.
///
/// Raised to 5 for revision 5: `add_project` and `list_github_repos` became
/// hub tools the desktop routes to. A revision-4 hub does not serve them, so
/// a desktop paired with one would offer "Add project" and fail every
/// attempt with an unknown tool. Refusing that hub with the skew banner says
/// what to do (update the hub); leaving it `InRange` would not.
///
/// Raised to 6 for revision 6: the asset-catalog commands route to the new
/// `catalog_admin` tool. A revision-5 hub does not serve it, so the Assets
/// panel's every action would fail with an unknown tool.
///
/// Raised to 7 for revision 7: file downloads (`list_downloads`,
/// `send_file`, `remove_download`, `GET /downloads/<id>`). A revision-6 hub
/// serves none of them, so the Downloads sheet would fail every action.
///
/// Raised to 8 for revision 8: multi-user M1's sharing commands route to the
/// five new tools `session_share`, `session_unshare`, `session_narrow`,
/// `session_access` and `my_grants`. A revision-7 hub serves none of them, so
/// a desktop paired with one would open the Share sheet and fail every button
/// with an unknown tool — and `my_grants` is the one place this client learns
/// its own person id and grant set, so without it `src/lib/access.ts` keeps
/// its previous (empty) answer rather than widening and a session shared with
/// this person never becomes reachable at all. A watcher would see a row it
/// may read and no way to read it. Refusing that hub with the skew banner
/// says what to do (update the hub); leaving it `InRange` would ship a
/// sharing UI that silently does nothing.
///
/// Raised to 9 for revision 9: chat forms route `list_forms`, `get_form`,
/// `answer_form` and `decline_form` to the new `ask` tool. A revision-8 hub
/// serves none of it, so every answer would fail with an unknown tool.
///
/// Raised to 10 for revision 10: the Debug devices page routes its seven
/// commands to the new `debug_devices` tool, which a revision-9 hub lacks.
///
/// Raised to 11 for revision 11: the desktop routes `touch_session_viewed`
/// and `repo_blame` to tools a revision-10 hub lacks, so the unread state
/// would never clear and Blame would fail with an unknown tool.
///
/// Raised to 12 for revision 12: the desktop routes the prompt queue, the
/// branch and range diffs, presence, peer linking, shell terminals, Runs
/// and the Control handoffs to tools a revision-11 hub lacks.
///
/// Raised to 13 for revision 13: Control's routing, the Library, the lost
/// session's target and placement, and start rules route to tools a
/// revision-12 hub lacks.
///
/// Raised to 14 for revision 14: account headroom and mission triage route
/// to tools a revision-13 hub lacks.
///
/// Raised to 16 for revision 16: named tokens and + Add account route to
/// the `api_tokens` and `add_account` tools a revision-15 hub lacks.
///
/// Raised to 17 for revision 17: the composer's context help routes to the
/// `session_context_help` tool a revision-16 hub lacks.
pub const MIN_HUB_CONTRACT: u32 = 17;

/// The highest hub wire-contract revision this build understands. A hub
/// ahead of this is running row shapes compiled after this build was —
/// safer to say so than to guess at fields it has never seen.
///
/// Moves in lockstep with [`fleet_core::wire_contract::CONTRACT_REVISION`]:
/// this build's own hub must be `InRange`, so bumping the revision without
/// bumping this is a shipped outage against itself.
///
/// Raised to 8 with revision 8 (multi-user M1's five sharing tools), in that
/// lockstep: this build's own `fleet-hub` reports 8, and leaving this at 7
/// would make every desktop refuse the hub it ships beside as `TooNew`.
///
/// Raised to 9 with revision 9 (chat forms' `ask` tool), in that lockstep.
///
/// Raised to 10 with revision 10 (debug devices' `debug_devices` tool).
///
/// Raised to 11 with revision 11 (the Orbit Fleet redesign's M2).
///
/// Raised to 12 with revision 12 (the redesign tools after M2).
///
/// Raised to 13 with revision 13.
///
/// Raised to 14 with revision 14.
///
/// Raised to 15 with revision 15 (the wider form spec, G1.1). The minimum
/// stays 14: revision 15 adds no tool the desktop routes to, and a
/// revision-14 hub's forms are in the older shape this build still reads.
///
/// Raised to 16 with revision 16 (gap plan M15 batch 2).
///
/// Raised to 17 with revision 17 (`session_context_help`).
pub const MAX_HUB_CONTRACT: u32 = 17;

/// Where a hub's wire-contract revision stands against what this build
/// accepts. A pure function of the three numbers on purpose: the real bounds
/// are `10..=10` today, and unlike the original `0..=1` range this one CAN
/// exercise "too old" through a live `u32` (a hub reporting `0`…`3` is below
/// `4`) — see `tests_contract.rs`, independent of whichever bounds
/// a future release ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractFit {
    /// Inside `[min, max]`: trust this hub's rows.
    InRange,
    /// Below `min`: this build has seen a rename this hub still uses the old
    /// name for. Update the hub.
    TooOld,
    /// Above `max`: this hub may be speaking a shape this build predates.
    /// Update this app.
    TooNew,
}

/// A hub's wire-contract revision, read out of a `ready` frame's raw JSON.
///
/// `0` for a hub that sends no `contract` field (every hub released before
/// this mechanism existed) or a frame that fails to parse at all (a hub that
/// HAS a contract to report always JSON-encodes it correctly, so treating an
/// unparsable frame the same as a missing field costs nothing real) — both
/// are "nothing to distrust here" and land `0`, which is in today's range.
///
/// A `contract` key that IS present but is not a `u32` (a string, a float, a
/// negative number, or a number past `u32::MAX`) is a different case: some
/// hub sent something this build cannot read, which is exactly what this
/// mechanism exists to catch. Folding that to the same `0` as "nothing to
/// report" would trust it anyway. `u32::MAX` instead, so it reads as a hub
/// too new to understand ([`classify_hub_contract`] against any real
/// `MAX_HUB_CONTRACT`) rather than as a clean bill of health.
pub fn hub_contract_revision(ready_frame_data: &str) -> u32 {
    let Some(contract) = serde_json::from_str::<serde_json::Value>(ready_frame_data)
        .ok()
        .and_then(|v| v.get("contract").cloned())
    else {
        return 0;
    };
    contract
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(u32::MAX)
}

/// Classify `hub` against `[min, max]`. See [`ContractFit`].
pub fn classify_hub_contract(hub: u32, min: u32, max: u32) -> ContractFit {
    if hub < min {
        ContractFit::TooOld
    } else if hub > max {
        ContractFit::TooNew
    } else {
        ContractFit::InRange
    }
}

/// Every type in `new` that lost a wire key `old` had — a rename or removal,
/// the only change [`fleet_core::wire_contract::CONTRACT_REVISION`] must be
/// bumped for. A type that only gained keys, or is new outright, is not
/// reported: that is exactly the additive case the revision must NOT move
/// for.
///
/// Pure and file-free on purpose, unlike the regenerate path that calls it:
/// this is the part of "does the golden diff need a bump" worth testing
/// directly.
pub fn types_that_lost_fields(
    old: &std::collections::BTreeMap<String, Vec<String>>,
    new: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut lost = Vec::new();
    for (ty, want) in old {
        match new.get(ty) {
            None => lost.push(format!("{ty}: the whole type is gone")),
            Some(got) => {
                let gone: Vec<&String> = want.iter().filter(|k| !got.contains(k)).collect();
                if !gone.is_empty() {
                    lost.push(format!("{ty}: lost {gone:?}"));
                }
            }
        }
    }
    lost
}

/// What a `REGEN_HUB_CONTRACT=1` run should do, decided BEFORE anything
/// touches disk.
///
/// Pure and file-free like [`types_that_lost_fields`], and for the same
/// reason: the regenerate path used to compute `lost` and the revision
/// check, write the file regardless, and only panic afterwards — so a
/// developer who read the "bump CONTRACT_REVISION" panic and committed the
/// already-rewritten file had already shipped the non-additive change at
/// the old revision, with the write done before anyone could refuse it.
/// Deciding first and writing only on [`RegenVerdict::Write`] closes that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegenVerdict {
    /// Nothing was lost, or the revision already accounts for what was:
    /// safe to write the regenerated golden.
    Write,
    /// A field was renamed or removed and
    /// [`fleet_core::wire_contract::CONTRACT_REVISION`] does not reflect
    /// it yet. The golden must NOT be written — see the regenerate path in
    /// `tests_contract.rs`.
    Refuse { lost: Vec<String> },
}

/// `lost` is [`types_that_lost_fields`]'s output; `old_revision` the
/// on-disk golden's recorded revision; `current_revision` the compiled
/// `CONTRACT_REVISION`. A revision that did not move past `old_revision`
/// does not cover a loss — moving it below `old_revision` (a hand-edited
/// rollback) does not either.
pub fn regen_verdict(lost: Vec<String>, old_revision: u32, current_revision: u32) -> RegenVerdict {
    if !lost.is_empty() && current_revision <= old_revision {
        RegenVerdict::Refuse { lost }
    } else {
        RegenVerdict::Write
    }
}

#[cfg(test)]
#[path = "tests_contract.rs"]
// `pub(crate)` so the event-bridge tests can build their rows from the
// SAME fully-populated samples this module pins. Two sets of fixtures for
// one set of row types is how one of them goes stale.
pub(crate) mod tests;
