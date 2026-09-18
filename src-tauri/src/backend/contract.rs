//! The wire contract between this desktop and the hub it is a window onto.
//!
//! # Why this file exists
//!
//! In remote mode every row the UI renders is `serde_json::from_str`-ed out of
//! a hub's tool result into the very same `fleet-core` struct the hub
//! serialised it from. That symmetry is what makes [`super::remote`] a
//! deserialisation rather than a translation — and it is also the hole.
//!
//! Task 2 had to put `#[serde(default)]` on roughly forty-six `Option` and
//! `Vec` fields, because the hub's list tools run results through
//! `ok_json_compact`, which strips every null key recursively; without the
//! attribute a row with any null column fails to parse. The cost is that
//! **a renamed optional field stops being an error and becomes a default**.
//!
//! The round-trip test Task 2 wrote (`a_null_stripped_session_row_survives_
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
//! REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract
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

#[cfg(test)]
#[path = "tests_contract.rs"]
mod tests;
