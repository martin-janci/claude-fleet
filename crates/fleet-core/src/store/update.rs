//! Application updates (migration 079): the hub's desired and observed state
//! for the fleet's own software, the transition log, and the cache of signed
//! documents it decides from. The rules are `service::update`'s; this module
//! only stores. Design: `docs/superpowers/specs/2026-09-28-update-channel-design.md`
//! §7.4.

use super::Store;
use crate::events::{EventBus as _, RowChange, UpdateChanged, UpdateDecision};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;

/// Transition rows older than this are pruned on insert.
pub const UPDATE_EVENT_RETENTION_SECS: i64 = 90 * 24 * 60 * 60;
/// At most this many transitions are kept per target (the newest), so a
/// reporter inventing attempts cannot grow the log without bound.
pub const UPDATE_EVENTS_PER_TARGET: u32 = 200;

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
    /// Emit `update:changed` (update design §11): ids only, kind `update`,
    /// so a host-bound or org-bound stream never carries it.
    pub fn emit_update_changed(&self, what: &str, target: Option<&str>) {
        self.bus.emit(&RowChange::UpdateChanged(UpdateChanged {
            what: what.into(),
            target: target.map(String::from),
        }));
    }

    /// Emit `update:decision` for one target (update design §6.4).
    pub fn emit_update_decision(&self, target: &str, status: &str, version: Option<&str>) {
        self.bus.emit(&RowChange::UpdateDecision(UpdateDecision {
            target: target.into(),
            status: status.into(),
            version: version.map(String::from),
        }));
    }

    /// Insert or replace what `row.target` says about itself. Emits
    /// `update:changed` only when something a reader shows moved (version,
    /// digest, phase, error, platform): a routine check that changes only
    /// the timestamps is silent.
    pub fn upsert_update_observed(&self, row: &UpdateObservedRow) -> Result<(), IpcError> {
        let moved = match self.update_observed(&row.target)? {
            None => true,
            Some(p) => {
                (&p.version, &p.digest, &p.phase, &p.last_error, &p.platform)
                    != (
                        &row.version,
                        &row.digest,
                        &row.phase,
                        &row.last_error,
                        &row.platform,
                    )
            }
        };
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
        if moved {
            self.emit_update_changed("observed", Some(&row.target));
        }
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
    /// was already recorded (a replayed report). Keeps the newest
    /// [`UPDATE_EVENTS_PER_TARGET`] rows of `target`.
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
        if n == 1 {
            self.conn.execute(
                "DELETE FROM update_events WHERE target = ?1 AND id NOT IN \
                   (SELECT id FROM update_events WHERE target = ?1 \
                    ORDER BY at DESC, id DESC LIMIT ?2)",
                rusqlite::params![target, UPDATE_EVENTS_PER_TARGET],
            )?;
        }
        Ok(n == 1)
    }

    /// The row id of the first transition recorded for `(target, attempt)`:
    /// the order in which the hub first heard of each attempt. `None` when
    /// none is kept.
    pub fn update_attempt_first_seen(
        &self,
        target: &str,
        attempt: &str,
    ) -> Result<Option<i64>, IpcError> {
        Ok(self.conn.query_row(
            "SELECT MIN(id) FROM update_events WHERE target = ?1 AND attempt = ?2",
            [target, attempt],
            |r| r.get(0),
        )?)
    }

    /// The newest `limit` transitions of `target`, newest first. Only tests
    /// read the log today; the dashboard's per-target history is S4b's.
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
        self.emit_update_changed(
            "pin",
            Some(&row.target)
                .filter(|t| !t.is_empty())
                .map(|t| t.as_str()),
        );
        Ok(())
    }

    /// `true` when a pin was removed.
    pub fn clear_update_desired(&self, component: &str, target: &str) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "DELETE FROM update_desired WHERE kind = 'artifact' AND component = ?1 AND target = ?2",
            [component, target],
        )?;
        if n > 0 {
            self.emit_update_changed("pin", Some(target).filter(|t| !t.is_empty()));
        }
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
        // An amendment (`<version>/<component>`) goes with its release.
        for d in self.update_docs("amendment")? {
            let version = d.key.split('/').next().unwrap_or_default();
            if !keep.iter().any(|k| k == version) {
                n += self.conn.execute(
                    "DELETE FROM update_docs WHERE kind = 'amendment' AND key = ?1",
                    [&d.key],
                )?;
            }
        }
        Ok(n)
    }
}

/// A staged rollout (migration 151, update design S9): `version` of
/// `component` opens to `waves[wave]` percent of its targets.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UpdateRolloutRow {
    pub id: i64,
    pub component: String,
    pub version: String,
    /// Cumulative percents, the last one 100.
    pub waves: Vec<u8>,
    pub wave: u32,
    pub wave_started_at: i64,
    pub paused_at: Option<i64>,
    pub paused_reason: Option<String>,
    pub halt_failure_ratio: f64,
    pub created_at: i64,
    pub ended_at: Option<i64>,
    /// `completed` | `aborted`, once ended.
    pub outcome: Option<String>,
}

const ROLLOUT_COLUMNS: &str = "id, component, version, waves, wave, wave_started_at, paused_at, \
    paused_reason, halt_failure_ratio, created_at, ended_at, outcome";

fn rollout_row(r: &rusqlite::Row) -> rusqlite::Result<UpdateRolloutRow> {
    let waves: String = r.get(3)?;
    Ok(UpdateRolloutRow {
        id: r.get(0)?,
        component: r.get(1)?,
        version: r.get(2)?,
        waves: serde_json::from_str(&waves).unwrap_or_else(|_| vec![100]),
        wave: r.get(4)?,
        wave_started_at: r.get(5)?,
        paused_at: r.get(6)?,
        paused_reason: r.get(7)?,
        halt_failure_ratio: r.get(8)?,
        created_at: r.get(9)?,
        ended_at: r.get(10)?,
        outcome: r.get(11)?,
    })
}

impl Store {
    /// Start a rollout; `E_CONFLICT` while `component` has an active one.
    pub fn insert_update_rollout(
        &self,
        component: &str,
        version: &str,
        waves: &[u8],
        halt_failure_ratio: f64,
        now: i64,
    ) -> Result<UpdateRolloutRow, IpcError> {
        if let Some(active) = self.update_rollout_active(component)? {
            return Err(IpcError::new(
                crate::ipc_error::codes::E_CONFLICT,
                format!(
                    "{component} already has an active rollout of {} (id {}); abort it first",
                    active.version, active.id
                ),
            ));
        }
        let waves_json = serde_json::to_string(waves).unwrap_or_else(|_| "[100]".into());
        self.conn.execute(
            "INSERT INTO update_rollouts (component, version, waves, wave, wave_started_at, \
               halt_failure_ratio, created_at) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?4)",
            rusqlite::params![component, version, waves_json, now, halt_failure_ratio],
        )?;
        let id = self.conn.last_insert_rowid();
        self.emit_update_changed("rollout", None);
        self.update_rollout(id)?
            .ok_or_else(|| IpcError::new(crate::ipc_error::codes::E_INTERNAL, "rollout vanished"))
    }

    pub fn update_rollout(&self, id: i64) -> Result<Option<UpdateRolloutRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ROLLOUT_COLUMNS} FROM update_rollouts WHERE id = ?1"),
                [id],
                rollout_row,
            )
            .optional()?)
    }

    /// The component's active rollout, if any.
    pub fn update_rollout_active(
        &self,
        component: &str,
    ) -> Result<Option<UpdateRolloutRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {ROLLOUT_COLUMNS} FROM update_rollouts \
                     WHERE component = ?1 AND ended_at IS NULL"
                ),
                [component],
                rollout_row,
            )
            .optional()?)
    }

    /// Every active rollout, then the newest `ended` ended ones.
    pub fn update_rollouts(&self, ended: u32) -> Result<Vec<UpdateRolloutRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ROLLOUT_COLUMNS} FROM update_rollouts WHERE ended_at IS NULL ORDER BY component"
        ))?;
        let mut out: Vec<UpdateRolloutRow> =
            stmt.query_map([], rollout_row)?.collect::<Result<_, _>>()?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ROLLOUT_COLUMNS} FROM update_rollouts WHERE ended_at IS NOT NULL \
             ORDER BY ended_at DESC, id DESC LIMIT ?1"
        ))?;
        out.extend(
            stmt.query_map([ended], rollout_row)?
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(out)
    }

    /// Pause (`Some((now, reason))`) or resume (`None`) an active rollout.
    /// Resuming restarts the wave's clock. `false` when it is not active.
    pub fn set_update_rollout_paused(
        &self,
        id: i64,
        paused: Option<(i64, &str)>,
        now: i64,
    ) -> Result<bool, IpcError> {
        let n = match paused {
            Some((at, reason)) => self.conn.execute(
                "UPDATE update_rollouts SET paused_at = ?2, paused_reason = ?3 \
                 WHERE id = ?1 AND ended_at IS NULL",
                rusqlite::params![id, at, reason],
            )?,
            None => self.conn.execute(
                "UPDATE update_rollouts SET paused_at = NULL, paused_reason = NULL, \
                   wave_started_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
                rusqlite::params![id, now],
            )?,
        };
        if n > 0 {
            self.emit_update_changed("rollout", None);
        }
        Ok(n > 0)
    }

    /// Open the next wave.
    pub fn advance_update_rollout(&self, id: i64, wave: u32, now: i64) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE update_rollouts SET wave = ?2, wave_started_at = ?3 \
             WHERE id = ?1 AND ended_at IS NULL",
            rusqlite::params![id, wave, now],
        )?;
        if n > 0 {
            self.emit_update_changed("rollout", None);
        }
        Ok(n > 0)
    }

    /// End an active rollout with `outcome` (`completed` | `aborted`).
    pub fn end_update_rollout(&self, id: i64, outcome: &str, now: i64) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE update_rollouts SET ended_at = ?2, outcome = ?3 \
             WHERE id = ?1 AND ended_at IS NULL",
            rusqlite::params![id, now, outcome],
        )?;
        if n > 0 {
            self.emit_update_changed("rollout", None);
        }
        Ok(n > 0)
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
    fn events_are_capped_per_target_keeping_the_newest() {
        let s = Store::open_in_memory().unwrap();
        let cap = super::UPDATE_EVENTS_PER_TARGET;
        for i in 0..cap + 5 {
            let attempt = format!("a{i}");
            assert!(s
                .insert_update_event(
                    "client:1",
                    Some(&attempt),
                    "checking",
                    None,
                    None,
                    None,
                    None,
                    1_000_000_000 + i64::from(i),
                )
                .unwrap());
        }
        // Another target is not touched by client:1's cap.
        s.insert_update_event(
            "client:2",
            Some("b"),
            "checking",
            None,
            None,
            None,
            None,
            1_000_000_000,
        )
        .unwrap();
        let rows = s.update_events("client:1", cap + 10).unwrap();
        assert_eq!(rows.len(), cap as usize);
        assert_eq!(rows[0].attempt, format!("a{}", cap + 4));
        assert_eq!(rows.last().unwrap().attempt, "a5");
        assert_eq!(s.update_events("client:2", 10).unwrap().len(), 1);
        assert_eq!(s.update_attempt_first_seen("client:1", "a0").unwrap(), None);
        assert!(s
            .update_attempt_first_seen("client:1", "a5")
            .unwrap()
            .is_some());
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
