//! The New session picker's per-project choices (project picker spec v2):
//! pinned, visibility (hide | keep) and the picker group. Keyed by
//! `owner`/`repo` TEXT, never `project_id` — project rows are deleted and
//! re-created, their ids re-derived (review C22, migration 050).

use super::*;
use crate::ipc_error::{codes, IpcError};

/// One project's picker choices. `serde(default)`: this row crosses the
/// hub wire.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectPickRow {
    pub owner: String,
    pub repo: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub vis: Option<String>,
    #[serde(default)]
    pub grp: Option<String>,
}

/// The values `project_picks.vis` takes (the migration's CHECK).
pub const PROJECT_VIS: [&str; 2] = ["hide", "keep"];
/// A picker group's name, at most.
pub const PROJECT_GROUP_MAX_CHARS: usize = 40;

const PICKS_SELECT: &str = "SELECT p.owner, p.repo, COALESCE(k.pinned, 0), k.vis, k.grp
   FROM projects p
   LEFT JOIN project_picks k ON k.owner = p.owner AND k.repo = p.repo
  WHERE p.system = 0";

fn map_pick_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectPickRow> {
    Ok(ProjectPickRow {
        owner: r.get(0)?,
        repo: r.get(1)?,
        pinned: r.get::<_, i64>(2)? != 0,
        vis: r.get(3)?,
        grp: r.get(4)?,
    })
}

impl Store {
    /// Every non-system project's picker choices, by owner then repo.
    pub fn list_project_picks(&self) -> Result<Vec<ProjectPickRow>, IpcError> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{PICKS_SELECT} ORDER BY p.owner, p.repo"))?;
        let rows = stmt.query_map([], map_pick_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Replace one project's choices (full replace; the empty state deletes
    /// the row) and return them. `E_INVALID` for a `vis` outside
    /// [`PROJECT_VIS`] or a group over [`PROJECT_GROUP_MAX_CHARS`];
    /// `E_NOTFOUND` for a project fleet does not know. A blank group clears.
    pub fn set_project_pick(
        &self,
        owner: &str,
        repo: &str,
        pinned: bool,
        vis: Option<&str>,
        grp: Option<&str>,
        now: i64,
    ) -> Result<ProjectPickRow, IpcError> {
        if let Some(v) = vis {
            if !PROJECT_VIS.contains(&v) {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("vis must be hide or keep, not {v:?}"),
                ));
            }
        }
        let grp = grp.map(str::trim).filter(|g| !g.is_empty());
        if let Some(g) = grp {
            if g.chars().count() > PROJECT_GROUP_MAX_CHARS {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("a group name is at most {PROJECT_GROUP_MAX_CHARS} characters"),
                ));
            }
        }
        let known: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE owner = ?1 AND repo = ?2 AND system = 0)",
            rusqlite::params![owner, repo],
            |r| r.get(0),
        )?;
        if !known {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no project {owner}/{repo}"),
            ));
        }
        if !pinned && vis.is_none() && grp.is_none() {
            self.conn.execute(
                "DELETE FROM project_picks WHERE owner = ?1 AND repo = ?2",
                rusqlite::params![owner, repo],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO project_picks (owner, repo, pinned, vis, grp, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(owner, repo) DO UPDATE SET
                   pinned = excluded.pinned, vis = excluded.vis, grp = excluded.grp,
                   updated_at = excluded.updated_at",
                rusqlite::params![owner, repo, pinned as i64, vis, grp, now],
            )?;
        }
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{PICKS_SELECT} AND p.owner = ?1 AND p.repo = ?2"))?;
        Ok(stmt.query_row(rusqlite::params![owner, repo], map_pick_row)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn store_with(projects: &[(&str, &str)]) -> Store {
        let s = Store::open_in_memory().unwrap();
        for (o, r) in projects {
            s.upsert_project(o, r, &format!("/p/{o}/{r}")).unwrap();
        }
        s
    }

    #[test]
    fn every_non_system_project_is_listed_with_empty_state() {
        let s = store_with(&[("o", "a"), ("o", "b")]);
        s.upsert_system_project("fleet", "operator", "/op").unwrap();
        let rows = s.list_project_picks().unwrap();
        let names: Vec<_> = rows
            .iter()
            .map(|r| format!("{}/{}", r.owner, r.repo))
            .collect();
        assert_eq!(names, ["o/a", "o/b"]);
        assert!(rows
            .iter()
            .all(|r| !r.pinned && r.vis.is_none() && r.grp.is_none()));
    }

    #[test]
    fn set_round_trips_and_the_empty_state_deletes_the_row() {
        let s = store_with(&[("o", "a")]);
        let row = s
            .set_project_pick("o", "a", true, Some("keep"), Some("  tools "), NOW)
            .unwrap();
        assert!(row.pinned);
        assert_eq!(row.vis.as_deref(), Some("keep"));
        assert_eq!(row.grp.as_deref(), Some("tools"), "trimmed");
        let row = s
            .set_project_pick("o", "a", false, None, None, NOW)
            .unwrap();
        assert_eq!((row.pinned, row.vis, row.grp), (false, None, None));
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM project_picks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn pinned_and_visibility_are_independent() {
        let s = store_with(&[("o", "a")]);
        s.set_project_pick("o", "a", true, Some("hide"), None, NOW)
            .unwrap();
        let row = s
            .set_project_pick("o", "a", false, Some("hide"), None, NOW)
            .unwrap();
        assert_eq!(
            row.vis.as_deref(),
            Some("hide"),
            "unpinning never touches visibility"
        );
    }

    #[test]
    fn set_validates_vis_group_and_project() {
        let s = store_with(&[("o", "a")]);
        let e = s
            .set_project_pick("o", "a", false, Some("star"), None, NOW)
            .unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        let long = "x".repeat(PROJECT_GROUP_MAX_CHARS + 1);
        let e = s
            .set_project_pick("o", "a", false, None, Some(&long), NOW)
            .unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
        let e = s
            .set_project_pick("o", "nope", true, None, None, NOW)
            .unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
        let row = s
            .set_project_pick("o", "a", true, None, Some("   "), NOW)
            .unwrap();
        assert_eq!(row.grp, None, "a blank group clears");
    }

    #[test]
    fn a_pick_survives_the_project_row_being_recreated() {
        let s = store_with(&[("o", "a")]);
        s.set_project_pick("o", "a", true, None, None, NOW).unwrap();
        s.conn.execute("DELETE FROM projects", []).unwrap();
        s.upsert_project("o", "a", "/p/o/a").unwrap();
        assert!(s.list_project_picks().unwrap()[0].pinned);
    }
}
