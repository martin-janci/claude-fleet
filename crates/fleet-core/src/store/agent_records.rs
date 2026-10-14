//! The fleet store (migration 185): records a fleet agent's `record`
//! actions write into a namespace, and its runs read back. A rule sees
//! only the namespaces it binds; `service::fleet_agents::records` keeps
//! that fence, this layer only reads and writes rows.

use super::Store;
use rusqlite::{params, OptionalExtension, Row};

/// Largest `value` JSON (the migration's CHECK).
pub const AGENT_RECORD_VALUE_MAX: usize = 64 * 1024;
/// Longest `key` (the migration's CHECK).
pub const AGENT_RECORD_KEY_MAX: usize = 300;
/// Most records one read answers.
pub const AGENT_RECORDS_MAX_LIMIT: usize = 100;

/// One record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentRecordRow {
    pub namespace: String,
    pub kind: String,
    pub key: String,
    /// JSON.
    pub value: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
    pub written_by_job: Option<i64>,
}

/// A record to write; `(namespace, kind, key)` replaces the one there,
/// keeping its `created_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAgentRecord {
    pub namespace: String,
    pub kind: String,
    pub key: String,
    pub value: String,
    pub expires_at: Option<i64>,
    pub written_by_job: Option<i64>,
}

const COLS: &str =
    "namespace, kind, key, value, created_at, updated_at, expires_at, written_by_job";

fn row(r: &Row<'_>) -> rusqlite::Result<AgentRecordRow> {
    Ok(AgentRecordRow {
        namespace: r.get(0)?,
        kind: r.get(1)?,
        key: r.get(2)?,
        value: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        expires_at: r.get(6)?,
        written_by_job: r.get(7)?,
    })
}

fn invalid(msg: String) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(msg)
}

/// `?, ?, …` for `n` namespaces after the first `skip` parameters.
fn in_list(skip: usize, n: usize) -> String {
    (skip + 1..=skip + n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `text` as a LIKE pattern that matches it literally (escape `\`).
fn like_pattern(text: &str) -> String {
    let mut out = String::from("%");
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

impl Store {
    /// Write a record. Refuses an over-long key, a value that is not JSON or
    /// is over [`AGENT_RECORD_VALUE_MAX`], before SQLite's CHECK would.
    pub fn put_agent_record(&self, rec: &NewAgentRecord, now: i64) -> Result<(), rusqlite::Error> {
        if rec.key.is_empty() || rec.key.chars().count() > AGENT_RECORD_KEY_MAX {
            return Err(invalid(format!(
                "a record key is 1-{AGENT_RECORD_KEY_MAX} characters"
            )));
        }
        if rec.value.len() > AGENT_RECORD_VALUE_MAX
            || serde_json::from_str::<serde_json::Value>(&rec.value).is_err()
        {
            return Err(invalid(format!(
                "a record value is JSON of at most {} KiB",
                AGENT_RECORD_VALUE_MAX / 1024
            )));
        }
        self.conn.execute(
            "INSERT INTO agent_records \
               (namespace, kind, key, value, created_at, updated_at, expires_at, written_by_job) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7) \
             ON CONFLICT(namespace, kind, key) DO UPDATE SET value = excluded.value, \
               updated_at = excluded.updated_at, expires_at = excluded.expires_at, \
               written_by_job = excluded.written_by_job",
            params![
                rec.namespace,
                rec.kind,
                rec.key,
                rec.value,
                now,
                rec.expires_at,
                rec.written_by_job
            ],
        )?;
        Ok(())
    }

    /// One record, unless it has expired.
    pub fn agent_record(
        &self,
        namespace: &str,
        kind: &str,
        key: &str,
        now: i64,
    ) -> Result<Option<AgentRecordRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM agent_records \
                     WHERE namespace = ?1 AND kind = ?2 AND key = ?3 \
                       AND (expires_at IS NULL OR expires_at > ?4)"
                ),
                params![namespace, kind, key, now],
                row,
            )
            .optional()
    }

    /// The newest records of `namespaces` (by `updated_at`), or those whose
    /// key or value contains `text` when it is given; expired ones left out.
    /// At most `limit` (≤ [`AGENT_RECORDS_MAX_LIMIT`]).
    pub fn find_agent_records(
        &self,
        namespaces: &[String],
        text: Option<&str>,
        limit: usize,
        now: i64,
    ) -> Result<Vec<AgentRecordRow>, rusqlite::Error> {
        if namespaces.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.min(AGENT_RECORDS_MAX_LIMIT) as i64;
        let mut values: Vec<rusqlite::types::Value> = vec![now.into(), limit.into()];
        let mut sql = format!(
            "SELECT {COLS} FROM agent_records \
             WHERE (expires_at IS NULL OR expires_at > ?1) AND namespace IN ({})",
            in_list(2, namespaces.len())
        );
        values.extend(namespaces.iter().map(|n| n.clone().into()));
        if let Some(t) = text.filter(|t| !t.is_empty()) {
            values.push(like_pattern(t).into());
            let i = values.len();
            sql.push_str(&format!(
                " AND (key LIKE ?{i} ESCAPE '\\' OR value LIKE ?{i} ESCAPE '\\')"
            ));
        }
        sql.push_str(" ORDER BY updated_at DESC, key LIMIT ?2");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), row)?;
        rows.collect()
    }

    /// Delete the expired records. Returns how many went.
    pub fn sweep_agent_records(&self, now: i64) -> Result<usize, rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM agent_records WHERE expires_at IS NOT NULL AND expires_at <= ?1",
            params![now],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ns: &str, key: &str, value: &str) -> NewAgentRecord {
        NewAgentRecord {
            namespace: ns.into(),
            kind: "record".into(),
            key: key.into(),
            value: value.into(),
            expires_at: None,
            written_by_job: None,
        }
    }

    #[test]
    fn a_record_is_replaced_in_place_and_read_back() {
        let s = Store::open_in_memory().unwrap();
        s.put_agent_record(&rec("rule:1", "NPE", r#"{"ticket":"L2S-1"}"#), 10)
            .unwrap();
        s.put_agent_record(&rec("rule:1", "NPE", r#"{"ticket":"L2S-2"}"#), 20)
            .unwrap();
        let r = s
            .agent_record("rule:1", "record", "NPE", 30)
            .unwrap()
            .unwrap();
        assert_eq!(
            (r.value.as_str(), r.created_at, r.updated_at),
            (r#"{"ticket":"L2S-2"}"#, 10, 20)
        );
        assert!(s
            .agent_record("rule:2", "record", "NPE", 30)
            .unwrap()
            .is_none());
        assert!(s.put_agent_record(&rec("rule:1", "", "{}"), 0).is_err());
        assert!(s.put_agent_record(&rec("rule:1", "k", "{"), 0).is_err());
        let big = format!("\"{}\"", "x".repeat(AGENT_RECORD_VALUE_MAX));
        assert!(s.put_agent_record(&rec("rule:1", "k", &big), 0).is_err());
    }

    #[test]
    fn reads_stay_in_their_namespaces_and_skip_the_expired() {
        let s = Store::open_in_memory().unwrap();
        s.put_agent_record(&rec("rule:1", "a", r#""100% sure""#), 10)
            .unwrap();
        s.put_agent_record(&rec("shared:fp", "b", r#""other""#), 20)
            .unwrap();
        s.put_agent_record(&rec("rule:2", "c", r#""100% hidden""#), 30)
            .unwrap();
        let mut gone = rec("rule:1", "d", r#""old""#);
        gone.expires_at = Some(15);
        s.put_agent_record(&gone, 12).unwrap();
        let mine = ["rule:1".to_string(), "shared:fp".to_string()];
        let keys = |v: Vec<AgentRecordRow>| v.into_iter().map(|r| r.key).collect::<Vec<_>>();
        assert_eq!(
            keys(s.find_agent_records(&mine, None, 10, 40).unwrap()),
            ["b", "a"]
        );
        assert_eq!(
            keys(s.find_agent_records(&mine, Some("100%"), 10, 40).unwrap()),
            ["a"]
        );
        assert_eq!(
            keys(s.find_agent_records(&mine, Some("_"), 10, 40).unwrap()),
            Vec::<String>::new(),
            "a LIKE wildcard is matched literally"
        );
        assert_eq!(
            keys(s.find_agent_records(&mine, None, 1, 40).unwrap()),
            ["b"]
        );
        assert!(s.find_agent_records(&[], None, 10, 40).unwrap().is_empty());
        assert!(s
            .agent_record("rule:1", "record", "d", 14)
            .unwrap()
            .is_some());
        assert!(s
            .agent_record("rule:1", "record", "d", 15)
            .unwrap()
            .is_none());
        assert_eq!(s.sweep_agent_records(40).unwrap(), 1);
    }
}
