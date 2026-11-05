//! An account forbidden for a project (migration 217):
//! `account_project_blocks`. The rules that read it are
//! `service::project_accounts`; the account's own row carries its list
//! ([`super::AccountRow::forbidden_project_ids`]).

use super::{AccountRow, Store};
use crate::ipc_error::{codes, IpcError};

/// At most this many projects per account: a list a person picks by hand.
pub const MAX_FORBIDDEN_PROJECTS: usize = 500;

impl Store {
    /// The accounts project `project_id` may not run on, ascending. Empty
    /// for a project with none (the usual case) or no such project.
    pub fn project_forbidden_accounts(&self, project_id: i64) -> Result<Vec<String>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT account_uuid FROM account_project_blocks \
              WHERE project_id = ?1 ORDER BY account_uuid",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![project_id], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    /// Whether any account is forbidden for any project: the cheap first
    /// read of a pass that checks many sessions.
    pub fn any_account_project_block(&self) -> Result<bool, IpcError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM account_project_blocks)")?;
        Ok(stmt.query_row([], |r| r.get::<_, bool>(0))?)
    }

    /// Replace account `uuid`'s forbidden projects with `project_ids`
    /// (duplicates dropped) and answer its row, announced as
    /// `account_upserted`. `E_NOTFOUND` for an unknown account or project,
    /// `E_INVALID` past [`MAX_FORBIDDEN_PROJECTS`]. One savepoint: a refused
    /// call changes nothing.
    pub fn set_account_forbidden_projects(
        &self,
        uuid: &str,
        project_ids: &[i64],
    ) -> Result<AccountRow, IpcError> {
        let mut ids = project_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() > MAX_FORBIDDEN_PROJECTS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("at most {MAX_FORBIDDEN_PROJECTS} projects per account"),
            ));
        }
        if self.get_account_by_uuid(uuid)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("account {uuid} not found"),
            ));
        }
        let now = super::now_unix();
        self.in_savepoint("set_account_forbidden_projects", |conn| {
            for id in &ids {
                let known: bool = conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)",
                    rusqlite::params![id],
                    |r| r.get(0),
                )?;
                if !known {
                    return Err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("project {id} not found"),
                    ));
                }
            }
            conn.execute(
                "DELETE FROM account_project_blocks WHERE account_uuid = ?1",
                rusqlite::params![uuid],
            )?;
            for id in &ids {
                conn.execute(
                    "INSERT INTO account_project_blocks (account_uuid, project_id, created_at) \
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![uuid, id, now],
                )?;
            }
            Ok::<(), IpcError>(())
        })?;
        let row = self
            .get_account_by_uuid(uuid)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, format!("account {uuid} vanished")))?;
        self.bus.account_upserted(&row);
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(s: &Store, uuid: &str) {
        s.upsert_account(&AccountRow {
            uuid: uuid.into(),
            email: Some(format!("{uuid}@example.com")),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn an_account_s_forbidden_projects_are_set_read_and_replaced() {
        let s = Store::open_in_memory().unwrap();
        account(&s, "u1");
        account(&s, "u2");
        let api = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
        let web = s.upsert_project("acme", "web", "/srv/acme/web").unwrap();
        assert!(!s.any_account_project_block().unwrap());

        let row = s
            .set_account_forbidden_projects("u1", &[web, api, web])
            .unwrap();
        let mut both = vec![api, web];
        both.sort_unstable();
        assert_eq!(row.forbidden_project_ids, both);
        assert!(s.any_account_project_block().unwrap());
        assert_eq!(s.project_forbidden_accounts(api).unwrap(), vec!["u1"]);
        // The list rides on every read of the row.
        let listed = s.list_accounts().unwrap();
        assert_eq!(listed[0].forbidden_project_ids, both);
        assert!(listed[1].forbidden_project_ids.is_empty());

        // Replaced, not added to.
        let row = s.set_account_forbidden_projects("u1", &[api]).unwrap();
        assert_eq!(row.forbidden_project_ids, vec![api]);
        assert!(s.project_forbidden_accounts(web).unwrap().is_empty());
        // A probe of the account does not clear it.
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: Some("new@example.com".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            s.get_account_by_uuid("u1")
                .unwrap()
                .unwrap()
                .forbidden_project_ids,
            vec![api]
        );
        // Empty clears.
        s.set_account_forbidden_projects("u1", &[]).unwrap();
        assert!(!s.any_account_project_block().unwrap());
    }

    #[test]
    fn an_unknown_account_or_project_changes_nothing() {
        let s = Store::open_in_memory().unwrap();
        account(&s, "u1");
        let api = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
        s.set_account_forbidden_projects("u1", &[api]).unwrap();
        let err = s
            .set_account_forbidden_projects("nobody", &[api])
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        let err = s
            .set_account_forbidden_projects("u1", &[api + 99])
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        // The refused call left the old list alone.
        assert_eq!(s.project_forbidden_accounts(api).unwrap(), vec!["u1"]);
    }

    #[test]
    fn a_removed_project_takes_its_blocks_with_it() {
        let s = Store::open_in_memory().unwrap();
        account(&s, "u1");
        let api = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
        s.set_account_forbidden_projects("u1", &[api]).unwrap();
        s.conn
            .execute("DELETE FROM projects WHERE id = ?1", [api])
            .unwrap();
        assert!(s.project_forbidden_accounts(api).unwrap().is_empty());
        assert!(s
            .get_account_by_uuid("u1")
            .unwrap()
            .unwrap()
            .forbidden_project_ids
            .is_empty());
    }
}
