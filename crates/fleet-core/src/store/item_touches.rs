//! The paths a task will change (lanes design 2026-10-23 §5.4, L5,
//! migration 211): `work_items.touches_json`, a list of repository-relative
//! paths or globs the planner names with `create_item`. The mission loop
//! reads it to keep work that would collide from running side by side
//! (`orchestrate::touches`); it is read apart from [`super::WorkItemRow`].

use super::Store;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

/// The most paths one task names.
pub const TOUCHES_MAX: usize = 20;
/// The longest one path.
pub const TOUCH_MAX_CHARS: usize = 200;

/// PURE: a task's touches as stored: trimmed, `./` dropped, duplicates
/// and blanks left out. Refused: an absolute path, `..`, a control
/// character, more than [`TOUCHES_MAX`] or one longer than
/// [`TOUCH_MAX_CHARS`].
pub fn normalize_touches(raw: &[String]) -> Result<Vec<String>, IpcError> {
    let bad = |why: String| IpcError::new(codes::E_INVALID, format!("touches: {why}"));
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        let t = r.trim().trim_start_matches("./").to_string();
        if t.is_empty() {
            continue;
        }
        if t.chars().count() > TOUCH_MAX_CHARS || t.chars().any(char::is_control) {
            return Err(bad(format!(
                "a path is one line of at most {TOUCH_MAX_CHARS} characters"
            )));
        }
        if t.starts_with('/') || t.split('/').any(|seg| seg == "..") {
            return Err(bad(format!("{t:?} is not a path inside the repository")));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.len() > TOUCHES_MAX {
        return Err(bad(format!("at most {TOUCHES_MAX} paths")));
    }
    Ok(out)
}

/// The part of a path or glob that is certain: up to its first wildcard,
/// without a trailing `/`. `""` stands for the whole repository.
fn certain_prefix(t: &str) -> &str {
    let cut = t.find(['*', '?', '[', '{']).unwrap_or(t.len());
    t[..cut].trim_end_matches('/')
}

/// PURE: whether two tasks' touches may meet. Conservative: two paths
/// meet when one's certain prefix (before any wildcard) holds the other's,
/// segment by segment, so `src/store/**` meets `src/store/mod.rs` and
/// `src/*.rs` meets `src/main.rs`, while `src/ui` and `src/store` do not.
/// An empty list says nothing and meets nothing.
pub fn touches_overlap(a: &[String], b: &[String]) -> bool {
    let within = |p: &str, q: &str| p.is_empty() || q == p || q.starts_with(&format!("{p}/"));
    a.iter().any(|x| {
        let p = certain_prefix(x);
        b.iter().any(|y| {
            let q = certain_prefix(y);
            within(p, q) || within(q, p)
        })
    })
}

impl Store {
    /// Set what `item_id` will change ([`normalize_touches`]); an empty
    /// list clears it.
    pub fn set_item_touches(&self, item_id: i64, touches: &[String]) -> Result<(), IpcError> {
        let t = normalize_touches(touches)?;
        let json = if t.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&t)
                    .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?,
            )
        };
        self.conn.execute(
            "UPDATE work_items SET touches_json = ?1 WHERE id = ?2",
            rusqlite::params![json, item_id],
        )?;
        Ok(())
    }

    /// What `item_id` will change; empty when nobody said.
    pub fn item_touches(&self, item_id: i64) -> Result<Vec<String>, IpcError> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT touches_json FROM work_items WHERE id = ?1",
                [item_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(json
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default())
    }

    /// The touches of mission `id`'s members that name any, by item.
    pub fn mission_touches(&self, id: i64) -> Result<HashMap<i64, Vec<String>>, IpcError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, touches_json FROM work_items \
             WHERE orchestration_project_id = ?1 AND touches_json IS NOT NULL",
        )?;
        let rows = stmt.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (item, json) = row?;
            if let Ok(t) = serde_json::from_str::<Vec<String>>(&json) {
                if !t.is_empty() {
                    out.insert(item, t);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn paths_meet_when_one_holds_the_other() {
        assert!(touches_overlap(
            &v(&["src/store/**"]),
            &v(&["src/store/mod.rs"])
        ));
        assert!(touches_overlap(&v(&["src/*.rs"]), &v(&["src/main.rs"])));
        assert!(touches_overlap(
            &v(&["docs/status.md"]),
            &v(&["docs/status.md"])
        ));
        assert!(
            touches_overlap(&v(&["**/*.sql"]), &v(&["src/ui"])),
            "a root glob meets all"
        );
        assert!(!touches_overlap(&v(&["src/ui"]), &v(&["src/store"])));
        assert!(
            !touches_overlap(&v(&["src/store"]), &v(&["src/storefront"])),
            "by segment"
        );
        assert!(
            !touches_overlap(&[], &v(&["src"])),
            "nothing said meets nothing"
        );
    }

    #[test]
    fn touches_are_paths_inside_the_repository() {
        assert_eq!(
            normalize_touches(&v(&[" ./src/a.rs ", "", "src/a.rs", "docs/**"])).unwrap(),
            v(&["src/a.rs", "docs/**"])
        );
        assert!(normalize_touches(&v(&["/etc/passwd"])).is_err());
        assert!(normalize_touches(&v(&["src/../../x"])).is_err());
        assert!(normalize_touches(&v(&["a\nb"])).is_err());
        let many: Vec<String> = (0..21).map(|i| format!("f{i}")).collect();
        assert!(normalize_touches(&many).is_err());
    }

    #[test]
    fn a_tasks_touches_are_kept_and_cleared() {
        let s = Store::open_in_memory().unwrap();
        let a = s.create_local_work_item(None, "a").unwrap().id;
        assert!(s.item_touches(a).unwrap().is_empty());
        s.set_item_touches(a, &v(&["src/store/**"])).unwrap();
        assert_eq!(s.item_touches(a).unwrap(), v(&["src/store/**"]));
        s.set_item_touches(a, &[]).unwrap();
        assert!(s.item_touches(a).unwrap().is_empty());
    }
}
