use super::*;
use crate::store::Store;

#[test]
fn recreate_session_errors_when_session_missing() {
    let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
    let args = RecreateSessionArgs {
        session_id: 999,
        force: false,
    };
    let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(recreate_session(args, &store, &ssh))
        .unwrap_err();
    assert_eq!(err.code, "E_NOTFOUND");
}

#[test]
fn recreate_session_errors_when_host_offline() {
    let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.conn_ref()
            .execute("UPDATE hosts SET reachable=0 WHERE alias='local'", [])
            .unwrap();
    }
    let id = store
        .lock()
        .unwrap()
        .get_session("dev", "local")
        .unwrap()
        .unwrap()
        .id;
    let args = RecreateSessionArgs {
        session_id: id,
        force: false,
    };
    let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(recreate_session(args, &store, &ssh))
        .unwrap_err();
    assert_eq!(err.code, "E_HOST_OFFLINE");
}

#[test]
fn dismiss_ghost_rejects_non_ghost() {
    let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
    }
    let id = store
        .lock()
        .unwrap()
        .get_session("dev", "local")
        .unwrap()
        .unwrap()
        .id;
    let args = DismissGhostSessionArgs { session_id: id };
    let result = dismiss_ghost_session(args, &store);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, "E_INVALID_STATE");
}

#[test]
fn dismiss_ghost_deletes_ghost_session() {
    let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
    }
    let id = store
        .lock()
        .unwrap()
        .get_session("dev", "local")
        .unwrap()
        .unwrap()
        .id;
    // Manually ghost it
    store
        .lock()
        .unwrap()
        .conn_ref()
        .execute(
            "UPDATE sessions SET status='ghost', lost_at=999 WHERE id=?1",
            rusqlite::params![id],
        )
        .unwrap();
    let args = DismissGhostSessionArgs { session_id: id };
    let result = dismiss_ghost_session(args, &store);
    assert!(result.is_ok());
    assert!(store
        .lock()
        .unwrap()
        .get_session_by_id(id)
        .unwrap()
        .is_none());
}

fn set_friendly_args(host: &str, tmux: &str, label: &str) -> SetFriendlyNameArgs {
    SetFriendlyNameArgs {
        host_alias: host.into(),
        tmux_name: tmux.into(),
        friendly_name: label.into(),
    }
}

#[test]
fn set_friendly_name_round_trips_through_store() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_session("dev-x", "h", None, None, 1, 1, "running", None)
            .unwrap();
    }
    let row = set_session_friendly_name(set_friendly_args("h", "dev-x", "fix login bug"), &store)
        .expect("update");
    assert_eq!(row.friendly_name.as_deref(), Some("fix login bug"));
    // Persisted, not just returned.
    let stored = store
        .lock()
        .unwrap()
        .get_session("dev-x", "h")
        .unwrap()
        .unwrap();
    assert_eq!(stored.friendly_name.as_deref(), Some("fix login bug"));
}

#[test]
fn set_friendly_name_trims_and_whitespace_clears() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_session("dev-x", "h", None, None, 1, 1, "running", None)
            .unwrap();
        // Seed an existing label so we can prove the clear path actually nulls it.
        s.set_friendly_name("h", "dev-x", Some("old label"))
            .unwrap();
    }
    // Padding around real content is trimmed.
    let row =
        set_session_friendly_name(set_friendly_args("h", "dev-x", "  trimmed label  "), &store)
            .expect("update");
    assert_eq!(row.friendly_name.as_deref(), Some("trimmed label"));
    // Whitespace-only input clears.
    let row =
        set_session_friendly_name(set_friendly_args("h", "dev-x", "   "), &store).expect("clear");
    assert!(row.friendly_name.is_none());
    // Empty input also clears.
    let row = set_session_friendly_name(set_friendly_args("h", "dev-x", ""), &store)
        .expect("clear empty");
    assert!(row.friendly_name.is_none());
}

#[test]
fn set_friendly_name_works_on_bg_session_rows() {
    // Regression: synthetic bg rows have `tmux_name = "bg:<uuid>"`, which
    // contains ':'. The create-mode `tmux_name` validator rejects ':',
    // so the lookup-mode `tmux_name_lookup` validator must be used here
    // — otherwise every bg agent fails with E_INVALID at the MCP layer.
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let uuid = "550e8400-e29b-41d4-a716-446655440000";
    let bg_name = format!("bg:{uuid}");
    {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_bg_session("local", &bg_name, None, uuid, Some("working"), 1)
            .unwrap();
    }
    let row = set_session_friendly_name(
        set_friendly_args("local", &bg_name, "review hardening spec"),
        &store,
    )
    .expect("bg row must be addressable");
    assert_eq!(row.tmux_name, bg_name);
    assert_eq!(row.kind, "bg");
    assert_eq!(row.friendly_name.as_deref(), Some("review hardening spec"));
}

#[test]
fn set_friendly_name_returns_not_found_when_row_absent() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("h").unwrap();
        // No session inserted.
    }
    let err = set_session_friendly_name(set_friendly_args("h", "ghost", "x"), &store)
        .expect_err("absent row");
    assert_eq!(err.code, "E_NOTFOUND");
}

#[test]
fn set_friendly_name_rejects_invalid_input() {
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    {
        let s = store.lock().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_session("dev-x", "h", None, None, 1, 1, "running", None)
            .unwrap();
    }
    // Control char rejected by validate::friendly_name.
    let err = set_session_friendly_name(set_friendly_args("h", "dev-x", "bad\nlabel"), &store)
        .expect_err("control char");
    assert_eq!(err.code, "E_INVALID");
    // Bad host alias rejected before any UPDATE.
    let err = set_session_friendly_name(set_friendly_args("-evil", "dev-x", "x"), &store)
        .expect_err("bad alias");
    assert_eq!(err.code, "E_INVALID");
}
