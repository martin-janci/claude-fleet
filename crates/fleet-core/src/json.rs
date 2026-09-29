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

/// The inverse of [`strip_nulls`], for a client that cannot treat an absent
/// key as `None` (the desktop's frontend checks `=== null`): put a `null`
/// back for every key `template` holds as `null` and `v` lacks, recursively
/// (objects by key, arrays by position).
///
/// `template` is the same row as `v`, round-tripped through its type — so it
/// has every field the type knows, with `null` where the value is `None`.
/// Nothing in `v` is ever removed or overwritten, and nothing but `null` is
/// ever added: a field the template does not know (a newer peer's) survives,
/// and a defaulted `[]` or `false` is not invented.
pub fn restore_nulls(v: &mut serde_json::Value, template: &serde_json::Value) {
    match (v, template) {
        (serde_json::Value::Object(map), serde_json::Value::Object(tmpl)) => {
            for (k, t) in tmpl {
                match map.get_mut(k) {
                    Some(val) => restore_nulls(val, t),
                    None if t.is_null() => {
                        map.insert(k.clone(), serde_json::Value::Null);
                    }
                    None => {}
                }
            }
        }
        (serde_json::Value::Array(arr), serde_json::Value::Array(tmpl)) => {
            for (val, t) in arr.iter_mut().zip(tmpl) {
                restore_nulls(val, t);
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
    fn restore_puts_back_exactly_the_nulls_strip_took() {
        let full = json!({
            "id": 7,
            "pr_url": null,
            "usage": { "five_hour": { "utilization": 0.0, "resets_at": null }, "opus": null },
            "rows": [{ "c": null, "d": 2 }],
        });
        let mut v = full.clone();
        strip_nulls(&mut v);
        restore_nulls(&mut v, &full);
        assert_eq!(v, full);
    }

    #[test]
    fn restore_keeps_a_field_the_template_does_not_know() {
        // A newer hub's field survives: restoring only ever adds nulls.
        let mut v = json!({ "id": 7, "new_field": [1, 2], "nested": { "x": 1, "y": true } });
        restore_nulls(
            &mut v,
            &json!({ "id": 7, "gone": null, "nested": { "x": 1, "z": null } }),
        );
        assert_eq!(
            v,
            json!({ "id": 7, "new_field": [1, 2], "gone": null, "nested": { "x": 1, "y": true, "z": null } })
        );
    }

    #[test]
    fn restore_never_overwrites_a_value_or_invents_a_non_null() {
        // Only a missing key whose template value is null is filled; a
        // defaulted `[]` or `false` in the template is not the wire's to add.
        let mut v = json!({ "a": 1 });
        restore_nulls(
            &mut v,
            &json!({ "a": null, "tags": [], "flag": false, "b": null }),
        );
        assert_eq!(v, json!({ "a": 1, "b": null }));
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
