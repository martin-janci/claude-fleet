//! Debug devices (`debug_devices`, `debug_device_scans`, migration 120): the
//! phones, emulators and simulators a scan found on each host. A scan
//! replaces what fleet knows of a host's hardware; what a person set (a
//! label, sharing) and a live claim survive it. The line waiting for a
//! claimed device is `debug_device_queue` (migration 177). See
//! `service::debug_devices`.

use super::Store;
use rusqlite::{params, OptionalExtension, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugDeviceRow {
    pub id: i64,
    pub host_alias: String,
    pub dev_key: String,
    pub platform: String,
    pub kind: String,
    pub serial: Option<String>,
    pub name: String,
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub state: String,
    pub label: Option<String>,
    pub shared: bool,
    pub claimed_by: Option<String>,
    pub claim_note: Option<String>,
    pub claimed_until: Option<i64>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
}

/// One device as a scan saw it: the hardware half of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenDevice {
    pub dev_key: String,
    pub platform: String,
    pub kind: String,
    pub serial: Option<String>,
    pub name: String,
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub state: String,
}

/// One holder waiting in a device's line (`debug_device_queue`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugDeviceWaiter {
    pub device_id: i64,
    pub holder: String,
    pub note: Option<String>,
    pub queued_at: i64,
}

/// When a host was last scanned, and why that scan failed if it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugDeviceScan {
    pub scanned_at: i64,
    pub error: Option<String>,
}

const COLS: &str = "id, host_alias, dev_key, platform, kind, serial, name, model, os_version, \
                    state, label, shared, claimed_by, claim_note, claimed_until, first_seen_at, \
                    last_seen_at";

fn row(r: &rusqlite::Row<'_>) -> Result<DebugDeviceRow> {
    Ok(DebugDeviceRow {
        id: r.get(0)?,
        host_alias: r.get(1)?,
        dev_key: r.get(2)?,
        platform: r.get(3)?,
        kind: r.get(4)?,
        serial: r.get(5)?,
        name: r.get(6)?,
        model: r.get(7)?,
        os_version: r.get(8)?,
        state: r.get(9)?,
        label: r.get(10)?,
        shared: r.get::<_, i64>(11)? != 0,
        claimed_by: r.get(12)?,
        claim_note: r.get(13)?,
        claimed_until: r.get(14)?,
        first_seen_at: r.get(15)?,
        last_seen_at: r.get(16)?,
    })
}

impl Store {
    /// Every inventoried device, by host then name.
    pub fn debug_devices(&self) -> Result<Vec<DebugDeviceRow>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM debug_devices ORDER BY host_alias, name, id"
        ))?;
        let rows = st.query_map([], row)?.collect();
        rows
    }

    pub fn debug_device(&self, id: i64) -> Result<Option<DebugDeviceRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM debug_devices WHERE id = ?1"),
                [id],
                row,
            )
            .optional()
    }

    /// Record a successful scan of `host`: every `seen` device is inserted
    /// or refreshed, and a device of the host the scan did not see becomes
    /// `missing` (kept, with its label, sharing and claim, until forgotten).
    pub fn debug_devices_apply_scan(
        &self,
        host: &str,
        seen: &[SeenDevice],
        now: i64,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for d in seen {
            tx.execute(
                "INSERT INTO debug_devices (host_alias, dev_key, platform, kind, serial, name, \
                   model, os_version, state, first_seen_at, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10) \
                 ON CONFLICT (host_alias, dev_key) DO UPDATE SET \
                   platform = excluded.platform, kind = excluded.kind, serial = excluded.serial, \
                   name = excluded.name, \
                   model = COALESCE(excluded.model, debug_devices.model), \
                   os_version = COALESCE(excluded.os_version, debug_devices.os_version), \
                   state = excluded.state, last_seen_at = excluded.last_seen_at",
                params![
                    host,
                    d.dev_key,
                    d.platform,
                    d.kind,
                    d.serial,
                    d.name,
                    d.model,
                    d.os_version,
                    d.state,
                    now
                ],
            )?;
        }
        let keys: Vec<&str> = seen.iter().map(|d| d.dev_key.as_str()).collect();
        let mut st =
            tx.prepare_cached("SELECT id, dev_key FROM debug_devices WHERE host_alias = ?1")?;
        let gone: Vec<i64> = st
            .query_map([host], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|(_, k)| !keys.contains(&k.as_str()))
            .map(|(id, _)| id)
            .collect();
        drop(st);
        for id in gone {
            tx.execute(
                "UPDATE debug_devices SET state = 'missing' WHERE id = ?1",
                [id],
            )?;
        }
        tx.execute(
            "INSERT INTO debug_device_scans (host_alias, scanned_at, error) VALUES (?1, ?2, NULL) \
             ON CONFLICT (host_alias) DO UPDATE SET scanned_at = excluded.scanned_at, error = NULL",
            params![host, now],
        )?;
        tx.commit()
    }

    /// A scan of `host` failed: when, and why. Its devices keep their last
    /// known state, since an unreachable host says nothing about them.
    pub fn debug_devices_scan_failed(&self, host: &str, error: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO debug_device_scans (host_alias, scanned_at, error) VALUES (?1, ?2, ?3) \
             ON CONFLICT (host_alias) DO UPDATE SET scanned_at = excluded.scanned_at, \
               error = excluded.error",
            params![host, now, error],
        )?;
        Ok(())
    }

    pub fn debug_device_scans(&self) -> Result<BTreeMap<String, DebugDeviceScan>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT host_alias, scanned_at, error FROM debug_device_scans")?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    DebugDeviceScan {
                        scanned_at: r.get(1)?,
                        error: r.get(2)?,
                    },
                ))
            })?
            .collect();
        rows
    }

    /// A person's settings: `label` (`Some(None)` clears it) and `shared`.
    pub fn debug_device_configure(
        &self,
        id: i64,
        label: Option<Option<&str>>,
        shared: Option<bool>,
    ) -> Result<usize> {
        let mut n = 0;
        if let Some(label) = label {
            n += self.conn.execute(
                "UPDATE debug_devices SET label = ?2 WHERE id = ?1",
                params![id, label],
            )?;
        }
        if let Some(shared) = shared {
            n += self.conn.execute(
                "UPDATE debug_devices SET shared = ?2 WHERE id = ?1",
                params![id, shared as i64],
            )?;
        }
        Ok(n)
    }

    /// Take or extend the claim. The caller decided it may.
    pub fn debug_device_claim(
        &self,
        id: i64,
        holder: &str,
        note: Option<&str>,
        until: i64,
    ) -> Result<usize> {
        self.conn.execute(
            "UPDATE debug_devices SET claimed_by = ?2, claim_note = ?3, claimed_until = ?4 \
             WHERE id = ?1",
            params![id, holder, note, until],
        )
    }

    /// Push a held claim's end out to `until`, keeping its note.
    pub fn debug_device_extend_claim(&self, id: i64, holder: &str, until: i64) -> Result<usize> {
        self.conn.execute(
            "UPDATE debug_devices SET claimed_until = MAX(COALESCE(claimed_until, 0), ?3) \
             WHERE id = ?1 AND claimed_by = ?2",
            params![id, holder, until],
        )
    }

    pub fn debug_device_release(&self, id: i64) -> Result<usize> {
        self.conn.execute(
            "UPDATE debug_devices SET claimed_by = NULL, claim_note = NULL, claimed_until = NULL \
             WHERE id = ?1",
            [id],
        )
    }

    /// Every device's line, each first come first served.
    pub fn debug_device_queue(&self) -> Result<Vec<DebugDeviceWaiter>> {
        let mut st = self.conn.prepare_cached(
            "SELECT device_id, holder, note, queued_at FROM debug_device_queue \
             ORDER BY device_id, queued_at, rowid",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(DebugDeviceWaiter {
                    device_id: r.get(0)?,
                    holder: r.get(1)?,
                    note: r.get(2)?,
                    queued_at: r.get(3)?,
                })
            })?
            .collect();
        rows
    }

    /// Join a device's line, or renew the note of a place already held
    /// (keeping the place).
    pub fn debug_device_enqueue(
        &self,
        id: i64,
        holder: &str,
        note: Option<&str>,
        now: i64,
    ) -> Result<usize> {
        self.conn.execute(
            "INSERT INTO debug_device_queue (device_id, holder, note, queued_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (device_id, holder) DO UPDATE SET note = excluded.note",
            params![id, holder, note, now],
        )
    }

    /// Leave a device's line.
    pub fn debug_device_dequeue(&self, id: i64, holder: &str) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM debug_device_queue WHERE device_id = ?1 AND holder = ?2",
            params![id, holder],
        )
    }

    /// Move every line along: drop places taken before `expired_before`,
    /// and hand each device whose claim is gone or lapsed (at `now`) to the
    /// first in its line, with a claim until `now + handoff_secs`. Returns
    /// how many devices changed hands.
    pub fn debug_devices_settle_queue(
        &self,
        now: i64,
        expired_before: i64,
        handoff_secs: i64,
    ) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM debug_device_queue WHERE queued_at < ?1",
            [expired_before],
        )?;
        // A holder waiting for the claim it already holds has it.
        tx.execute(
            "DELETE FROM debug_device_queue WHERE EXISTS (SELECT 1 FROM debug_devices d \
               WHERE d.id = device_id AND d.claimed_by = holder AND d.claimed_until > ?1)",
            [now],
        )?;
        let heads: Vec<(i64, String, Option<String>)> = {
            let mut st = tx.prepare_cached(
                "SELECT q.device_id, q.holder, q.note FROM debug_device_queue q \
                 JOIN debug_devices d ON d.id = q.device_id \
                 WHERE (d.claimed_until IS NULL OR d.claimed_until <= ?1) \
                   AND q.rowid = (SELECT q2.rowid FROM debug_device_queue q2 \
                     WHERE q2.device_id = q.device_id ORDER BY q2.queued_at, q2.rowid LIMIT 1)",
            )?;
            let rows = st
                .query_map([now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        for (id, holder, note) in &heads {
            tx.execute(
                "UPDATE debug_devices SET claimed_by = ?2, claim_note = ?3, claimed_until = ?4 \
                 WHERE id = ?1",
                params![id, holder, note, now + handoff_secs],
            )?;
            tx.execute(
                "DELETE FROM debug_device_queue WHERE device_id = ?1 AND holder = ?2",
                params![id, holder],
            )?;
        }
        tx.commit()?;
        Ok(heads.len())
    }

    /// Set a device's state after fleet booted or shut it down itself.
    pub fn debug_device_set_state(&self, id: i64, state: &str) -> Result<usize> {
        self.conn.execute(
            "UPDATE debug_devices SET state = ?2 WHERE id = ?1",
            params![id, state],
        )
    }

    pub fn debug_device_forget(&self, id: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM debug_devices WHERE id = ?1", [id])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(key: &str, state: &str) -> SeenDevice {
        SeenDevice {
            dev_key: key.into(),
            platform: "android".into(),
            kind: "physical".into(),
            serial: Some(key.into()),
            name: format!("Phone {key}"),
            model: Some("Pixel_7".into()),
            os_version: None,
            state: state.into(),
        }
    }

    #[test]
    fn a_scan_upserts_and_marks_the_unseen_missing_keeping_what_a_person_set() {
        let s = Store::open_in_memory().unwrap();
        s.debug_devices_apply_scan("local", &[seen("A1", "online"), seen("B2", "online")], 10)
            .unwrap();
        let a = s
            .debug_devices()
            .unwrap()
            .into_iter()
            .find(|d| d.dev_key == "A1")
            .unwrap();
        s.debug_device_configure(a.id, Some(Some("bench phone")), Some(true))
            .unwrap();
        s.debug_device_claim(a.id, "host:x", Some("ui tests"), 99)
            .unwrap();

        s.debug_devices_apply_scan("local", &[seen("B2", "offline")], 20)
            .unwrap();
        let all = s.debug_devices().unwrap();
        assert_eq!(all.len(), 2);
        let a = all.iter().find(|d| d.dev_key == "A1").unwrap();
        assert_eq!(a.state, "missing");
        assert_eq!(a.label.as_deref(), Some("bench phone"));
        assert!(a.shared);
        assert_eq!(a.claimed_by.as_deref(), Some("host:x"));
        assert_eq!(a.last_seen_at, 10);
        let b = all.iter().find(|d| d.dev_key == "B2").unwrap();
        assert_eq!(
            (b.state.as_str(), b.last_seen_at, b.first_seen_at),
            ("offline", 20, 10)
        );
        // An absent model or OS version keeps the one seen before.
        assert_eq!(b.model.as_deref(), Some("Pixel_7"));
        assert_eq!(s.debug_device_scans().unwrap()["local"].scanned_at, 20);
    }

    #[test]
    fn a_failed_scan_is_recorded_and_a_good_one_clears_it() {
        let s = Store::open_in_memory().unwrap();
        s.debug_devices_scan_failed("local", "adb: boom", 5)
            .unwrap();
        assert_eq!(
            s.debug_device_scans().unwrap()["local"].error.as_deref(),
            Some("adb: boom")
        );
        s.debug_devices_apply_scan("local", &[], 6).unwrap();
        assert_eq!(s.debug_device_scans().unwrap()["local"].error, None);
    }

    #[test]
    fn a_line_hands_a_lapsed_claim_to_its_first_and_forgets_old_places() {
        let s = Store::open_in_memory().unwrap();
        s.debug_devices_apply_scan("local", &[seen("A1", "online")], 1)
            .unwrap();
        let id = s.debug_devices().unwrap()[0].id;
        s.debug_device_claim(id, "host:x", None, 100).unwrap();
        s.debug_device_enqueue(id, "host:old", None, 1).unwrap();
        s.debug_device_enqueue(id, "host:y", Some("ui"), 50)
            .unwrap();
        s.debug_device_enqueue(id, "host:z", None, 60).unwrap();
        // Renewing keeps the place.
        s.debug_device_enqueue(id, "host:y", Some("ui 2"), 70)
            .unwrap();
        // Held: nobody moves, but the day-old place goes.
        assert_eq!(s.debug_devices_settle_queue(90, 10, 600).unwrap(), 0);
        let line: Vec<String> = s
            .debug_device_queue()
            .unwrap()
            .into_iter()
            .map(|w| w.holder)
            .collect();
        assert_eq!(line, ["host:y", "host:z"]);
        // Lapsed: the first in line gets it, with its note.
        assert_eq!(s.debug_devices_settle_queue(100, 10, 600).unwrap(), 1);
        let d = s.debug_device(id).unwrap().unwrap();
        assert_eq!(
            (
                d.claimed_by.as_deref(),
                d.claim_note.as_deref(),
                d.claimed_until
            ),
            (Some("host:y"), Some("ui 2"), Some(700))
        );
        assert_eq!(s.debug_device_queue().unwrap().len(), 1);
        // Forgetting the device drops its line.
        s.debug_device_forget(id).unwrap();
        assert!(s.debug_device_queue().unwrap().is_empty());
    }

    #[test]
    fn a_removed_host_takes_its_devices() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("lab").unwrap();
        s.debug_devices_apply_scan("lab", &[seen("A1", "online")], 1)
            .unwrap();
        s.delete_host("lab").unwrap();
        assert!(s.debug_devices().unwrap().is_empty());
        assert!(s.debug_device_scans().unwrap().is_empty());
    }
}
