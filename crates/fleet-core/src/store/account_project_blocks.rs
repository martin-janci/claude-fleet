//! An account forbidden for a project (migration 217,
//! `account_project_blocks`) or for every project of an org (migration 218,
//! `account_org_blocks`). The rules that read them are
//! `service::project_accounts`; the account's own row carries both lists
//! ([`super::AccountRow::forbidden_project_ids`],
//! [`super::AccountRow::forbidden_org_ids`]).

use super::{org_of_session, AccountRow, SessionOrgFacts, Store};
use crate::events::EventBus;
use crate::ipc_error::{codes, IpcError};

/// At most this many projects (and, separately, orgs) per account: a list a
/// person picks by hand.
pub const MAX_FORBIDDEN_PROJECTS: usize = 500;

impl Store {
    /// The accounts project `project_id` may not run on, ascending: those
    /// forbidden for the project itself and those forbidden for its org
    /// ([`Store::project_org`]). Empty for a project with none (the usual
    /// case) or no such project.
    pub fn project_forbidden_accounts(&self, project_id: i64) -> Result<Vec<String>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT account_uuid FROM account_project_blocks WHERE project_id = ?1",
        )?;
        let mut rows = stmt
            .query_map(rusqlite::params![project_id], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        if self.any_account_org_block()? {
            if let Some(org) = self.project_org(project_id)? {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT account_uuid FROM account_org_blocks WHERE org_id = ?1",
                )?;
                rows.extend(
                    stmt.query_map(rusqlite::params![org], |r| r.get(0))?
                        .collect::<Result<Vec<String>, _>>()?,
                );
            }
        }
        rows.sort_unstable();
        rows.dedup();
        Ok(rows)
    }

    /// The org project `project_id` belongs to by its own rules: the most
    /// specific org rule on its owner, repo or path. A host-only rule and a
    /// host's own org name a host, not the project, so they do not count.
    /// `None` for no such project or no matching rule.
    pub fn project_org(&self, project_id: i64) -> Result<Option<i64>, IpcError> {
        let Some(p) = self.get_project(project_id)? else {
            return Ok(None);
        };
        let facts = SessionOrgFacts {
            host_alias: "",
            owner: Some(&p.owner),
            repo: Some(&p.repo),
            path: Some(&p.base_path),
        };
        let rules: Vec<_> = self
            .list_org_rules()?
            .into_iter()
            .filter(|r| r.host_alias.is_none())
            .collect();
        Ok(org_of_session(&facts, &rules, None))
    }

    /// Whether any account is forbidden for any project or org: the cheap
    /// first read of a pass that checks many sessions.
    pub fn any_account_project_block(&self) -> Result<bool, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM account_project_blocks) \
                 OR EXISTS (SELECT 1 FROM account_org_blocks)",
        )?;
        Ok(stmt.query_row([], |r| r.get::<_, bool>(0))?)
    }

    fn any_account_org_block(&self) -> Result<bool, IpcError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM account_org_blocks)")?;
        Ok(stmt.query_row([], |r| r.get::<_, bool>(0))?)
    }

    /// Replace account `uuid`'s forbidden projects with `project_ids`
    /// (duplicates dropped) and answer its row, announced as
    /// `account_upserted`. [`Store::set_account_blocks`] with the org list
    /// left alone.
    pub fn set_account_forbidden_projects(
        &self,
        uuid: &str,
        project_ids: &[i64],
    ) -> Result<AccountRow, IpcError> {
        self.set_account_blocks(uuid, Some(project_ids), None)
    }

    /// Replace account `uuid`'s forbidden projects and/or orgs (`None`
    /// leaves that list alone; duplicates dropped) and answer its row,
    /// announced as `account_upserted`. `E_NOTFOUND` for an unknown account,
    /// project or org, `E_INVALID` past [`MAX_FORBIDDEN_PROJECTS`] in either
    /// list. One savepoint: a refused call changes nothing.
    pub fn set_account_blocks(
        &self,
        uuid: &str,
        project_ids: Option<&[i64]>,
        org_ids: Option<&[i64]>,
    ) -> Result<AccountRow, IpcError> {
        let clean = |ids: Option<&[i64]>| -> Result<Option<Vec<i64>>, IpcError> {
            let Some(ids) = ids else { return Ok(None) };
            let mut ids = ids.to_vec();
            ids.sort_unstable();
            ids.dedup();
            if ids.len() > MAX_FORBIDDEN_PROJECTS {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("at most {MAX_FORBIDDEN_PROJECTS} projects or orgs per account"),
                ));
            }
            Ok(Some(ids))
        };
        let projects = clean(project_ids)?;
        let orgs = clean(org_ids)?;
        if self.get_account_by_uuid(uuid)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("account {uuid} not found"),
            ));
        }
        let now = super::now_unix();
        self.in_savepoint("set_account_blocks", |conn| {
            // (table, its id column, the table the id names, its word)
            let lists = [
                (
                    projects.as_ref(),
                    "account_project_blocks",
                    "project_id",
                    "projects",
                    "project",
                ),
                (orgs.as_ref(), "account_org_blocks", "org_id", "orgs", "org"),
            ];
            for (ids, table, column, parent, word) in lists {
                let Some(ids) = ids else { continue };
                for id in ids {
                    let known: bool = conn.query_row(
                        &format!("SELECT EXISTS (SELECT 1 FROM {parent} WHERE id = ?1)"),
                        rusqlite::params![id],
                        |r| r.get(0),
                    )?;
                    if !known {
                        return Err(IpcError::new(
                            codes::E_NOTFOUND,
                            format!("{word} {id} not found"),
                        ));
                    }
                }
                conn.execute(
                    &format!("DELETE FROM {table} WHERE account_uuid = ?1"),
                    rusqlite::params![uuid],
                )?;
                for id in ids {
                    conn.execute(
                        &format!(
                            "INSERT INTO {table} (account_uuid, {column}, created_at) \
                             VALUES (?1, ?2, ?3)"
                        ),
                        rusqlite::params![uuid, id, now],
                    )?;
                }
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
    use crate::store::OrgRuleRow;

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

    #[test]
    fn an_org_block_covers_every_project_of_the_org_now_and_later() {
        let s = Store::open_in_memory().unwrap();
        account(&s, "u1");
        account(&s, "u2");
        let api = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
        let other = s.upsert_project("zeta", "web", "/srv/zeta/web").unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        s.add_org_rule(OrgRuleRow {
            id: 0,
            org_id: org,
            owner: Some("acme".into()),
            repo: None,
            path_prefix: None,
            host_alias: None,
        })
        .unwrap();
        assert_eq!(s.project_org(api).unwrap(), Some(org));
        assert_eq!(s.project_org(other).unwrap(), None);

        let row = s.set_account_blocks("u1", None, Some(&[org, org])).unwrap();
        assert_eq!(row.forbidden_org_ids, vec![org]);
        assert!(row.forbidden_project_ids.is_empty());
        assert!(s.any_account_project_block().unwrap());
        assert_eq!(s.project_forbidden_accounts(api).unwrap(), vec!["u1"]);
        assert!(s.project_forbidden_accounts(other).unwrap().is_empty());
        // A project added later is covered too.
        let later = s.upsert_project("acme", "new", "/srv/acme/new").unwrap();
        assert_eq!(s.project_forbidden_accounts(later).unwrap(), vec!["u1"]);
        // Both lists at once, without duplicates.
        s.set_account_forbidden_projects("u1", &[api]).unwrap();
        s.set_account_forbidden_projects("u2", &[api]).unwrap();
        assert_eq!(s.project_forbidden_accounts(api).unwrap(), vec!["u1", "u2"]);
        // Setting projects left the org list alone.
        assert_eq!(
            s.get_account_by_uuid("u1")
                .unwrap()
                .unwrap()
                .forbidden_org_ids,
            vec![org]
        );
        // An unknown org changes nothing.
        let err = s
            .set_account_blocks("u1", Some(&[]), Some(&[org + 99]))
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert_eq!(s.project_forbidden_accounts(api).unwrap(), vec!["u1", "u2"]);
        // A removed org takes its blocks with it.
        s.remove_org(org).unwrap();
        assert!(s
            .get_account_by_uuid("u1")
            .unwrap()
            .unwrap()
            .forbidden_org_ids
            .is_empty());
    }
}
