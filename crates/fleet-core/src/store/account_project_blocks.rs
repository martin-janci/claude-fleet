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
        self.forbidden_accounts_of(project_id, None)
    }

    /// [`Store::project_forbidden_accounts`] for work on `host_alias`: also
    /// the accounts forbidden for the org a session there would be filed
    /// under ([`Store::project_orgs_on`]). Every caller that knows the host
    /// asks this one, so an org that is a host (`hosts.org_id`, a
    /// host-qualified rule) is blocked like one that is an owner.
    pub fn project_forbidden_accounts_on(
        &self,
        project_id: i64,
        host_alias: &str,
    ) -> Result<Vec<String>, IpcError> {
        self.forbidden_accounts_of(project_id, Some(host_alias))
    }

    fn forbidden_accounts_of(
        &self,
        project_id: i64,
        host_alias: Option<&str>,
    ) -> Result<Vec<String>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT account_uuid FROM account_project_blocks WHERE project_id = ?1",
        )?;
        let mut rows = stmt
            .query_map(rusqlite::params![project_id], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        if self.any_account_org_block()? {
            let orgs = match host_alias {
                Some(h) => self.project_orgs_on(project_id, h)?,
                None => self.project_org(project_id)?.into_iter().collect(),
            };
            for org in orgs {
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

    /// The orgs work in `project_id` on `host_alias` belongs to, for an org
    /// block: the project's own ([`Store::project_org`]) and the one a
    /// session started there now would be filed under
    /// ([`Store::org_for_new_session`]: host-qualified rules and the host's
    /// own org count). Usually one org or none; two when a host-qualified
    /// rule outranks the project's own.
    pub fn project_orgs_on(&self, project_id: i64, host_alias: &str) -> Result<Vec<i64>, IpcError> {
        let mut orgs: Vec<i64> = self.project_org(project_id)?.into_iter().collect();
        if let Some(o) = self.org_for_new_session(host_alias, project_id)? {
            if !orgs.contains(&o) {
                orgs.push(o);
            }
        }
        Ok(orgs)
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

    /// The accounts forbidden for project `project_id` by name (not through
    /// its org): read before the project row and its blocks are deleted, so
    /// [`Store::announce_accounts`] can tell clients afterwards.
    pub(super) fn accounts_blocked_for_project(
        &self,
        project_id: i64,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT account_uuid FROM account_project_blocks WHERE project_id = ?1 \
             ORDER BY account_uuid",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id], |r| r.get(0))?;
        rows.collect()
    }

    /// [`Store::accounts_blocked_for_project`] for an org.
    pub(super) fn accounts_blocked_for_org(
        &self,
        org_id: i64,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT account_uuid FROM account_org_blocks WHERE org_id = ?1 \
             ORDER BY account_uuid",
        )?;
        let rows = stmt.query_map(rusqlite::params![org_id], |r| r.get(0))?;
        rows.collect()
    }

    /// `account_upserted` for each of `uuids`, re-read: after a deleted
    /// project or org took its blocks with it, so a client's copy of the
    /// account drops the dead id instead of sending it back on its next
    /// edit. Best effort: the delete already committed.
    pub(super) fn announce_accounts(&self, uuids: &[String]) {
        for uuid in uuids {
            if let Ok(Some(row)) = self.get_account_by_uuid(uuid) {
                self.bus.account_upserted(&row);
            }
        }
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

    /// Review finding 8: a deleted project or org takes its block rows with
    /// it, and each account that lost one is announced so no client keeps
    /// the dead id on its copy.
    #[test]
    fn a_deleted_project_or_org_announces_the_accounts_it_unblocked() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        account(&s, "u1");
        account(&s, "u2");
        let api = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        s.set_account_blocks("u1", Some(&[api]), Some(&[org]))
            .unwrap();
        bus.take();

        s.delete_project(api, &crate::store::FingerprintKeys::new())
            .unwrap();
        let events = bus.take();
        assert!(
            events.contains(&"account:upserted:u1".to_string()),
            "{events:?}"
        );
        assert!(!events.contains(&"account:upserted:u2".to_string()));
        assert!(s
            .get_account_by_uuid("u1")
            .unwrap()
            .unwrap()
            .forbidden_project_ids
            .is_empty());

        assert!(s.remove_org(org).unwrap());
        let events = bus.take();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.as_str() == "account:upserted:u1")
                .count(),
            1,
            "{events:?}"
        );
    }

    /// Review finding 6: an org a project reaches only through its host
    /// (the host's own org, a host-qualified rule) is blocked on that host.
    #[test]
    fn an_org_block_reaches_a_project_through_the_host_it_runs_on() {
        let s = Store::open_in_memory().unwrap();
        account(&s, "u1");
        s.upsert_host("vm1").unwrap();
        s.upsert_host("mac").unwrap();
        let api = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
        let org = s.add_org("Client", None, false).unwrap().id;
        s.add_org_rule(OrgRuleRow {
            id: 0,
            org_id: org,
            owner: None,
            repo: None,
            path_prefix: None,
            host_alias: Some("vm1".into()),
        })
        .unwrap();
        s.set_account_blocks("u1", None, Some(&[org])).unwrap();
        // Host-blind, the project has no org.
        assert_eq!(s.project_org(api).unwrap(), None);
        assert!(s.project_forbidden_accounts(api).unwrap().is_empty());
        assert_eq!(
            s.project_forbidden_accounts_on(api, "vm1").unwrap(),
            vec!["u1"]
        );
        assert!(s
            .project_forbidden_accounts_on(api, "mac")
            .unwrap()
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
