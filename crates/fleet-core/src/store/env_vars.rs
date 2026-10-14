//! Session environment variables (spec 2026-10-11-session-env-and-secrets-design.md).
//! Every statement that reads `env_vars` lives here (pinned by a test).

use std::collections::BTreeMap;

use super::{now_unix, Secret, Store};
use crate::ipc_error::{codes, IpcError};
use serde::Serialize;

/// Every scope, in precedence order: a later one wins over an earlier one
/// for the same name. `person` (migration 192) is the session OWNER's
/// (`sessions.owner_person_id`, keyed by `people.id` as text): it sits above
/// the host and project defaults, so a person's identity (git author,
/// `GH_TOKEN`) replaces a fleet-wide or per-host fallback, and below
/// `session`, so one session's explicit override still wins.
pub const ENV_SCOPES: &[&str] = &["global", "host", "project", "person", "session"];
const RESERVED: &[&str] = &[
    "PATH",
    "HOME",
    "SHELL",
    "USER",
    "TERM",
    "COLORTERM",
    "LANG",
    "TMUX",
    "CLAUDE_CONFIG_DIR",
    "ANTHROPIC_API_KEY",
];
const RESERVED_PREFIXES: &[&str] = &["LC_", "TMUX_", "CLAUDE_FLEET_"];
pub const MAX_ENV_VALUE: usize = 32 * 1024;

/// What `set_env_var` stores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    Plain(String),
    Secret(Secret),
    /// `env:NAME` | `file:/abs`, resolved by the controlling process at launch.
    Ref(String),
}

/// A merged entry for one launch. No `Serialize`; `Secret` redacts its Debug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvSource {
    Value(Secret),
    Ref(String),
}

/// A listing row: `value` only for a plain, non-reference entry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EnvVarRow {
    pub id: i64,
    pub scope: String,
    pub scope_key: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub secret: bool,
    pub is_ref: bool,
    pub updated_at: i64,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

pub fn check_env_name(name: &str) -> Result<(), IpcError> {
    let mut chars = name.chars();
    let ok = name.len() <= 128
        && matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    if !ok {
        return Err(invalid(format!(
            "{name:?} is not a variable name: letters, digits and _, not starting with a digit"
        )));
    }
    if RESERVED.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Err(invalid(format!(
            "{name} is set by fleet or the session itself"
        )));
    }
    Ok(())
}

pub fn check_env_value(value: &str) -> Result<(), IpcError> {
    if value.len() > MAX_ENV_VALUE {
        return Err(invalid(format!("a value is at most {MAX_ENV_VALUE} bytes")));
    }
    if value.contains('\0') {
        return Err(invalid("a value cannot contain a NUL byte"));
    }
    Ok(())
}

pub fn check_env_scope(scope: &str, key: &str) -> Result<(), IpcError> {
    let ok = match scope {
        "global" => key.is_empty(),
        "host" => !key.is_empty() && key.len() <= 255,
        "project" => matches!(
            key.split_once('/'),
            Some((o, r)) if !o.is_empty() && !r.is_empty() && !r.contains('/')
        ),
        "session" | "person" => key.parse::<i64>().map(|id| id > 0).unwrap_or(false),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "scope {scope:?} needs scope_key: '' (global), a host alias, owner/repo (project), \
             a person id (person) or a session id"
        )))
    }
}

fn rank(scope: &str) -> u8 {
    ENV_SCOPES.iter().position(|s| *s == scope).unwrap_or(0) as u8
}

impl Store {
    pub fn set_env_var(
        &self,
        scope: &str,
        scope_key: &str,
        name: &str,
        v: &EnvValue,
    ) -> Result<(), IpcError> {
        check_env_scope(scope, scope_key)?;
        check_env_name(name)?;
        let (value, cref, secret): (Option<&str>, Option<&str>, bool) = match v {
            EnvValue::Plain(s) => {
                check_env_value(s)?;
                (Some(s), None, false)
            }
            EnvValue::Secret(s) => {
                check_env_value(s.expose())?;
                (Some(s.expose()), None, true)
            }
            EnvValue::Ref(r) => {
                super::validate_credential_ref(r)?;
                (None, Some(r), true)
            }
        };
        self.conn.execute(
            "INSERT INTO env_vars (scope, scope_key, name, value, credential_ref, secret, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(scope, scope_key, name) DO UPDATE SET value = excluded.value, \
               credential_ref = excluded.credential_ref, secret = excluded.secret, \
               updated_at = excluded.updated_at",
            rusqlite::params![scope, scope_key, name, value, cref, secret as i64, now_unix()],
        )?;
        Ok(())
    }

    pub fn unset_env_var(
        &self,
        scope: &str,
        scope_key: &str,
        name: &str,
    ) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "DELETE FROM env_vars WHERE scope = ?1 AND scope_key = ?2 AND name = ?3",
            rusqlite::params![scope, scope_key, name],
        )?;
        Ok(n > 0)
    }

    pub fn list_env_vars(&self) -> Result<Vec<EnvVarRow>, IpcError> {
        let mut st = self.conn.prepare(
            "SELECT id, scope, scope_key, name, \
               CASE WHEN secret = 0 AND credential_ref IS NULL THEN value END, \
               secret, credential_ref IS NOT NULL, updated_at \
             FROM env_vars ORDER BY scope, scope_key, name",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok(EnvVarRow {
                    id: r.get(0)?,
                    scope: r.get(1)?,
                    scope_key: r.get(2)?,
                    name: r.get(3)?,
                    value: r.get(4)?,
                    secret: r.get::<_, i64>(5)? != 0,
                    is_ref: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The merged set for one launch, narrower scope winning, sorted by name.
    /// `person` is the session owner's `people.id` (`None`: an unclaimed
    /// session, which gets no person scope).
    pub fn env_for_launch(
        &self,
        host: &str,
        project: Option<&str>,
        person: Option<i64>,
        session: Option<i64>,
    ) -> Result<Vec<(String, EnvSource)>, IpcError> {
        let session = session.map(|id| id.to_string());
        let person = person.map(|id| id.to_string());
        let mut st = self.conn.prepare(
            "SELECT scope, name, value, credential_ref FROM env_vars \
             WHERE scope = 'global' OR (scope = 'host' AND scope_key = ?1) \
                OR (scope = 'project' AND scope_key = ?2) OR (scope = 'session' AND scope_key = ?3) \
                OR (scope = 'person' AND scope_key = ?4)",
        )?;
        let mut merged: BTreeMap<String, (u8, EnvSource)> = BTreeMap::new();
        let rows = st.query_map(rusqlite::params![host, project, session, person], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (scope, name, value, cref) = row?;
            let src = match (value, cref) {
                (_, Some(r)) => EnvSource::Ref(r),
                (Some(v), None) => EnvSource::Value(Secret::new(v)),
                (None, None) => continue,
            };
            let rk = rank(&scope);
            if merged.get(&name).is_none_or(|(have, _)| rk > *have) {
                merged.insert(name, (rk, src));
            }
        }
        Ok(merged.into_iter().map(|(n, (_, s))| (n, s)).collect())
    }

    /// Copy session `from`'s session-scope rows onto session `to` (a moved
    /// session's new row), secrets and references included, replacing any
    /// `to` already has under the same name. Nothing leaves the store.
    pub fn copy_session_env(&self, from: i64, to: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO env_vars \
               (scope, scope_key, name, value, credential_ref, secret, updated_at) \
             SELECT 'session', ?2, name, value, credential_ref, secret, updated_at FROM env_vars \
             WHERE scope = 'session' AND scope_key = ?1",
            rusqlite::params![from.to_string(), to.to_string()],
        )?;
        Ok(())
    }

    /// Stored secret values, for the diagnostics bundle's literal masking.
    pub fn env_secret_literals(&self) -> Result<Vec<String>, IpcError> {
        let mut st = self
            .conn
            .prepare("SELECT value FROM env_vars WHERE secret = 1 AND value IS NOT NULL")?;
        let out = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }
    fn plain(s: &str) -> EnvValue {
        EnvValue::Plain(s.into())
    }
    fn val(src: &EnvSource) -> String {
        match src {
            EnvSource::Value(s) => s.expose().to_string(),
            EnvSource::Ref(r) => format!("ref:{r}"),
        }
    }

    #[test]
    fn the_narrower_scope_wins() {
        let s = store();
        s.set_env_var("global", "", "A", &plain("g")).unwrap();
        s.set_env_var("host", "mercury", "A", &plain("h")).unwrap();
        s.set_env_var("project", "o/r", "A", &plain("p")).unwrap();
        s.set_env_var("session", "7", "A", &plain("s")).unwrap();
        s.set_env_var("global", "", "B", &plain("gb")).unwrap();
        s.set_env_var("host", "other", "C", &plain("no")).unwrap();
        s.set_env_var("person", "3", "A", &plain("me")).unwrap();
        s.set_env_var("person", "4", "D", &plain("not-mine"))
            .unwrap();
        let get = |h, p, who, sid| {
            s.env_for_launch(h, p, who, sid)
                .unwrap()
                .into_iter()
                .map(|(n, v)| format!("{n}={}", val(&v)))
                .collect::<Vec<_>>()
        };
        // global < host < project < person < session.
        assert_eq!(
            get("mercury", Some("o/r"), Some(3), Some(7)),
            ["A=s", "B=gb"]
        );
        assert_eq!(get("mercury", Some("o/r"), Some(3), None), ["A=me", "B=gb"]);
        assert_eq!(get("mercury", Some("o/r"), None, None), ["A=p", "B=gb"]);
        assert_eq!(get("mercury", None, None, None), ["A=h", "B=gb"]);
        assert_eq!(get("local", None, None, None), ["A=g", "B=gb"]);
        // Only the owner's person rows, never another person's.
        assert_eq!(
            get("local", None, Some(4), None),
            ["A=g", "B=gb", "D=not-mine"]
        );
    }

    #[test]
    fn set_replaces_and_unset_removes() {
        let s = store();
        s.set_env_var("global", "", "A", &plain("1")).unwrap();
        s.set_env_var("global", "", "A", &EnvValue::Ref("env:X".into()))
            .unwrap();
        let rows = s.list_env_vars().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_ref && rows[0].secret && rows[0].value.is_none());
        assert!(s.unset_env_var("global", "", "A").unwrap());
        assert!(!s.unset_env_var("global", "", "A").unwrap());
    }

    #[test]
    fn a_listing_never_carries_a_secret_or_a_reference() {
        let s = store();
        s.set_env_var("global", "", "PLAIN", &plain("visible"))
            .unwrap();
        s.set_env_var(
            "global",
            "",
            "SEC",
            &EnvValue::Secret(Secret::new("hunter2-hunter2")),
        )
        .unwrap();
        s.set_env_var(
            "global",
            "",
            "REF",
            // Absolute on Windows too, or the store refuses it.
            &EnvValue::Ref(
                if cfg!(windows) {
                    r"file:C:\run\fleet-ref-leaf"
                } else {
                    "file:/run/fleet-ref-leaf"
                }
                .into(),
            ),
        )
        .unwrap();
        let json = serde_json::to_string(&s.list_env_vars().unwrap()).unwrap();
        assert!(json.contains("visible"), "{json}");
        assert!(
            !json.contains("hunter2") && !json.contains("fleet-ref-leaf"),
            "{json}"
        );
    }

    #[test]
    fn names_scopes_and_values_are_checked() {
        let s = store();
        for bad in [
            "",
            "1A",
            "A-B",
            "PATH",
            "LC_ALL",
            "TMUX_PANE",
            "CLAUDE_FLEET_X",
            "ANTHROPIC_API_KEY",
        ] {
            let e = s.set_env_var("global", "", bad, &plain("v")).unwrap_err();
            assert_eq!(e.code, crate::ipc_error::codes::E_INVALID, "{bad}");
        }
        assert!(s.set_env_var("global", "x", "A", &plain("v")).is_err());
        assert!(s.set_env_var("host", "", "A", &plain("v")).is_err());
        assert!(s.set_env_var("session", "abc", "A", &plain("v")).is_err());
        assert!(s.set_env_var("person", "", "A", &plain("v")).is_err());
        assert!(s.set_env_var("person", "me", "A", &plain("v")).is_err());
        assert!(s.set_env_var("person", "0", "A", &plain("v")).is_err());
        assert!(s
            .set_env_var("project", "norepo", "A", &plain("v"))
            .is_err());
        assert!(s.set_env_var("planet", "x", "A", &plain("v")).is_err());
        assert!(s.set_env_var("global", "", "A", &plain("a\0b")).is_err());
        assert!(s
            .set_env_var("global", "", "A", &plain(&"x".repeat(32 * 1024 + 1)))
            .is_err());
        assert!(s
            .set_env_var("global", "", "A", &EnvValue::Ref("file:relative".into()))
            .is_err());
        // A refusal never echoes the value it refused.
        let e = s
            .set_env_var(
                "global",
                "",
                "PATH",
                &EnvValue::Secret(Secret::new("hunter2-hunter2")),
            )
            .unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        assert!(!e.message.contains("hunter2"), "{}", e.message);
    }

    #[test]
    fn deleting_a_session_drops_its_rows() {
        let s = store();
        s.upsert_host("local").unwrap();
        s.conn
            .execute(
                "INSERT INTO sessions (id, tmux_name, host_alias, created_at, last_activity_at, status) \
                 VALUES (7, 't', 'local', 0, 0, 'running')",
                [],
            )
            .unwrap();
        s.set_env_var("session", "7", "A", &plain("v")).unwrap();
        s.conn
            .execute("DELETE FROM sessions WHERE id = 7", [])
            .unwrap();
        assert!(s.list_env_vars().unwrap().is_empty());
    }

    #[test]
    fn secret_literals_are_the_stored_secrets_only() {
        let s = store();
        s.set_env_var("global", "", "P", &plain("plainplain"))
            .unwrap();
        s.set_env_var(
            "global",
            "",
            "S",
            &EnvValue::Secret(Secret::new("sekret-sekret")),
        )
        .unwrap();
        assert_eq!(s.env_secret_literals().unwrap(), ["sekret-sekret"]);
    }

    #[test]
    fn only_these_statements_read_the_table() {
        let src = include_str!("env_vars.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        for ty in ["pub enum EnvSource", "pub enum EnvValue"] {
            let at = src.find(ty).unwrap();
            let derive = &src[src[..at].rfind("#[derive").unwrap()..at];
            assert!(!derive.contains("Serialize"), "{ty} derives Serialize");
        }
        assert_eq!(
            src.matches("FROM env_vars").count(),
            5,
            "env_vars is read by list_env_vars, env_for_launch, \
             env_secret_literals and copy_session_env, and deleted by \
             unset_env_var only"
        );
    }
}
