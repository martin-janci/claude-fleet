//! `work_link { worktree_setup }`: a project's new worktree setup
//! (docs/work-graph.md#new-worktree-setup), the files a fresh worktree gets
//! from the base checkout and the command run in it before its agent starts.
//! The start itself is `service::sessions::worktree_setup`.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::WorkLinkArgs;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs;
use crate::service::view_scope::ViewScope;
use crate::store::{Store, WorktreeSetup};

/// A project's setup as the dialog shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSetupView {
    pub project_id: i64,
    /// `owner/repo`.
    pub project: String,
    /// Relative globs copied from the base checkout.
    pub copy: Vec<String>,
    /// The setup command, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// `work_link { worktree_setup, project_id, globs?, command? }`. With
/// neither `globs` nor `command` it reads the setup; with either it sets
/// both (a missing one clears: `globs: []`, `command: ""`). A setup command
/// runs on the fleet's hosts, so only a person sets it, and only a person
/// reads it (it may name a private registry or path).
pub fn setup(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    view: &ViewScope,
) -> Result<WorktreeSetupView, IpcError> {
    super::graph::person_only(
        view,
        "a person sets a project's worktree setup, from the desktop or the master token",
    )?;
    let pid = args
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "worktree_setup needs project_id"))?;
    // One guard throughout: the project read below proves the row for the
    // write and the re-read.
    let s = lock(store)?;
    let project = s
        .get_project(pid)?
        .ok_or_else(|| orgs::not_found("project", pid))?;
    if args.globs.is_some() || args.command.is_some() {
        let next = WorktreeSetup {
            copy: args.globs.clone().unwrap_or_default(),
            command: args.command.clone(),
        };
        s.set_worktree_setup(pid, &next)?;
    }
    let cur = s.worktree_setup(pid)?.unwrap_or_default();
    Ok(WorktreeSetupView {
        project_id: pid,
        project: format!("{}/{}", project.owner, project.repo),
        copy: cur.copy,
        command: cur.command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pid: i64) -> WorkLinkArgs {
        WorkLinkArgs {
            action: "worktree_setup".into(),
            project_id: Some(pid),
            ..Default::default()
        }
    }

    #[test]
    fn a_person_sets_reads_and_clears_a_projects_setup() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = lock(&store)
            .unwrap()
            .upsert_project("acme", "api", "/srv/acme/api")
            .unwrap();
        let view = ViewScope::internal();
        let read = setup(&args(pid), &store, &view).unwrap();
        assert_eq!(read.project, "acme/api");
        assert!(read.copy.is_empty() && read.command.is_none());
        let set = setup(
            &WorkLinkArgs {
                globs: Some(vec![".env".into()]),
                command: Some("pnpm install".into()),
                ..args(pid)
            },
            &store,
            &view,
        )
        .unwrap();
        assert_eq!(set.copy, vec![".env".to_string()]);
        assert_eq!(set.command.as_deref(), Some("pnpm install"));
        assert_eq!(setup(&args(pid), &store, &view).unwrap(), set);
        let cleared = setup(
            &WorkLinkArgs {
                globs: Some(vec![]),
                command: Some(String::new()),
                ..args(pid)
            },
            &store,
            &view,
        )
        .unwrap();
        assert!(cleared.copy.is_empty() && cleared.command.is_none());
        let bad = setup(
            &WorkLinkArgs {
                globs: Some(vec!["../x".into()]),
                ..args(pid)
            },
            &store,
            &view,
        )
        .unwrap_err();
        assert_eq!(bad.code, codes::E_INVALID);
        assert_eq!(
            setup(&args(pid + 99), &store, &view).unwrap_err().code,
            codes::E_NOTFOUND
        );
        assert_eq!(
            setup(
                &WorkLinkArgs {
                    project_id: None,
                    ..args(pid)
                },
                &store,
                &view
            )
            .unwrap_err()
            .code,
            codes::E_INVALID
        );
    }
}
