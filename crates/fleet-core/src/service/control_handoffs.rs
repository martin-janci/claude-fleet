//! Control's handoff receipts (redesign steps 9.3 and 9.6): when the
//! operator session — Control's agent — hands work on, the receipt says what
//! went where, so Control can draw "Sent to a session" and "Sent to a
//! mission" chips, live task cards and the proposed tree's card, each
//! following its target's state.
//!
//! The MCP server's `call_tool` hands every successful call of the operator
//! to [`record`]; [`handoff_of`] decides, from the tool, its arguments and
//! its result, whether that call was a handoff and to what. Nothing else
//! writes a receipt, and no caller but the operator gets one.

use crate::ipc_error::{lock, IpcError};
use crate::store::{handoff_preview, ControlHandoffRow, NewHandoff, Store};
use serde_json::Value;
use std::sync::Mutex;

/// How many receipts `control_handoffs` answers when the caller names none.
pub const DEFAULT_LIMIT: i64 = 50;

fn int(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// `item:<id>` (or a bare id) as `work_link`'s `parent` names a local item.
fn item_ref(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.strip_prefix("item:").unwrap_or(s).trim().parse().ok(),
        _ => None,
    }
}

/// PURE: the receipt a successful call of the operator leaves, or `None`
/// for a call that handed nothing on (a read, a key press, an edit).
/// `session_named` resolves a `host_alias` + `tmux_name` target to its
/// fleet id, as `send_prompt` accepts either.
pub fn handoff_of(
    tool: &str,
    args: &Value,
    result: &Value,
    session_named: impl Fn(&str, &str) -> Option<i64>,
) -> Option<NewHandoff> {
    let preview = |key: &str| text(args, key).and_then(handoff_preview);
    let base = NewHandoff {
        tool: tool.to_string(),
        ..Default::default()
    };
    match tool {
        "send_prompt" => {
            // A key press answers the session's own prompt; staged text
            // (`submit: false`) waits for a person's Enter. Neither is work
            // handed on.
            if text(args, "keys").is_some_and(|k| !k.is_empty())
                || args.get("submit").and_then(Value::as_bool) == Some(false)
            {
                return None;
            }
            let session_id = int(args, "session_id")
                .or_else(|| session_named(text(args, "host_alias")?, text(args, "tmux_name")?))?;
            Some(NewHandoff {
                kind: "session",
                session_id: Some(session_id),
                preview: preview("prompt"),
                ..base
            })
        }
        "queue_prompt" | "run_prompt" => Some(NewHandoff {
            kind: "session",
            session_id: Some(int(args, "session_id")?),
            preview: preview("prompt"),
            ..base
        }),
        "dispatch_task" => Some(NewHandoff {
            kind: "session",
            session_id: Some(int(result, "worker_session_id")?),
            task_id: int(result, "id"),
            preview: preview("prompt"),
            ..base
        }),
        "new_session" | "new_bg_session" => Some(NewHandoff {
            kind: "session",
            session_id: Some(int(result, "id")?),
            preview: text(result, "friendly_name")
                .or_else(|| text(args, "friendly_name"))
                .or_else(|| text(args, "name"))
                .and_then(handoff_preview),
            ..base
        }),
        "work_link" => match text(args, "action")? {
            "create" => Some(NewHandoff {
                kind: "task",
                item_id: Some(int(result, "id")?),
                preview: preview("title"),
                ..base
            }),
            "propose_tree" => {
                let rows = result.as_array()?;
                let item_ids: Vec<i64> = rows.iter().filter_map(|r| int(r, "id")).collect();
                if item_ids.is_empty() {
                    return None;
                }
                let parent = rows
                    .first()
                    .and_then(|r| int(r, "parent_id"))
                    .or_else(|| item_ref(args, "parent"));
                Some(NewHandoff {
                    kind: "tree",
                    item_id: parent,
                    item_ids,
                    ..base
                })
            }
            // A new mission; saving an existing one is an edit, not a
            // handoff.
            "mission_save" if int(args, "mission_id").is_none() => Some(NewHandoff {
                kind: "mission",
                mission_id: Some(int(result, "id")?),
                preview: args
                    .get("mission")
                    .and_then(|m| text(m, "goal").or_else(|| text(m, "name")))
                    .and_then(handoff_preview),
                ..base
            }),
            "mission_item" | "mission_start" | "mission_plan" => Some(NewHandoff {
                kind: "mission",
                mission_id: Some(int(args, "mission_id")?),
                item_id: if text(args, "action") == Some("mission_item") {
                    int(args, "item_id")
                } else {
                    None
                },
                ..base
            }),
            _ => None,
        },
        _ => None,
    }
}

/// The JSON a tool answered: its structured content, else its first text
/// block parsed. `None` for an answer that is not JSON (a pane dump).
pub fn result_json(result: &rmcp::model::CallToolResult) -> Option<Value> {
    if let Some(v) = result.structured_content.clone() {
        return Some(v);
    }
    result
        .content
        .iter()
        .find_map(|c| c.as_text())
        .and_then(|t| serde_json::from_str(&t.text).ok())
}

/// Write the receipt for one successful operator call, if it was a handoff,
/// and tell the owner's devices (`handoff:changed`). A failure here never
/// fails the call that already ran: the receipt is a view of it.
pub fn record(
    store: &Mutex<Store>,
    tool: &str,
    args: &Value,
    result: &rmcp::model::CallToolResult,
) {
    let Some(result) = result_json(result) else {
        return;
    };
    let Ok(s) = store.lock() else {
        return;
    };
    let named = |host: &str, tmux: &str| {
        s.find_sessions_by_tmux_name(tmux, Some(host))
            .ok()?
            .first()
            .map(|r| r.id)
    };
    let Some(h) = handoff_of(tool, args, &result, named) else {
        return;
    };
    match s.insert_control_handoff(&h, crate::store::now_unix()) {
        Ok(_) => s.bus_handoff_changed(),
        Err(e) => tracing::warn!(tool, "control handoff not recorded: {e}"),
    }
}

/// `control_handoffs { limit? }`: the newest receipts, newest first.
pub fn list(store: &Mutex<Store>, limit: Option<i64>) -> Result<Vec<ControlHandoffRow>, IpcError> {
    Ok(lock(store)?.list_control_handoffs(limit.unwrap_or(DEFAULT_LIMIT))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn none(_: &str, _: &str) -> Option<i64> {
        None
    }

    #[test]
    fn a_prompt_to_a_session_is_a_session_receipt() {
        let h = handoff_of(
            "send_prompt",
            &json!({ "session_id": 7, "prompt": "Fix the build\nthen push" }),
            &json!({}),
            none,
        )
        .unwrap();
        assert_eq!(h.kind, "session");
        assert_eq!(h.session_id, Some(7));
        assert_eq!(h.preview.as_deref(), Some("Fix the build"));
    }

    #[test]
    fn a_prompt_by_name_resolves_the_session() {
        let h = handoff_of(
            "send_prompt",
            &json!({ "host_alias": "box", "tmux_name": "w1", "prompt": "go" }),
            &json!({}),
            |h, t| (h == "box" && t == "w1").then_some(9),
        )
        .unwrap();
        assert_eq!(h.session_id, Some(9));
    }

    #[test]
    fn a_key_press_or_staged_text_is_no_handoff() {
        for args in [
            json!({ "session_id": 7, "prompt": "", "keys": "Enter" }),
            json!({ "session_id": 7, "prompt": "draft", "submit": false }),
        ] {
            assert_eq!(handoff_of("send_prompt", &args, &json!({}), none), None);
        }
    }

    #[test]
    fn a_dispatched_task_points_at_its_worker() {
        let h = handoff_of(
            "dispatch_task",
            &json!({ "worker_session_id": 3, "prompt": "Review #12" }),
            &json!({ "id": 41, "worker_session_id": 3, "state": "running" }),
            none,
        )
        .unwrap();
        assert_eq!(
            (h.kind, h.session_id, h.task_id),
            ("session", Some(3), Some(41))
        );
    }

    #[test]
    fn a_new_session_points_at_the_row_it_made() {
        let h = handoff_of(
            "new_session",
            &json!({ "host_alias": "box", "project_id": 1, "name": "w2" }),
            &json!({ "id": 12, "tmux_name": "w2" }),
            none,
        )
        .unwrap();
        assert_eq!((h.kind, h.session_id), ("session", Some(12)));
        assert_eq!(h.preview.as_deref(), Some("w2"));
    }

    #[test]
    fn missions_tasks_and_trees_have_their_own_kinds() {
        let m = handoff_of(
            "work_link",
            &json!({ "action": "mission_save", "mission": { "name": "M", "goal": "Ship it" } }),
            &json!({ "id": 5 }),
            none,
        )
        .unwrap();
        assert_eq!((m.kind, m.mission_id), ("mission", Some(5)));
        assert_eq!(m.preview.as_deref(), Some("Ship it"));
        // Saving an existing mission is an edit.
        assert_eq!(
            handoff_of(
                "work_link",
                &json!({ "action": "mission_save", "mission_id": 5, "mission": {} }),
                &json!({ "id": 5 }),
                none,
            ),
            None
        );
        let s = handoff_of(
            "work_link",
            &json!({ "action": "mission_start", "mission_id": 5 }),
            &json!({}),
            none,
        )
        .unwrap();
        assert_eq!((s.kind, s.mission_id), ("mission", Some(5)));
        let t = handoff_of(
            "work_link",
            &json!({ "action": "create", "title": "Write docs" }),
            &json!({ "id": 22 }),
            none,
        )
        .unwrap();
        assert_eq!((t.kind, t.item_id), ("task", Some(22)));
        let tree = handoff_of(
            "work_link",
            &json!({ "action": "propose_tree", "parent": "item:20", "tree": [] }),
            &json!([{ "id": 30, "parent_id": 20 }, { "id": 31, "parent_id": 20 }]),
            none,
        )
        .unwrap();
        assert_eq!((tree.kind, tree.item_id), ("tree", Some(20)));
        assert_eq!(tree.item_ids, vec![30, 31]);
    }

    #[test]
    fn reads_and_edits_are_no_handoff() {
        for (tool, args) in [
            ("list_sessions", json!({})),
            ("work_link", json!({ "action": "set_status", "item_id": 1 })),
            ("kill_session", json!({ "session_id": 1 })),
        ] {
            assert_eq!(handoff_of(tool, &args, &json!({ "id": 1 }), none), None);
        }
    }

    #[test]
    fn record_writes_the_receipt_and_says_so() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let store = Mutex::new(Store::open_with_bus_in_memory(bus.clone()).unwrap());
        let result = rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(
            r#"{"id":41,"worker_session_id":3}"#,
        )]);
        record(
            &store,
            "dispatch_task",
            &json!({ "worker_session_id": 3, "prompt": "Review" }),
            &result,
        );
        let rows = list(&store, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, Some(3));
        assert!(bus.names().contains(&"handoff:changed"));
    }
}
