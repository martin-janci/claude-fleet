//! JSON shaping shared by everything that writes a row to a remote client.
//!
//! [`strip_nulls`] has two callers that must not drift apart, because they
//! send the *same row types* to the *same clients* over the same connection:
//!
//! - the MCP tool boundary — `mcp::tools::support::ok_json_compact`, where a
//!   list of forty-column rows would otherwise spend a large share of its
//!   bytes on `"field": null`;
//! - the hub's `/events` broadcast — [`crate::events::BroadcastEventBus`],
//!   where those same rows go out again on every single change.
//!
//! Until the bus used it, the two disagreed: `list_sessions` arrived
//! null-stripped and the `session:updated` frame for one of its rows did not.
//! Measured on a 44-session fleet, nulls were 34 % of the bytes `/events`
//! wrote in a fifteen-second sample.
//!
//! **Clients are built for the stripped shape**, which is what makes this
//! safe rather than a wire break: every optional field on a row type carries
//! `#[serde(default)]` (`src-tauri/src/backend/contract.rs` explains why, and
//! what it costs), and fleet-mobile's models give every field but `id` a
//! default for exactly this reason. An absent key and a null key deserialize
//! to the same `None`.
//!
//! The local Tauri event bus deliberately does *not* use this: the desktop
//! patches its stores from the payload, so there a null is how a value is
//! cleared. Only the remote wire is stripped.

/// Remove every `null`-valued key, recursively, in place.
///
/// Arrays are walked but not compacted: a `null` *element* is a position in a
/// list and means something, while a `null` *field* is the absence of a value
/// and does not.
pub fn strip_nulls(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|_, val| !val.is_null());
            for val in map.values_mut() {
                strip_nulls(val);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr.iter_mut() {
                strip_nulls(val);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_null_field_is_dropped_and_a_real_value_is_kept() {
        let mut v = json!({ "id": 7, "pr_url": null, "status": "running" });
        strip_nulls(&mut v);
        assert_eq!(v, json!({ "id": 7, "status": "running" }));
    }

    #[test]
    fn nested_objects_are_stripped_too() {
        let mut v = json!({ "row": { "a": null, "b": 1 }, "rows": [{ "c": null, "d": 2 }] });
        strip_nulls(&mut v);
        assert_eq!(v, json!({ "row": { "b": 1 }, "rows": [{ "d": 2 }] }));
    }

    #[test]
    fn a_null_element_keeps_its_place_in_an_array() {
        // Dropping it would renumber every element after it.
        let mut v = json!({ "xs": [1, null, 3] });
        strip_nulls(&mut v);
        assert_eq!(v, json!({ "xs": [1, null, 3] }));
    }

    #[test]
    fn an_empty_collection_is_not_a_null_and_stays() {
        // `tags: []` is "no tags", which a client renders; it is not absence.
        let mut v = json!({ "tags": [], "notes": "" });
        strip_nulls(&mut v);
        assert_eq!(v, json!({ "tags": [], "notes": "" }));
    }
}
