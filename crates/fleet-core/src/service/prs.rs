//! Pull requests (redesign step 6.4): Work › Pull requests and the control
//! API's `prs { action: list }`. Reconcile records every PR a session's
//! branch has had (`store::pull_requests`); this answers them, newest change
//! first, to whoever may see the session that opened each one.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::{PullRequestRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Most rows one list answers.
pub const PRS_MAX_LIMIT: u32 = 500;
const PRS_DEFAULT_LIMIT: u32 = 200;

/// `prs { action, state?, project_id?, limit? }`.
#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "PrsParams")]
pub struct PrsArgs {
    /// list.
    #[serde(default = "default_action")]
    pub action: String,
    /// open | merged | closed | all (default all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Only this project's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// ≤ 500 (default 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

fn default_action() -> String {
    "list".into()
}

impl Default for PrsArgs {
    fn default() -> Self {
        PrsArgs {
            action: default_action(),
            state: None,
            project_id: None,
            limit: None,
        }
    }
}

/// What `prs { action: list }` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrList {
    pub items: Vec<PullRequestRow>,
    /// Every visible row the filters match, before `limit`.
    pub total: u32,
}

fn states(state: Option<&str>) -> Result<&'static [&'static str], IpcError> {
    Ok(match state.map(str::trim).unwrap_or("all") {
        "" | "all" => &[],
        "open" => &["OPEN"],
        "merged" => &["MERGED"],
        "closed" => &["CLOSED"],
        other => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("state {other:?}: open, merged, closed or all"),
            ))
        }
    })
}

/// May `scope` see this PR? Through the session that opened it, by the same
/// rule as the session row; a PR whose session is gone is seen only by the
/// hub's own unnarrowed reader or the one person of a one-person hub.
pub(crate) fn visible(s: &Store, scope: &ViewScope, pr: &PullRequestRow) -> Result<bool, IpcError> {
    Ok(visible_with_diffstat(s, scope, pr)?.is_some())
}

/// What the list adds to a stored PR row from its opening session: the
/// diffstat (lines added, removed) and the mission that started it.
#[derive(Default)]
struct Extras {
    additions: Option<u32>,
    deletions: Option<u32>,
    mission: Option<(i64, String)>,
}

/// [`visible`], answering what the opening session adds when it is: the
/// diffstat, read from its probe while that session's PR is still this one
/// (gap plan G3.10), and its mission when a mission started it and `scope`
/// may see that mission (G7.8). `None` = not visible.
fn visible_with_diffstat(
    s: &Store,
    scope: &ViewScope,
    pr: &PullRequestRow,
) -> Result<Option<Extras>, IpcError> {
    let row = match pr.session_id {
        Some(id) => s.get_session_by_id(id)?,
        None => None,
    };
    Ok(match row {
        Some(row) if scope.sees_session_row(&row).is_visible() => {
            let ev = row
                .pr_evidence
                .as_ref()
                .filter(|_| row.pr_url.as_deref() == Some(pr.url.as_str()));
            let mission = match (row.origin.as_deref(), row.origin_ref.as_deref()) {
                (Some("mission"), Some(r)) => match r.parse::<i64>().ok() {
                    Some(id) => match s.get_mission(id)? {
                        Some(m) if crate::service::work::missions::sees_mission(s, scope, &m)? => {
                            Some((m.id, m.name))
                        }
                        _ => None,
                    },
                    None => None,
                },
                _ => None,
            };
            Some(Extras {
                additions: ev.and_then(|e| e.additions),
                deletions: ev.and_then(|e| e.deletions),
                mission,
            })
        }
        Some(_) => None,
        None => (scope.is_unrestricted() || scope.is_sole_person()).then(Extras::default),
    })
}

pub fn list(store: &Mutex<Store>, scope: &ViewScope, args: &PrsArgs) -> Result<PrList, IpcError> {
    if args.action != "list" {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("prs: unknown action {:?} (list)", args.action),
        ));
    }
    let wanted = states(args.state.as_deref())?;
    let limit = args
        .limit
        .unwrap_or(PRS_DEFAULT_LIMIT)
        .clamp(1, PRS_MAX_LIMIT) as usize;
    let s = lock(store)?;
    let mut items = Vec::new();
    for pr in s.list_pull_requests(wanted)? {
        if args.project_id.is_some_and(|p| pr.project_id != Some(p)) {
            continue;
        }
        if let Some(x) = visible_with_diffstat(&s, scope, &pr)? {
            let (mission_id, mission_name) = x.mission.unzip();
            items.push(PullRequestRow {
                additions: x.additions,
                deletions: x.deletions,
                mission_id,
                mission_name,
                ..pr
            });
        }
    }
    let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
    items.truncate(limit);
    Ok(PrList { items, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::outcome::PrEvidence;
    use crate::store::PrSeenBy;

    fn seed(s: &Store, n: i64, state: &str, session_id: i64, project_id: Option<i64>, at: i64) {
        let ev = PrEvidence {
            state: Some(state.into()),
            ..PrEvidence::default()
        };
        Store::upsert_pull_request_in_tx(
            s.conn_ref(),
            &format!("https://github.com/o/r/pull/{n}"),
            None,
            Some(&ev),
            PrSeenBy {
                session_id,
                session_name: "s",
                host_alias: "trn",
                project_id,
            },
            at,
        )
        .unwrap();
    }

    fn args(state: Option<&str>) -> PrsArgs {
        PrsArgs {
            state: state.map(str::to_string),
            ..PrsArgs::default()
        }
    }

    #[test]
    fn the_list_filters_by_state_and_project_and_counts_before_the_limit() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = st.lock().unwrap();
            seed(&s, 1, "OPEN", 10, Some(1), 100);
            seed(&s, 2, "MERGED", 11, Some(1), 200);
            seed(&s, 3, "CLOSED", 12, Some(2), 300);
            seed(&s, 4, "OPEN", 13, Some(2), 400);
        }
        let all = ViewScope::internal();
        let n = |l: &PrList| {
            l.items
                .iter()
                .map(|p| p.number.unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(n(&list(&st, &all, &args(None)).unwrap()), vec![4, 3, 2, 1]);
        assert_eq!(
            n(&list(&st, &all, &args(Some("open"))).unwrap()),
            vec![4, 1]
        );
        assert_eq!(n(&list(&st, &all, &args(Some("merged"))).unwrap()), vec![2]);
        let p1 = PrsArgs {
            project_id: Some(1),
            ..args(Some("all"))
        };
        assert_eq!(n(&list(&st, &all, &p1).unwrap()), vec![2, 1]);
        let one = PrsArgs {
            limit: Some(1),
            ..args(None)
        };
        let page = list(&st, &all, &one).unwrap();
        assert_eq!((n(&page), page.total), (vec![4], 4));
        assert!(list(&st, &all, &args(Some("draft"))).is_err());
        let bad = PrsArgs {
            action: "merge".into(),
            ..PrsArgs::default()
        };
        assert!(list(&st, &all, &bad).is_err(), "list is the only action");
    }

    /// Gap plan G3.10: a row carries its diffstat from the opening
    /// session's probe, and only while that session is still on this PR.
    #[test]
    fn the_diffstat_comes_from_the_opening_sessions_probe_of_that_pr() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = st.lock().unwrap();
            s.insert_host("trn", None).unwrap();
            let a = s
                .upsert_session("a", "trn", None, None, 1, 1, "running", None)
                .unwrap();
            let b = s
                .upsert_session("b", "trn", None, None, 1, 1, "running", None)
                .unwrap();
            let ev = r#"{"additions":18,"deletions":6}"#;
            for (id, n) in [(a, 1), (b, 9)] {
                s.conn_ref()
                    .execute(
                        "UPDATE sessions SET pr_url = ?1, pr_evidence = ?2 WHERE id = ?3",
                        rusqlite::params![format!("https://github.com/o/r/pull/{n}"), ev, id],
                    )
                    .unwrap();
            }
            seed(&s, 1, "OPEN", a, None, 100);
            // b has moved on to PR 9: its probe says nothing about PR 2.
            seed(&s, 2, "MERGED", b, None, 200);
        }
        let l = list(&st, &ViewScope::internal(), &args(None)).unwrap();
        let stat = |n: i64| {
            let p = l.items.iter().find(|p| p.number == Some(n)).unwrap();
            (p.additions, p.deletions)
        };
        assert_eq!(stat(1), (Some(18), Some(6)));
        assert_eq!(stat(2), (None, None));
        let json = serde_json::to_value(&l.items[0]).unwrap();
        assert!(json.get("additions").is_none(), "unknown is left out");
    }

    /// Gap plan G7.8: a PR a mission's session opened says which mission,
    /// and one a person's session opened says nothing.
    #[test]
    fn a_pr_names_the_mission_whose_session_opened_it() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = st.lock().unwrap();
            s.insert_host("trn", None).unwrap();
            let m = s
                .create_mission(
                    &crate::store::NewMission {
                        org_id: None,
                        owner_person_id: None,
                        root_item_id: None,
                        name: "Hub federation v2",
                        goal: "pair two hubs",
                        non_goals: None,
                        done_when: &[],
                        mode: None,
                        level: None,
                    },
                    "test",
                )
                .unwrap();
            let a = s
                .upsert_session("a", "trn", None, None, 1, 1, "running", None)
                .unwrap();
            let b = s
                .upsert_session("b", "trn", None, None, 1, 1, "running", None)
                .unwrap();
            s.set_session_origin(a, &crate::store::SessionOrigin::mission(m.id))
                .unwrap();
            seed(&s, 1, "OPEN", a, None, 100);
            seed(&s, 2, "OPEN", b, None, 200);
        }
        let l = list(&st, &ViewScope::internal(), &args(None)).unwrap();
        let from = |n: i64| {
            let p = l.items.iter().find(|p| p.number == Some(n)).unwrap();
            p.mission_name.clone()
        };
        assert_eq!(from(1).as_deref(), Some("Hub federation v2"));
        assert_eq!(from(2), None);
        let json =
            serde_json::to_value(l.items.iter().find(|p| p.number == Some(2)).unwrap()).unwrap();
        assert!(json.get("mission_name").is_none(), "no mission is left out");
    }

    #[test]
    fn a_narrowed_scope_sees_no_pr_whose_session_it_cannot_see() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        seed(&st.lock().unwrap(), 1, "OPEN", 999, None, 100);
        let narrowed =
            crate::service::view_scope::org_only_view(&crate::service::orgs::OrgScope::Org {
                org: 42,
                sees_unassigned: false,
            });
        assert!(list(&st, &narrowed, &args(None)).unwrap().items.is_empty());
        assert_eq!(
            list(&st, &ViewScope::internal(), &args(None))
                .unwrap()
                .total,
            1
        );
    }
}
