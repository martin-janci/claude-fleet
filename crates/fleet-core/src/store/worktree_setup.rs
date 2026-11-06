//! A project's new worktree setup (migration 221): `projects.setup_copy`
//! and `projects.setup_command`, read apart from [`super::ProjectRow`].
//! What a start does with them is `service::sessions::worktree_setup`.

use super::Store;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// At most this many copy globs per project.
pub const SETUP_GLOBS_MAX: usize = 32;
/// A glob or a command longer than this is refused.
const SETUP_TEXT_MAX: usize = 1024;

/// What a new worktree of a project gets before its agent starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetup {
    /// Relative globs copied from the base checkout, in the order set.
    #[serde(default)]
    pub copy: Vec<String>,
    /// One shell command run in the new worktree after the copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl WorktreeSetup {
    /// Nothing to copy and nothing to run: a start skips the step.
    pub fn is_empty(&self) -> bool {
        self.copy.is_empty() && self.command.is_none()
    }
}

/// Check one copy glob: relative to the checkout, inside it, one line.
/// Returns the trimmed glob.
pub fn check_setup_glob(glob: &str) -> Result<String, IpcError> {
    let g = glob.trim();
    let bad = |why: &str| IpcError::new(codes::E_INVALID, format!("copy glob {g:?}: {why}"));
    if g.is_empty() {
        return Err(bad("empty"));
    }
    if g.len() > SETUP_TEXT_MAX {
        return Err(bad("too long"));
    }
    if g.chars().any(|c| c.is_control()) {
        return Err(bad("one line, no control characters"));
    }
    if g.starts_with('/') || g.starts_with('~') || g.contains('\\') {
        return Err(bad("must be relative to the project folder"));
    }
    if g.split('/').any(|part| part == "..") {
        return Err(bad("must stay inside the project folder"));
    }
    if g.split('/').any(|part| part == ".git") {
        return Err(bad("git's own files are never copied"));
    }
    Ok(g.to_string())
}

/// Check the whole setup a person typed; returns it normalised (globs
/// trimmed, blank lines and repeats dropped, a blank command as none).
pub fn check_setup(copy: &[String], command: Option<&str>) -> Result<WorktreeSetup, IpcError> {
    let mut globs: Vec<String> = Vec::new();
    for g in copy {
        if g.trim().is_empty() {
            continue;
        }
        let g = check_setup_glob(g)?;
        if !globs.contains(&g) {
            globs.push(g);
        }
    }
    if globs.len() > SETUP_GLOBS_MAX {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("at most {SETUP_GLOBS_MAX} copy globs"),
        ));
    }
    let command = command.map(str::trim).filter(|c| !c.is_empty());
    if let Some(c) = command {
        if c.len() > SETUP_TEXT_MAX {
            return Err(IpcError::new(codes::E_INVALID, "setup command is too long"));
        }
        if c.contains(['\n', '\r', '\0']) {
            return Err(IpcError::new(
                codes::E_INVALID,
                "setup command is one line; chain steps with &&",
            ));
        }
    }
    Ok(WorktreeSetup {
        copy: globs,
        command: command.map(str::to_string),
    })
}

impl Store {
    /// Project `id`'s setup; `None` when there is no such project.
    pub fn worktree_setup(&self, id: i64) -> Result<Option<WorktreeSetup>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT setup_copy, setup_command FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |r| {
                    let copy: Option<String> = r.get(0)?;
                    Ok(WorktreeSetup {
                        copy: copy
                            .unwrap_or_default()
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .map(str::to_string)
                            .collect(),
                        command: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Set project `id`'s setup, checked by [`check_setup`]. `false` when
    /// there is no such project.
    pub fn set_worktree_setup(&self, id: i64, setup: &WorktreeSetup) -> Result<bool, IpcError> {
        let setup = check_setup(&setup.copy, setup.command.as_deref())?;
        let copy = (!setup.copy.is_empty()).then(|| setup.copy.join("\n"));
        let n = self.conn.execute(
            "UPDATE projects SET setup_copy = ?2, setup_command = ?3 WHERE id = ?1",
            rusqlite::params![id, copy, setup.command],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_sets_up_nothing_until_a_person_says_what() {
        let s = Store::open_in_memory().unwrap();
        let p = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
        assert!(s.worktree_setup(p).unwrap().unwrap().is_empty());
        let typed = WorktreeSetup {
            copy: vec![
                " .env ".into(),
                "".into(),
                "config/*.local".into(),
                ".env".into(),
            ],
            command: Some("  pnpm install --frozen-lockfile ".into()),
        };
        assert!(s.set_worktree_setup(p, &typed).unwrap());
        assert_eq!(
            s.worktree_setup(p).unwrap().unwrap(),
            WorktreeSetup {
                copy: vec![".env".into(), "config/*.local".into()],
                command: Some("pnpm install --frozen-lockfile".into()),
            }
        );
        assert!(s.set_worktree_setup(p, &WorktreeSetup::default()).unwrap());
        assert!(s.worktree_setup(p).unwrap().unwrap().is_empty());
        assert!(s.worktree_setup(p + 99).unwrap().is_none());
        assert!(!s
            .set_worktree_setup(p + 99, &WorktreeSetup::default())
            .unwrap());
    }

    #[test]
    fn a_copy_glob_stays_inside_the_checkout() {
        for bad in [
            "/etc/passwd",
            "~/.ssh/id_rsa",
            "../secrets",
            "a/../../b",
            ".git/config",
            "a\nb",
            "c:\\x",
        ] {
            assert_eq!(
                check_setup_glob(bad).unwrap_err().code,
                codes::E_INVALID,
                "{bad}"
            );
        }
        for ok in [".env", "config/*.local", "**/.env.local", "a..b"] {
            assert_eq!(check_setup_glob(ok).unwrap(), ok);
        }
        let err = check_setup(&[], Some("pnpm i\nrm -rf /")).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        let many: Vec<String> = (0..=SETUP_GLOBS_MAX).map(|i| format!("f{i}")).collect();
        assert_eq!(check_setup(&many, None).unwrap_err().code, codes::E_INVALID);
    }
}
