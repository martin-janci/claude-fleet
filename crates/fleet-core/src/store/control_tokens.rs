//! Named Control API tokens (migration 157, Orbit Fleet M15 step G2.8): a
//! token the hub's owner creates for a script or another agent, with a
//! scope, an expiry and an optional host limit.
//!
//! Like `client_tokens`, only the SHA-256 of the token is stored: the
//! plaintext exists once, in the answer to `create_api_token`, and never
//! again. The auth layer reads the live rows ([`Store::auth_control_tokens`])
//! and refuses an expired one per request (`mcp::auth::resolve_token_at`), so
//! an expiry needs no write and no sweep.

use super::*;
use crate::ipc_error::{codes, IpcError};

/// What a named token may do. The master token is [`ApiScope::Admin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiScope {
    /// Only the tools that observe the fleet (a `readonly` caller).
    Read,
    /// Start, send to and stop sessions: every tool but the fleet-admin and
    /// settings ones.
    Act,
    /// What the master token can do, settings and fleet admin included.
    Admin,
}

impl ApiScope {
    pub fn as_str(self) -> &'static str {
        match self {
            ApiScope::Read => "read",
            ApiScope::Act => "act",
            ApiScope::Admin => "admin",
        }
    }

    /// A stored scope. Anything unknown reads as `Read`: fail closed, as
    /// `TokenMode::parse` does.
    pub fn parse(s: &str) -> ApiScope {
        match s {
            "act" => ApiScope::Act,
            "admin" => ApiScope::Admin,
            _ => ApiScope::Read,
        }
    }

    /// A scope a caller asked for; `E_VALIDATE` for anything else.
    pub fn parse_strict(s: &str) -> Result<ApiScope, IpcError> {
        match s {
            "read" => Ok(ApiScope::Read),
            "act" => Ok(ApiScope::Act),
            "admin" => Ok(ApiScope::Admin),
            other => Err(IpcError::new(
                codes::E_VALIDATE,
                format!("scope {other:?} must be one of read | act | admin"),
            )),
        }
    }
}

/// One `control_tokens` row. Never carries the token, only its hash, and
/// the hash is not serialized.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ControlTokenRow {
    pub id: i64,
    pub name: String,
    #[serde(skip)]
    pub token_sha256: String,
    pub scope: ApiScope,
    /// The hosts it may reach; `None` is every host.
    pub hosts: Option<Vec<String>>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl ControlTokenRow {
    /// Past its expiry at `now`.
    pub fn expired_at(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|at| now >= at)
    }
}

const COLUMNS: &str =
    "id, name, token_sha256, scope, hosts, expires_at, created_at, last_used_at, revoked_at";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ControlTokenRow> {
    let hosts: Option<String> = r.get(4)?;
    Ok(ControlTokenRow {
        id: r.get(0)?,
        name: r.get(1)?,
        token_sha256: r.get(2)?,
        scope: ApiScope::parse(&r.get::<_, String>(3)?),
        // A list that does not parse limits the token to no host at all:
        // fail closed rather than widen to every host.
        hosts: hosts.map(|h| serde_json::from_str::<Vec<String>>(&h).unwrap_or_default()),
        expires_at: r.get(5)?,
        created_at: r.get(6)?,
        last_used_at: r.get(7)?,
        revoked_at: r.get(8)?,
    })
}

/// What [`Store::insert_control_token`] writes.
pub struct NewControlToken<'a> {
    pub name: &'a str,
    pub token_sha256: &'a str,
    pub scope: ApiScope,
    pub hosts: Option<&'a [String]>,
    pub expires_at: Option<i64>,
}

impl Store {
    /// Insert a named token. The name is validated as a client name is (it
    /// reaches the untrusted-content marker line); `E_INVALID` when a live
    /// row already holds it, or when an admin token is given a host limit.
    pub fn insert_control_token(
        &self,
        new: &NewControlToken<'_>,
    ) -> Result<ControlTokenRow, IpcError> {
        let name = validate_client_name(new.name)?;
        if new.scope == ApiScope::Admin && new.hosts.is_some() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "an admin token reaches every host; limit hosts on a read or act token",
            ));
        }
        let hosts = new
            .hosts
            .map(|h| serde_json::to_string(h).unwrap_or_else(|_| "[]".into()));
        self.conn
            .execute(
                "INSERT INTO control_tokens (name, token_sha256, scope, hosts, expires_at, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    name,
                    new.token_sha256,
                    new.scope.as_str(),
                    hosts,
                    new.expires_at,
                    now_unix()
                ],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    IpcError::new(
                        codes::E_INVALID,
                        format!("a token named '{name}' already exists"),
                    )
                }
                other => IpcError::from(other),
            })?;
        let id = self.conn.last_insert_rowid();
        self.control_token_by_id(id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!("control token {id} vanished right after insert"),
            )
        })
    }

    fn control_token_by_id(&self, id: i64) -> Result<Option<ControlTokenRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM control_tokens WHERE id = ?1"),
                [id],
                map_row,
            )
            .optional()?)
    }

    /// Every live (not revoked) named token, newest first, expired ones
    /// included: the list shows them as expired, and the auth layer refuses
    /// them by time.
    pub fn list_control_tokens(&self) -> Result<Vec<ControlTokenRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM control_tokens WHERE revoked_at IS NULL ORDER BY id DESC"
        ))?;
        let rows = stmt.query_map([], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The rows the auth layer matches against: the live ones.
    pub fn auth_control_tokens(&self) -> Result<Vec<ControlTokenRow>, IpcError> {
        self.list_control_tokens()
    }

    /// Revoke the live token named `name`. `E_NOTFOUND` when there is none.
    pub fn revoke_control_token(&self, name: &str) -> Result<ControlTokenRow, IpcError> {
        let name = name.trim();
        let id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM control_tokens WHERE name = ?1 AND revoked_at IS NULL",
                [name],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("no live token named '{name}'"))
            })?;
        self.conn.execute(
            "UPDATE control_tokens SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            rusqlite::params![id, now_unix()],
        )?;
        self.control_token_by_id(id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!("control token {id} vanished right after revoke"),
            )
        })
    }

    /// Stamp `last_used_at`, at most once a minute (it moves no auth epoch).
    pub fn touch_control_token(&self, id: i64, now: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE control_tokens SET last_used_at = ?2 \
             WHERE id = ?1 AND (last_used_at IS NULL OR last_used_at <= ?2 - 60)",
            rusqlite::params![id, now],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new<'a>(
        name: &'a str,
        sha: &'a str,
        scope: ApiScope,
        hosts: Option<&'a [String]>,
    ) -> NewControlToken<'a> {
        NewControlToken {
            name,
            token_sha256: sha,
            scope,
            hosts,
            expires_at: None,
        }
    }

    #[test]
    fn a_token_is_stored_by_hash_listed_and_revoked() {
        let s = Store::open_in_memory().unwrap();
        let hosts = vec!["mercury".to_string()];
        let row = s
            .insert_control_token(&new(
                " grafana ",
                &"a".repeat(64),
                ApiScope::Read,
                Some(&hosts),
            ))
            .unwrap();
        assert_eq!(row.name, "grafana");
        assert_eq!(row.hosts.as_deref(), Some(&hosts[..]));
        assert_eq!(s.list_control_tokens().unwrap(), vec![row.clone()]);
        // The hash never leaves through serde.
        let json = serde_json::to_string(&row).unwrap();
        assert!(!json.contains(&"a".repeat(64)), "{json}");
        assert!(json.contains("\"scope\":\"read\""), "{json}");
        let gone = s.revoke_control_token("grafana").unwrap();
        assert!(gone.revoked_at.is_some());
        assert!(s.list_control_tokens().unwrap().is_empty());
        assert_eq!(
            s.revoke_control_token("grafana").unwrap_err().code,
            codes::E_NOTFOUND
        );
        // A revoked name is free again.
        s.insert_control_token(&new("grafana", &"b".repeat(64), ApiScope::Act, None))
            .unwrap();
    }

    #[test]
    fn a_live_name_is_unique_and_an_admin_token_has_no_host_limit() {
        let s = Store::open_in_memory().unwrap();
        s.insert_control_token(&new("ci", &"a".repeat(64), ApiScope::Act, None))
            .unwrap();
        let dup = s
            .insert_control_token(&new("ci", &"b".repeat(64), ApiScope::Act, None))
            .unwrap_err();
        assert_eq!(dup.code, codes::E_INVALID);
        let hosts = vec!["mercury".to_string()];
        let admin = s
            .insert_control_token(&new("root", &"c".repeat(64), ApiScope::Admin, Some(&hosts)))
            .unwrap_err();
        assert_eq!(admin.code, codes::E_INVALID);
        let bad = s
            .insert_control_token(&new("a\nb", &"d".repeat(64), ApiScope::Read, None))
            .unwrap_err();
        assert_eq!(bad.code, codes::E_VALIDATE);
    }

    #[test]
    fn the_auth_epoch_moves_on_create_and_revoke_but_not_on_a_touch() {
        let s = Store::open_in_memory().unwrap();
        let e0 = s.auth_epoch().unwrap();
        let row = s
            .insert_control_token(&new("ci", &"a".repeat(64), ApiScope::Act, None))
            .unwrap();
        let e1 = s.auth_epoch().unwrap();
        assert!(e1 > e0, "create");
        s.touch_control_token(row.id, 1_000).unwrap();
        assert_eq!(s.auth_epoch().unwrap(), e1, "a touch is liveness only");
        s.revoke_control_token("ci").unwrap();
        assert!(s.auth_epoch().unwrap() > e1, "revoke");
    }

    #[test]
    fn an_unparseable_host_list_reaches_no_host() {
        let s = Store::open_in_memory().unwrap();
        let row = s
            .insert_control_token(&new(
                "ci",
                &"a".repeat(64),
                ApiScope::Act,
                Some(&["x".to_string()]),
            ))
            .unwrap();
        s.conn
            .execute(
                "UPDATE control_tokens SET hosts = 'nope' WHERE id = ?1",
                [row.id],
            )
            .unwrap();
        assert_eq!(s.list_control_tokens().unwrap()[0].hosts, Some(vec![]));
    }

    #[test]
    fn expiry_is_judged_by_time() {
        let row = ControlTokenRow {
            id: 1,
            name: "x".into(),
            token_sha256: String::new(),
            scope: ApiScope::Read,
            hosts: None,
            expires_at: Some(100),
            created_at: 0,
            last_used_at: None,
            revoked_at: None,
        };
        assert!(!row.expired_at(99));
        assert!(row.expired_at(100));
        assert!(!ControlTokenRow {
            expires_at: None,
            ..row
        }
        .expired_at(i64::MAX));
    }
}
