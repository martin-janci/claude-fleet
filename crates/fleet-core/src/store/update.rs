//! Application updates (migration 076): the hub's desired and observed state
//! for the fleet's own software, the transition log, and the cache of signed
//! documents it decides from. The rules are `service::update`'s; this module
//! only stores. Design: `docs/superpowers/specs/2026-09-28-update-channel-design.md`
//! §7.4.

use super::Store;
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;

/// Transition rows older than this are pruned on insert.
pub const UPDATE_EVENT_RETENTION_SECS: i64 = 90 * 24 * 60 * 60;

/// An operator's pin: `target` empty means every target of the component.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UpdateDesiredRow {
    pub component: String,
    pub target: String,
    pub version: String,
    pub mandatory: bool,
    pub reason: Option<String>,
    pub set_by: String,
    pub set_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UpdateObservedRow {
    pub target: String,
    pub component: String,
    pub platform: Option<String>,
    pub version: String,
    pub commit_sha: Option<String>,
    pub build_id: Option<String>,
    pub digest: Option<String>,
    /// JSON: the caller's own protocol window.
    pub speaks: Option<String>,
    pub phase: String,
    pub attempt: Option<String>,
    pub last_error: Option<String>,
    pub reported_at: i64,
    pub last_checked_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UpdateEventRow {
    pub target: String,
    pub attempt: String,
    pub phase: String,
    pub from_version: Option<String>,
    pub to_version: Option<String>,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub at: i64,
}

/// A signed document exactly as fetched: `kind` is `channel` (key: track) or
/// `manifest` (key: version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDocRow {
    pub kind: String,
    pub key: String,
    pub body: String,
    pub sig: String,
    pub sequence: Option<i64>,
    pub fetched_at: i64,
}

const OBSERVED_COLUMNS: &str = "target, component, platform, version, commit_sha, build_id, \
    digest, speaks, phase, attempt, last_error, reported_at, last_checked_at";

fn observed_row(r: &rusqlite::Row) -> rusqlite::Result<UpdateObservedRow> {
    Ok(UpdateObservedRow {
        target: r.get(0)?,
        component: r.get(1)?,
        platform: r.get(2)?,
        version: r.get(3)?,
        commit_sha: r.get(4)?,
        build_id: r.get(5)?,
        digest: r.get(6)?,
        speaks: r.get(7)?,
        phase: r.get(8)?,
        attempt: r.get(9)?,
        last_error: r.get(10)?,
        reported_at: r.get(11)?,
        last_checked_at: r.get(12)?,
    })
}

fn desired_row(r: &rusqlite::Row) -> rusqlite::Result<UpdateDesiredRow> {
    Ok(UpdateDesiredRow {
        component: r.get(0)?,
        target: r.get(1)?,
        version: r.get(2)?,
        mandatory: r.get::<_, i64>(3)? != 0,
        reason: r.get(4)?,
        set_by: r.get(5)?,
        set_at: r.get(6)?,
    })
}

impl Store {
    /// Insert or replace what `row.target` says about itself.
    pub fn upsert_update_observed(&self, row: &UpdateObservedRow) -> Result<(), IpcError> {
        self.conn.execute(
            &format!(
                "INSERT INTO update_observed ({OBSERVED_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
                 ON CONFLICT(target) DO UPDATE SET component = excluded.component, \
                   platform = excluded.platform, version = excluded.version, \
                   commit_sha = excluded.commit_sha, build_id = excluded.build_id, \
                   digest = excluded.digest, speaks = excluded.speaks, phase = excluded.phase, \
                   attempt = excluded.attempt, last_error = excluded.last_error, \
                   reported_at = excluded.reported_at, \
                   last_checked_at = COALESCE(excluded.last_checked_at, update_observed.last_checked_at)"
            ),
            rusqlite::params![
                row.target,
                row.component,
                row.platform,
                row.version,
                row.commit_sha,
                row.build_id,
                row.digest,
                row.speaks,
                row.phase,
                row.attempt,
                row.last_error,
                row.reported_at,
                row.last_checked_at,
            ],
        )?;
        Ok(())
    }

    pub fn update_observed(&self, target: &str) -> Result<Option<UpdateObservedRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {OBSERVED_COLUMNS} FROM update_observed WHERE target = ?1"),
                [target],
                observed_row,
            )
            .optional()?)
    }

    pub fn update_observed_all(&self) -> Result<Vec<UpdateObservedRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {OBSERVED_COLUMNS} FROM update_observed ORDER BY component, target"
        ))?;
        let rows = stmt.query_map([], observed_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Record one transition. `false` when this `(target, attempt, phase)`
    /// was already recorded (a replayed report).
    #[allow(clippy::too_many_arguments)]
    pub fn insert_update_event(
        &self,
        target: &str,
        attempt: Option<&str>,
        phase: &str,
        from_version: Option<&str>,
        to_version: Option<&str>,
        detail: Option<&str>,
        error: Option<&str>,
        now: i64,
    ) -> Result<bool, IpcError> {
        self.conn.execute(
            "DELETE FROM update_events WHERE at < ?1",
            [now - UPDATE_EVENT_RETENTION_SECS],
        )?;
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO update_events \
               (target, attempt, phase, from_version, to_version, detail, error, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                target,
                attempt.unwrap_or(""),
                phase,
                from_version,
                to_version,
                detail,
                error,
                now
            ],
        )?;
        Ok(n == 1)
    }

    /// The newest `limit` transitions of `target`, newest first.
    pub fn update_events(&self, target: &str, limit: u32) -> Result<Vec<UpdateEventRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT target, attempt, phase, from_version, to_version, detail, error, at \
             FROM update_events WHERE target = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![target, limit], |r| {
            Ok(UpdateEventRow {
                target: r.get(0)?,
                attempt: r.get(1)?,
                phase: r.get(2)?,
                from_version: r.get(3)?,
                to_version: r.get(4)?,
                detail: r.get(5)?,
                error: r.get(6)?,
                at: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_update_desired(&self, row: &UpdateDesiredRow) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO update_desired (kind, component, target, version, mandatory, reason, set_by, set_at) \
             VALUES ('artifact', ?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(kind, component, target) DO UPDATE SET version = excluded.version, \
               mandatory = excluded.mandatory, reason = excluded.reason, \
               set_by = excluded.set_by, set_at = excluded.set_at",
            rusqlite::params![
                row.component,
                row.target,
                row.version,
                i64::from(row.mandatory),
                row.reason,
                row.set_by,
                row.set_at
            ],
        )?;
        Ok(())
    }

    /// `true` when a pin was removed.
    pub fn clear_update_desired(&self, component: &str, target: &str) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "DELETE FROM update_desired WHERE kind = 'artifact' AND component = ?1 AND target = ?2",
            [component, target],
        )?;
        Ok(n > 0)
    }

    /// The pin that applies to `target`: its own, else the component's.
    pub fn update_desired_for(
        &self,
        component: &str,
        target: &str,
    ) -> Result<Option<UpdateDesiredRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT component, target, version, mandatory, reason, set_by, set_at \
                 FROM update_desired WHERE kind = 'artifact' AND component = ?1 \
                   AND target IN (?2, '') \
                 ORDER BY target = '' ASC LIMIT 1",
                [component, target],
                desired_row,
            )
            .optional()?)
    }

    pub fn update_desired_all(&self) -> Result<Vec<UpdateDesiredRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT component, target, version, mandatory, reason, set_by, set_at \
             FROM update_desired WHERE kind = 'artifact' ORDER BY component, target",
        )?;
        let rows = stmt.query_map([], desired_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn put_update_doc(&self, doc: &UpdateDocRow) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO update_docs (kind, key, body, sig, sequence, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(kind, key) DO UPDATE SET body = excluded.body, sig = excluded.sig, \
               sequence = excluded.sequence, fetched_at = excluded.fetched_at",
            rusqlite::params![
                doc.kind,
                doc.key,
                doc.body,
                doc.sig,
                doc.sequence,
                doc.fetched_at
            ],
        )?;
        Ok(())
    }

    pub fn update_doc(&self, kind: &str, key: &str) -> Result<Option<UpdateDocRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT kind, key, body, sig, sequence, fetched_at FROM update_docs \
                 WHERE kind = ?1 AND key = ?2",
                [kind, key],
                |r| {
                    Ok(UpdateDocRow {
                        kind: r.get(0)?,
                        key: r.get(1)?,
                        body: r.get(2)?,
                        sig: r.get(3)?,
                        sequence: r.get(4)?,
                        fetched_at: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    /// Every cached document of `kind`.
    pub fn update_docs(&self, kind: &str) -> Result<Vec<UpdateDocRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, key, body, sig, sequence, fetched_at FROM update_docs \
             WHERE kind = ?1 ORDER BY key",
        )?;
        let rows = stmt.query_map([kind], |r| {
            Ok(UpdateDocRow {
                kind: r.get(0)?,
                key: r.get(1)?,
                body: r.get(2)?,
                sig: r.get(3)?,
                sequence: r.get(4)?,
                fetched_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Drop cached manifests whose version is not in `keep`.
    pub fn prune_update_manifests(&self, keep: &[String]) -> Result<usize, IpcError> {
        let cached = self.update_docs("manifest")?;
        let mut n = 0;
        for d in cached.iter().filter(|d| !keep.contains(&d.key)) {
            n += self.conn.execute(
                "DELETE FROM update_docs WHERE kind = 'manifest' AND key = ?1",
                [&d.key],
            )?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::{Store, UpdateDesiredRow, UpdateDocRow, UpdateObservedRow};

    fn observed(target: &str, version: &str, checked: Option<i64>) -> UpdateObservedRow {
        UpdateObservedRow {
            target: target.into(),
            component: "desktop".into(),
            platform: Some("macos-aarch64".into()),
            version: version.into(),
            commit_sha: None,
            build_id: None,
            digest: None,
            speaks: Some(r#"{"contract_accepts":[5,5]}"#.into()),
            phase: "idle".into(),
            attempt: None,
            last_error: None,
            reported_at: 100,
            last_checked_at: checked,
        }
    }

    #[test]
    fn observed_upserts_and_keeps_the_last_check() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_update_observed(&observed("client:1", "0.3.3", Some(50)))
            .unwrap();
        // A report carries no check time; the earlier one stays.
        s.upsert_update_observed(&observed("client:1", "0.3.4", None))
            .unwrap();
        let row = s.update_observed("client:1").unwrap().unwrap();
        assert_eq!(row.version, "0.3.4");
        assert_eq!(row.last_checked_at, Some(50));
        assert_eq!(s.update_observed_all().unwrap().len(), 1);
    }

    #[test]
    fn events_are_idempotent_and_pruned() {
        let s = Store::open_in_memory().unwrap();
        let ins = |attempt, phase, at| {
            s.insert_update_event(
                "hub:self",
                Some(attempt),
                phase,
                Some("0.3.3"),
                Some("0.3.4"),
                None,
                None,
                at,
            )
            .unwrap()
        };
        assert!(ins("a1", "downloading", 1_000_000_000));
        assert!(
            !ins("a1", "downloading", 1_000_000_001),
            "a replay adds nothing"
        );
        assert!(ins("a1", "success", 1_000_000_002));
        assert_eq!(s.update_events("hub:self", 10).unwrap()[0].phase, "success");
        // Far in the future: the old rows age out.
        assert!(ins(
            "a2",
            "downloading",
            1_000_000_000 + super::UPDATE_EVENT_RETENTION_SECS + 10
        ));
        assert_eq!(s.update_events("hub:self", 10).unwrap().len(), 1);
    }

    #[test]
    fn a_target_pin_outranks_the_component_pin() {
        let s = Store::open_in_memory().unwrap();
        let pin = |target: &str, version: &str| UpdateDesiredRow {
            component: "agent".into(),
            target: target.into(),
            version: version.into(),
            mandatory: false,
            reason: None,
            set_by: "operator".into(),
            set_at: 1,
        };
        s.set_update_desired(&pin("", "0.3.4")).unwrap();
        assert_eq!(
            s.update_desired_for("agent", "agent:box")
                .unwrap()
                .unwrap()
                .version,
            "0.3.4"
        );
        s.set_update_desired(&pin("agent:box", "0.3.3")).unwrap();
        assert_eq!(
            s.update_desired_for("agent", "agent:box")
                .unwrap()
                .unwrap()
                .version,
            "0.3.3"
        );
        assert_eq!(
            s.update_desired_for("agent", "agent:other")
                .unwrap()
                .unwrap()
                .version,
            "0.3.4"
        );
        assert!(s
            .update_desired_for("desktop", "client:1")
            .unwrap()
            .is_none());
        assert!(s.clear_update_desired("agent", "agent:box").unwrap());
        assert!(!s.clear_update_desired("agent", "agent:box").unwrap());
        assert_eq!(s.update_desired_all().unwrap().len(), 1);
    }

    #[test]
    fn docs_round_trip_and_prune() {
        let s = Store::open_in_memory().unwrap();
        let doc = |kind: &str, key: &str| UpdateDocRow {
            kind: kind.into(),
            key: key.into(),
            body: "{}".into(),
            sig: "sig".into(),
            sequence: None,
            fetched_at: 1,
        };
        s.put_update_doc(&doc("channel", "stable")).unwrap();
        s.put_update_doc(&doc("manifest", "0.3.3")).unwrap();
        s.put_update_doc(&doc("manifest", "0.3.4")).unwrap();
        assert_eq!(
            s.update_doc("channel", "stable").unwrap().unwrap().sig,
            "sig"
        );
        assert_eq!(s.prune_update_manifests(&["0.3.4".into()]).unwrap(), 1);
        assert_eq!(s.update_docs("manifest").unwrap().len(), 1);
        assert!(s.update_doc("channel", "stable").unwrap().is_some());
    }
}
