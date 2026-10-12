//! Admin events (identity sub-project 1, spec §3, migration 182): a
//! Keycloak-style audit log of every administrative change — who changed
//! which person, group, role, device or org, when, and with what.
//!
//! The table is append-only in SQL, not by convention: an UPDATE always
//! aborts, and a DELETE aborts unless [`Store::sweep_admin_events`] has
//! opened `admin_events_gc` inside its own transaction. The retention sweep
//! (`auth.audit_retention_days`, `service::gc`) is the only door.
//!
//! A representation never carries a secret: [`scrub`] drops every key that
//! could hold one, at any depth, before anything is written, and bounds the
//! text.

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::Connection;

/// Most rows one [`Store::admin_events`] read returns.
pub const ADMIN_EVENTS_MAX_LIMIT: u32 = 500;
/// Longest representation stored, in characters of its JSON text.
pub const REPRESENTATION_MAX_CHARS: usize = 4096;

/// Keys a representation never keeps, on top of the control API audit's
/// own skip list (`mcp::guard::SKIP_KEYS`: `code`, `secret`, `value`,
/// `api_key`, `confirm_nonce`).
const SECRET_KEYS: &[&str] = &["token", "token_sha256", "password", "client_secret"];

/// One administrative change, as a caller records it.
#[derive(Debug, Clone)]
pub struct AdminEvent<'a> {
    /// The person who made it; `None` for the operator on the hub machine.
    pub actor_person: Option<i64>,
    /// The device it came through, when it came through one.
    pub actor_device: Option<i64>,
    /// What was done (`create`, `set_member`, `disable_person`, …).
    pub action: &'a str,
    /// What kind of thing it was done to (`person`, `group`, `device`, …).
    pub resource_type: &'a str,
    /// Which one: its name, id or path.
    pub resource_id: &'a str,
    /// What the change said, scrubbed of secrets before it is stored.
    pub representation: serde_json::Value,
}

/// One stored admin event.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminEventRow {
    pub id: i64,
    /// Unix seconds.
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_person: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_device: Option<i64>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    /// The scrubbed representation. A text cut at
    /// [`REPRESENTATION_MAX_CHARS`] is no longer JSON and reads back as a
    /// JSON string holding the cut text.
    pub representation: serde_json::Value,
}

/// A filter over the log, newest first. Every set field narrows.
#[derive(Debug, Clone, Default)]
pub struct AdminEventQuery {
    /// Events at or after this Unix second.
    pub since: Option<i64>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub actor_person: Option<i64>,
    /// At most this many rows (capped at [`ADMIN_EVENTS_MAX_LIMIT`]; 0 reads
    /// as 1).
    pub limit: u32,
}

fn is_secret_key(k: &str) -> bool {
    crate::mcp::guard::SKIP_KEYS.contains(&k) || SECRET_KEYS.contains(&k)
}

/// `v` without any key that could hold a secret, at any depth.
pub fn scrub(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => serde_json::Value::Object(
            m.iter()
                .filter(|(k, _)| !is_secret_key(k))
                .map(|(k, v)| (k.clone(), scrub(v)))
                .collect(),
        ),
        serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(scrub).collect()),
        other => other.clone(),
    }
}

/// The text stored for `v`: scrubbed, then cut at
/// [`REPRESENTATION_MAX_CHARS`].
fn representation_text(v: &serde_json::Value) -> String {
    let text = scrub(v).to_string();
    if text.chars().count() > REPRESENTATION_MAX_CHARS {
        text.chars().take(REPRESENTATION_MAX_CHARS).collect()
    } else {
        text
    }
}

/// Record `ev` on `conn`, inside whatever transaction the caller holds
/// (never opens one). Returns the new row's id.
pub(crate) fn record_admin_event_in_tx(
    conn: &Connection,
    ev: &AdminEvent<'_>,
) -> Result<i64, IpcError> {
    if ev.action.trim().is_empty() || ev.resource_type.trim().is_empty() {
        return Err(IpcError::new(
            codes::E_INTERNAL,
            "an admin event names its action and resource type",
        ));
    }
    conn.execute(
        "INSERT INTO admin_events (ts, actor_person_id, actor_device_id, action, \
         resource_type, resource_id, representation) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            now_unix(),
            ev.actor_person,
            ev.actor_device,
            ev.action,
            ev.resource_type,
            ev.resource_id,
            representation_text(&ev.representation),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn map_event(r: &rusqlite::Row) -> rusqlite::Result<AdminEventRow> {
    let text: String = r.get(7)?;
    Ok(AdminEventRow {
        id: r.get(0)?,
        ts: r.get(1)?,
        actor_person: r.get(2)?,
        actor_device: r.get(3)?,
        action: r.get(4)?,
        resource_type: r.get(5)?,
        resource_id: r.get(6)?,
        representation: serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)),
    })
}

impl Store {
    /// Record one admin event. Plain statement: inside an open transaction
    /// (`Store::atomically`) it is part of it.
    pub fn record_admin_event(&self, ev: &AdminEvent<'_>) -> Result<i64, IpcError> {
        record_admin_event_in_tx(&self.conn, ev)
    }

    /// Record `ev` after a write that has already committed: a failure is
    /// logged, never raised, since the change it describes happened.
    pub fn record_admin_event_or_log(&self, ev: &AdminEvent<'_>) {
        if let Err(e) = self.record_admin_event(ev) {
            tracing::error!(
                error = %e,
                action = ev.action,
                resource_type = ev.resource_type,
                "[admin_events] could not record an admin event"
            );
        }
    }

    /// The log, newest first, narrowed by `q`.
    pub fn admin_events(&self, q: &AdminEventQuery) -> Result<Vec<AdminEventRow>, IpcError> {
        let limit = q.limit.clamp(1, ADMIN_EVENTS_MAX_LIMIT) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, actor_person_id, actor_device_id, action, resource_type, \
             resource_id, representation FROM admin_events \
             WHERE (?1 IS NULL OR ts >= ?1) \
               AND (?2 IS NULL OR resource_type = ?2) \
               AND (?3 IS NULL OR resource_id = ?3) \
               AND (?4 IS NULL OR actor_person_id = ?4) \
             ORDER BY id DESC LIMIT ?5",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                q.since,
                q.resource_type,
                q.resource_id,
                q.actor_person,
                limit
            ],
            map_event,
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Delete every event older than `older_than` (Unix seconds) — the
    /// retention sweep, and the only DELETE the table lets through. Opens
    /// `admin_events_gc` inside its own transaction and closes it before
    /// the commit. A crash between the two leaves nothing open: the
    /// transaction never committed, so the open is rolled back with it. A
    /// gate left open some other way (a hand-edited database) is shut by
    /// the next sweep's closing write. Returns how many rows went.
    pub fn sweep_admin_events(&self, older_than: i64) -> Result<usize, IpcError> {
        self.with_tx(|tx| {
            tx.execute("UPDATE admin_events_gc SET open = 1 WHERE id = 1", [])?;
            let n = tx.execute(
                "DELETE FROM admin_events WHERE ts < ?1",
                rusqlite::params![older_than],
            )?;
            tx.execute("UPDATE admin_events_gc SET open = 0 WHERE id = 1", [])?;
            Ok(n)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_events_are_append_only_except_for_the_sweep() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .record_admin_event(&AdminEvent {
                actor_person: None,
                actor_device: None,
                action: "create",
                resource_type: "group",
                resource_id: "/eng",
                representation: serde_json::json!({"name":"eng"}),
            })
            .unwrap();
        assert!(s
            .conn
            .execute("UPDATE admin_events SET action = 'x' WHERE id = ?1", [id])
            .is_err());
        assert!(s
            .conn
            .execute("DELETE FROM admin_events WHERE id = ?1", [id])
            .is_err());
        assert_eq!(s.sweep_admin_events(i64::MAX).unwrap(), 1);
        // The gate is shut again after the sweep.
        assert!(
            s.conn
                .query_row("SELECT open FROM admin_events_gc WHERE id = 1", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
                == 0
        );
    }

    #[test]
    fn a_representation_never_carries_a_secret() {
        let s = Store::open_in_memory().unwrap();
        s.record_admin_event(&AdminEvent {
            actor_person: None,
            actor_device: None,
            action: "pair",
            resource_type: "device",
            resource_id: "ada-mac",
            representation: serde_json::json!({"name":"ada-mac","code":"ABCD1234","secret":"x","token":"y"}),
        })
        .unwrap();
        let rep = &s
            .admin_events(&AdminEventQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap()[0]
            .representation;
        for k in ["code", "secret", "token"] {
            assert!(rep.get(k).is_none(), "{k} leaked: {rep}");
        }
        assert_eq!(rep["name"], "ada-mac");
    }

    #[test]
    fn a_nested_secret_is_dropped_and_a_long_representation_is_cut() {
        let v = serde_json::json!({
            "args": {"password": "p", "keep": 1, "list": [{"client_secret": "c", "ok": true}]},
            "token_sha256": "abc",
        });
        assert_eq!(
            scrub(&v),
            serde_json::json!({"args": {"keep": 1, "list": [{"ok": true}]}})
        );
        let s = Store::open_in_memory().unwrap();
        s.record_admin_event(&AdminEvent {
            actor_person: Some(1),
            actor_device: None,
            action: "update",
            resource_type: "person",
            resource_id: "1",
            representation: serde_json::json!({"note": "x".repeat(10_000)}),
        })
        .unwrap();
        let row = &s
            .admin_events(&AdminEventQuery {
                limit: 1,
                ..Default::default()
            })
            .unwrap()[0];
        let text = row
            .representation
            .as_str()
            .expect("cut text reads as a string");
        assert_eq!(text.chars().count(), REPRESENTATION_MAX_CHARS);
    }

    #[test]
    fn admin_events_filter_newest_first() {
        let s = Store::open_in_memory().unwrap();
        for (who, ty, id) in [
            (Some(1), "person", "ada"),
            (Some(2), "group", "/eng"),
            (None, "person", "bob"),
        ] {
            s.record_admin_event(&AdminEvent {
                actor_person: who,
                actor_device: None,
                action: "create",
                resource_type: ty,
                resource_id: id,
                representation: serde_json::json!({}),
            })
            .unwrap();
        }
        let all = s
            .admin_events(&AdminEventQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        let ids: Vec<&str> = all.iter().map(|e| e.resource_id.as_str()).collect();
        assert_eq!(ids, ["bob", "/eng", "ada"]);
        let people = s
            .admin_events(&AdminEventQuery {
                resource_type: Some("person".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(people.len(), 2);
        let by_two = s
            .admin_events(&AdminEventQuery {
                actor_person: Some(2),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_two.len(), 1);
        assert_eq!(by_two[0].resource_id, "/eng");
        let ada = s
            .admin_events(&AdminEventQuery {
                resource_id: Some("ada".into()),
                since: Some(0),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ada.len(), 1);
        assert!(s
            .admin_events(&AdminEventQuery {
                since: Some(i64::MAX),
                limit: 10,
                ..Default::default()
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_sweep_keeps_newer_events_and_reopens_a_gate_left_open() {
        let s = Store::open_in_memory().unwrap();
        let ev = AdminEvent {
            actor_person: None,
            actor_device: None,
            action: "create",
            resource_type: "person",
            resource_id: "ada",
            representation: serde_json::json!({}),
        };
        s.record_admin_event(&ev).unwrap();
        // Nothing is older than the epoch's first second.
        assert_eq!(s.sweep_admin_events(0).unwrap(), 0);
        assert_eq!(
            s.admin_events(&AdminEventQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap()
            .len(),
            1
        );
        // A gate somebody left open is shut by the next sweep.
        s.conn
            .execute("UPDATE admin_events_gc SET open = 1 WHERE id = 1", [])
            .unwrap();
        s.sweep_admin_events(0).unwrap();
        let id = s.record_admin_event(&ev).unwrap();
        assert!(s
            .conn
            .execute("DELETE FROM admin_events WHERE id = ?1", [id])
            .is_err());
    }
}
