//! An org's project catalog (M15 step G2.10, migration 158): org-wide
//! projects with their remote, where they are checked out, and the hosts
//! they may run on. Settings → Organisations lists them under What belongs
//! and adds or removes one; nothing else writes them.

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

/// Longest project name.
pub const ORG_PROJECT_NAME_MAX_CHARS: usize = 80;
/// Longest remote or path.
const ORG_PROJECT_FIELD_MAX_CHARS: usize = 1024;

/// One catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OrgProjectRow {
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The hosts it may run on; empty = every host of the org.
    #[serde(default)]
    pub hosts: Vec<String>,
    pub created_at: i64,
}

/// What a new entry says, as a person typed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewOrgProject<'a> {
    pub name: &'a str,
    pub remote: Option<&'a str>,
    pub path: Option<&'a str>,
    /// Comma- or space-separated host aliases; empty = every host of the org.
    pub hosts: Option<&'a str>,
}

fn map_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<OrgProjectRow> {
    let hosts: String = r.get(5)?;
    Ok(OrgProjectRow {
        id: r.get(0)?,
        org_id: r.get(1)?,
        name: r.get(2)?,
        remote: r.get(3)?,
        path: r.get(4)?,
        hosts: split_hosts(&hosts),
        created_at: r.get(6)?,
    })
}

const PROJECT_COLUMNS: &str = "id, org_id, name, remote, path, hosts, created_at";

/// `a, b c` → `["a", "b", "c"]`, without repeats.
fn split_hosts(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let h = h.trim();
        if !h.is_empty() && !out.iter().any(|x| x == h) {
            out.push(h.to_string());
        }
    }
    out
}

/// An optional one-line field: trimmed, empty → `None`.
fn optional_field(v: Option<&str>, what: &str) -> Result<Option<String>, IpcError> {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > ORG_PROJECT_FIELD_MAX_CHARS || v.chars().any(char::is_control) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "a project's {what} is at most {ORG_PROJECT_FIELD_MAX_CHARS} characters, one line"
            ),
        ));
    }
    Ok(Some(v.to_string()))
}

impl Store {
    /// The org's catalog, by name.
    pub fn org_projects(&self, org: i64) -> Result<Vec<OrgProjectRow>, IpcError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {PROJECT_COLUMNS} FROM org_projects WHERE org_id = ?1 \
              ORDER BY name COLLATE NOCASE, id"
        ))?;
        let rows = st.query_map([org], map_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_org_project(&self, id: i64) -> Result<Option<OrgProjectRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PROJECT_COLUMNS} FROM org_projects WHERE id = ?1"),
                [id],
                map_project,
            )
            .optional()?)
    }

    /// Add an entry. `E_NOTFOUND` for an unknown org or host, `E_EXISTS` for
    /// a name the org already has (without case), `E_INVALID` for an empty
    /// or overlong field.
    pub fn add_org_project(
        &self,
        org: i64,
        p: &NewOrgProject<'_>,
    ) -> Result<OrgProjectRow, IpcError> {
        if self.get_org(org)?.is_none() {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("org {org} not found"),
            ));
        }
        let name = p.name.trim();
        if name.is_empty()
            || name.chars().count() > ORG_PROJECT_NAME_MAX_CHARS
            || name.chars().any(char::is_control)
        {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("a project name is 1–{ORG_PROJECT_NAME_MAX_CHARS} characters, one line"),
            ));
        }
        let remote = optional_field(p.remote, "remote")?;
        let path = optional_field(p.path, "path")?;
        let hosts = split_hosts(p.hosts.unwrap_or(""));
        for h in &hosts {
            let known: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM hosts WHERE alias = ?1)",
                [h],
                |r| r.get(0),
            )?;
            if !known {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("no host named {h:?}"),
                ));
            }
        }
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM org_projects WHERE org_id = ?1 AND lower(name) = lower(?2))",
            rusqlite::params![org, name],
            |r| r.get(0),
        )?;
        if taken {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("the org already has a project named {name:?}"),
            ));
        }
        self.conn.execute(
            "INSERT INTO org_projects (org_id, name, remote, path, hosts, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![org, name, remote, path, hosts.join(","), now_unix()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_org_project(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "project vanished"))
    }

    /// Remove an entry; `false` when there was none.
    pub fn remove_org_project(&self, id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .execute("DELETE FROM org_projects WHERE id = ?1", [id])?
            > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_org() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        (s, org)
    }

    #[test]
    fn an_entry_keeps_its_remote_path_and_hosts() {
        let (s, org) = store_with_org();
        s.upsert_host("box-a").unwrap();
        s.upsert_host("box-b").unwrap();
        let p = s
            .add_org_project(
                org,
                &NewOrgProject {
                    name: " api ",
                    remote: Some("git@github.com:acme/api.git"),
                    path: Some("~/src/api"),
                    hosts: Some("box-a, box-b box-a"),
                },
            )
            .unwrap();
        assert_eq!(p.name, "api");
        assert_eq!(p.remote.as_deref(), Some("git@github.com:acme/api.git"));
        assert_eq!(p.path.as_deref(), Some("~/src/api"));
        assert_eq!(p.hosts, vec!["box-a", "box-b"]);
        assert_eq!(s.org_projects(org).unwrap(), vec![p.clone()]);
        assert!(s.remove_org_project(p.id).unwrap());
        assert!(s.org_projects(org).unwrap().is_empty());
        assert!(!s.remove_org_project(p.id).unwrap());
    }

    #[test]
    fn a_name_is_unique_within_its_org_and_hosts_must_exist() {
        let (s, org) = store_with_org();
        let new = |name| NewOrgProject {
            name,
            ..Default::default()
        };
        s.add_org_project(org, &new("Api")).unwrap();
        assert_eq!(
            s.add_org_project(org, &new("api")).unwrap_err().code,
            codes::E_EXISTS
        );
        assert_eq!(
            s.add_org_project(org, &new("  ")).unwrap_err().code,
            codes::E_INVALID
        );
        let other = s.add_org("Other", None, false).unwrap().id;
        s.add_org_project(other, &new("api")).unwrap();
        let ghost = NewOrgProject {
            name: "web",
            hosts: Some("nowhere"),
            ..Default::default()
        };
        assert_eq!(
            s.add_org_project(org, &ghost).unwrap_err().code,
            codes::E_NOTFOUND
        );
        assert_eq!(
            s.add_org_project(999, &new("x")).unwrap_err().code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn removing_the_org_removes_its_catalog() {
        let (s, org) = store_with_org();
        s.add_org_project(
            org,
            &NewOrgProject {
                name: "api",
                ..Default::default()
            },
        )
        .unwrap();
        s.remove_org(org).unwrap();
        let left: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM org_projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }
}
