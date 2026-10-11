//! Single-sign-on identities (migration 164): which person an OIDC account
//! is — Keycloak, or any provider that speaks OpenID Connect
//! (`mcp::oidc`).
//!
//! An account is the pair (issuer, subject): the `iss` and `sub` claims of
//! an ID token, the one pair OIDC promises is stable and never reassigned.
//! A username or e-mail address is a claim the provider may let its user
//! edit, so nothing here keys on one — the username only names the person
//! a FIRST sign-in creates.
//!
//! A link is written two ways: by a first sign-in that provisions a new
//! person (`mcp::oidc`), and by the operator on the hub machine (`fleet-hub
//! person link-sso`), which is how an account becomes an EXISTING person —
//! this hub's own owner above all. A sign-in never links itself to an
//! existing person by name: that would let anybody who can set their
//! username on the provider walk into a colleague's sessions.

use super::{breaks_a_line, now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

/// Longest issuer URL or subject stored. A Keycloak subject is a UUID and
/// an issuer a URL; the bound is for what a hostile or broken provider
/// could otherwise send.
pub const MAX_IDENTITY_FIELD_LEN: usize = 512;

/// One linked account.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdentityRow {
    pub issuer: String,
    pub subject: String,
    pub person_id: i64,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<i64>,
}

const IDENTITY_COLUMNS: &str = "issuer, subject, person_id, created_at, last_login_at";

fn map_identity(row: &rusqlite::Row) -> rusqlite::Result<IdentityRow> {
    Ok(IdentityRow {
        issuer: row.get(0)?,
        subject: row.get(1)?,
        person_id: row.get(2)?,
        created_at: row.get(3)?,
        last_login_at: row.get(4)?,
    })
}

/// The issuer is stored without a trailing `/`, so `…/realms/acme` and
/// `…/realms/acme/` are one issuer, as every provider treats them.
pub fn normalize_issuer(issuer: &str) -> String {
    issuer.trim().trim_end_matches('/').to_string()
}

fn check_field(what: &str, value: &str) -> Result<(), IpcError> {
    if value.is_empty()
        || value.chars().count() > MAX_IDENTITY_FIELD_LEN
        || value.chars().any(breaks_a_line)
    {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!("an identity's {what} is 1–{MAX_IDENTITY_FIELD_LEN} characters, one line"),
        ));
    }
    Ok(())
}

impl Store {
    /// The account (issuer, subject), if it is linked to anybody — live or
    /// disabled: the caller tells the two apart, and refuses a disabled one.
    pub fn get_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<IdentityRow>, IpcError> {
        let issuer = normalize_issuer(issuer);
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {IDENTITY_COLUMNS} FROM person_identities \
                     WHERE issuer = ?1 AND subject = ?2"
                ),
                rusqlite::params![issuer, subject],
                map_identity,
            )
            .optional()?)
    }

    /// Every account linked to `person_id`, oldest first.
    pub fn list_identities(&self, person_id: i64) -> Result<Vec<IdentityRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {IDENTITY_COLUMNS} FROM person_identities \
             WHERE person_id = ?1 ORDER BY created_at, issuer, subject"
        ))?;
        let rows = stmt
            .query_map(rusqlite::params![person_id], map_identity)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Link the account to a LIVE person. `E_EXISTS` when the account is
    /// already linked to somebody else — moving an account between people is
    /// an unlink and a link, two deliberate steps, never a side effect.
    /// Linking it to the person it already belongs to is a no-op.
    pub fn link_identity(
        &self,
        issuer: &str,
        subject: &str,
        person_id: i64,
    ) -> Result<IdentityRow, IpcError> {
        let issuer = normalize_issuer(issuer);
        let subject = subject.trim();
        check_field("issuer", &issuer)?;
        check_field("subject", subject)?;
        match self.get_person(person_id)? {
            Some(p) if p.disabled_at.is_none() => {}
            Some(_) => {
                return Err(IpcError::new(
                    codes::E_VALIDATE,
                    format!("person {person_id} is disabled; an account cannot be linked to them"),
                ))
            }
            None => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("person {person_id} not found"),
                ))
            }
        }
        if let Some(existing) = self.get_identity(&issuer, subject)? {
            if existing.person_id == person_id {
                return Ok(existing);
            }
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "that account is already linked to person {}; unlink it first",
                    existing.person_id
                ),
            ));
        }
        self.conn.execute(
            "INSERT INTO person_identities (issuer, subject, person_id, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![issuer, subject, person_id, now_unix()],
        )?;
        self.get_identity(&issuer, subject)?.ok_or_else(|| {
            IpcError::new(codes::E_INTERNAL, "the identity link vanished after insert")
        })
    }

    /// Remove the link; `true` when there was one. The person and their
    /// devices are untouched: a device already paired keeps working until
    /// it is revoked (`fleet-hub client revoke`).
    pub fn unlink_identity(&self, issuer: &str, subject: &str) -> Result<bool, IpcError> {
        let issuer = normalize_issuer(issuer);
        let n = self.conn.execute(
            "DELETE FROM person_identities WHERE issuer = ?1 AND subject = ?2",
            rusqlite::params![issuer, subject.trim()],
        )?;
        Ok(n > 0)
    }

    /// Record a successful sign-in.
    pub fn touch_identity_login(&self, issuer: &str, subject: &str) -> Result<(), IpcError> {
        let issuer = normalize_issuer(issuer);
        self.conn.execute(
            "UPDATE person_identities SET last_login_at = ?3 WHERE issuer = ?1 AND subject = ?2",
            rusqlite::params![issuer, subject, now_unix()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISS: &str = "https://sso.example.com/realms/acme";

    #[test]
    fn an_account_links_to_one_person_and_the_issuer_slash_does_not_matter() {
        let s = Store::open_in_memory().unwrap();
        let ada = s.create_person("ada", None).unwrap();
        let bob = s.create_person("bob", None).unwrap();
        let row = s
            .link_identity(&format!("{ISS}/"), "sub-1", ada.id)
            .unwrap();
        assert_eq!(row.issuer, ISS);
        assert_eq!(
            s.get_identity(ISS, "sub-1").unwrap().unwrap().person_id,
            ada.id
        );
        // Idempotent for the same person, refused for another.
        s.link_identity(ISS, "sub-1", ada.id).unwrap();
        let err = s.link_identity(ISS, "sub-1", bob.id).unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert_eq!(s.list_identities(ada.id).unwrap().len(), 1);
        assert!(s.list_identities(bob.id).unwrap().is_empty());
    }

    #[test]
    fn a_disabled_or_missing_person_cannot_be_linked() {
        let s = Store::open_in_memory().unwrap();
        let ada = s.create_person("ada", None).unwrap();
        s.disable_person(ada.id).unwrap();
        assert_eq!(
            s.link_identity(ISS, "sub-1", ada.id).unwrap_err().code,
            codes::E_VALIDATE
        );
        assert_eq!(
            s.link_identity(ISS, "sub-1", 9_999).unwrap_err().code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn unlink_frees_the_account_and_a_login_is_recorded() {
        let s = Store::open_in_memory().unwrap();
        let ada = s.create_person("ada", None).unwrap();
        let bob = s.create_person("bob", None).unwrap();
        s.link_identity(ISS, "sub-1", ada.id).unwrap();
        s.touch_identity_login(ISS, "sub-1").unwrap();
        assert!(s
            .get_identity(ISS, "sub-1")
            .unwrap()
            .unwrap()
            .last_login_at
            .is_some());
        assert!(s.unlink_identity(ISS, "sub-1").unwrap());
        assert!(!s.unlink_identity(ISS, "sub-1").unwrap());
        s.link_identity(ISS, "sub-1", bob.id).unwrap();
    }

    #[test]
    fn a_line_break_in_a_subject_is_refused() {
        let s = Store::open_in_memory().unwrap();
        let ada = s.create_person("ada", None).unwrap();
        assert_eq!(
            s.link_identity(ISS, "a\nb", ada.id).unwrap_err().code,
            codes::E_VALIDATE
        );
    }
}
