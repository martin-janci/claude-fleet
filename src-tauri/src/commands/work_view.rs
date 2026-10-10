//! Tauri commands of the Work view (work graph M14,
//! `docs/superpowers/specs/2026-09-27-work-view-design.md`): eight reads of
//! `work` and ten decisions of `work_link`, each routed by command name on a
//! paired desktop and served by `service::work::{view, structure}` here.
//! Thin wrappers, as `commands::work` is: the rules live in fleet-core.
//!
//! Sprints and releases (design 2026-09-28 §6a/§6b): `work_buckets` /
//! `work_bucket` read `work`, `add_work_to_bucket` /
//! `remove_work_from_bucket` write `work_link`, and `work_bucket_admin`
//! (create, update, close, delete) is `work_admin` here and, on a paired
//! desktop, `work_link { action: bucket_admin }` — the hub decides whether
//! this person may plan (their own personal buckets; an org's as its admin,
//! or member when the org allows it).

use crate::backend::FleetBackend;
use fleet_core::ipc_error::codes;
use fleet_core::ipc_error::IpcError;
use fleet_core::service::orgs::OrgScope;
use fleet_core::service::trackers::admin::{self as tracker_admin, WorkAdminArgs};
use fleet_core::service::work::buckets::{self, BucketAction, BucketDetail};
use fleet_core::service::work::structure::{
    self, BatchResult, Deleted, LinkDecision, OrgImpact, RuleInput, RulePreview, ViewInput,
};
use fleet_core::service::work::view::{
    self, ReviewPage, SectionAsk, SessionTasks, TaskDetail, TreeArgs, TreePage, WorkTask,
    WorkTreeFilters,
};
use fleet_core::service::work::{self, WorkArgs, WorkLinkArgs};
use fleet_core::store::{BucketRow, CommentRow, Decider, SessionRow, Store, WorkRule, WorkView};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

/// Who a local placement is recorded as having been made by.
const LOCAL_ACTOR: &str = "desktop";

/// `work_tree`: one page of the Work view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkTreeCmdArgs {
    #[serde(default)]
    pub filters: Option<WorkTreeFilters>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub per_task: Option<usize>,
    /// Open sections to page from the same read (absent from an older hub's
    /// answer: the view then reads each by itself).
    #[serde(default)]
    pub sections: Option<Vec<SectionAsk>>,
    /// Add the review inbox's total from the same read.
    #[serde(default)]
    pub with_review_total: Option<bool>,
}

/// A command that names one task.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkTaskCmdArgs {
    pub task_id: String,
}

/// `work_session_tasks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkSessionTasksArgs {
    pub session_id: i64,
}

/// `work_review`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkReviewArgs {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Commands with no argument still take `{ args: {} }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoArgs {}

/// `work_rule_preview` / `save_work_rule`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkRuleArgs {
    pub rule: RuleInput,
}

/// `work_org_impact`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkOrgImpactArgs {
    pub task_id: String,
    /// 0: no org.
    pub org_id: i64,
}

/// `set_primary_work`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetPrimaryWorkArgs {
    pub session_id: i64,
    pub link_id: i64,
    /// The primary the person saw (0: none); absent: no check.
    #[serde(default)]
    pub expected_primary: Option<i64>,
}

/// `switch_session_work` (task → session P-2): end link `link_id` and make
/// the target (`key` or `item_id`) the session's primary, in one step.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwitchSessionWorkArgs {
    pub session_id: i64,
    pub link_id: i64,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub item_id: Option<i64>,
    /// The primary the person saw (0: none); absent: no check.
    #[serde(default)]
    pub expected_primary: Option<i64>,
    /// `false`: refuse with `E_EXISTS` when the target has another live
    /// session (P-3); `true`: the person saw that and goes ahead.
    #[serde(default)]
    pub ack_live: Option<bool>,
    /// Another org's work anyway: a person saw the refusal and meant it.
    #[serde(default)]
    pub force_cross_org: bool,
}

/// `reconsider_work_link` / `ack_work_link`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkLinkDecisionArgs {
    pub session_id: i64,
    pub link_id: i64,
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// `decide_work_batch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecideWorkBatchArgs {
    pub decisions: Vec<LinkDecision>,
}

/// `place_work`: `group` empty and no `note` clears the placement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaceWorkArgs {
    pub task_id: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub note: Option<String>,
    /// The placement's version the person saw (0: none).
    pub expected_version: i64,
}

/// `assign_work_org`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssignWorkOrgArgs {
    pub task_id: String,
    /// 0: no org.
    pub org_id: i64,
    pub impact_token: String,
}

/// `delete_work_rule`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteWorkRuleArgs {
    pub rule_id: i64,
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// `save_work_view`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SaveWorkViewArgs {
    pub view: ViewInput,
}

/// `delete_work_view`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteWorkViewArgs {
    pub view_id: i64,
    /// The view's version the person saw; absent: no check.
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// `work_buckets`: the sprints and releases in scope, with roll-ups.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkBucketsArgs {
    /// sprint | release; absent: both.
    #[serde(default)]
    pub kind: Option<String>,
}

/// `work_bucket`: one bucket with its members, past ones too.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkBucketArgs {
    pub bucket_id: i64,
}

/// `comment_on_work`: a comment on a task (a work item).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommentOnWorkArgs {
    pub item_id: i64,
    pub body: String,
}

/// `delete_work_comment`: the author's own comment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteWorkCommentArgs {
    pub comment_id: i64,
}

/// `add_work_to_bucket` / `remove_work_from_bucket`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BucketMemberArgs {
    pub bucket_id: i64,
    pub item_id: i64,
}

fn read(action: &str) -> WorkArgs {
    WorkArgs {
        action: Some(action.into()),
        ..Default::default()
    }
}

fn decide(action: &str, session_id: i64, link_id: i64) -> WorkLinkArgs {
    WorkLinkArgs {
        action: action.into(),
        session_id: Some(session_id),
        link_id: Some(link_id),
        ..Default::default()
    }
}

// --- the commands --------------------------------------------------------------

#[tauri::command]
pub async fn comment_on_work(
    args: CommentOnWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<CommentRow, IpcError> {
    routed::comment_on_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn delete_work_comment(
    args: DeleteWorkCommentArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<CommentRow, IpcError> {
    routed::delete_work_comment(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_buckets(
    args: WorkBucketsArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<BucketRow>, IpcError> {
    routed::work_buckets(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_bucket(
    args: WorkBucketArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<BucketDetail, IpcError> {
    routed::work_bucket(&backend, args, &store).await
}

#[tauri::command]
pub async fn add_work_to_bucket(
    args: BucketMemberArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<BucketRow, IpcError> {
    routed::add_work_to_bucket(&backend, args, &store).await
}

#[tauri::command]
pub async fn remove_work_from_bucket(
    args: BucketMemberArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<BucketRow, IpcError> {
    routed::remove_work_from_bucket(&backend, args, &store).await
}

/// Create, change, close or delete a sprint or release: `work_admin`'s
/// `bucket_*` actions and no other (adoption stays with the hub's admin).
/// Paired, it routes as `work_link { action: bucket_admin, bucket_op }`.
#[tauri::command]
pub async fn work_bucket_admin(
    args: WorkAdminArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<serde_json::Value, IpcError> {
    routed::work_bucket_admin(&backend, args, &store).await
}

/// The standalone desktop's reader (multi-user M1): one person at the
/// keyboard, so the Work view's reads run with the hub's own unrestricted
/// scope exactly as they did before people existed. A desktop PAIRED to a hub
/// never reaches these branches — it routes to the hub, which builds the
/// caller's own [`ViewScope`].
fn internal_view() -> fleet_core::service::view_scope::ViewScope {
    fleet_core::service::view_scope::ViewScope::internal()
}

#[tauri::command]
pub async fn work_tree(
    args: WorkTreeCmdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TreePage, IpcError> {
    routed::work_tree(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_task(
    args: WorkTaskCmdArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<TaskDetail, IpcError> {
    routed::work_task(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_session_tasks(
    args: WorkSessionTasksArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionTasks, IpcError> {
    routed::work_session_tasks(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_review(
    args: WorkReviewArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<ReviewPage, IpcError> {
    routed::work_review(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_rules(
    args: NoArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<WorkRule>, IpcError> {
    routed::work_rules(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_rule_preview(
    args: WorkRuleArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<RulePreview, IpcError> {
    routed::work_rule_preview(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_views(
    args: NoArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<WorkView>, IpcError> {
    routed::work_views(&backend, args, &store).await
}

#[tauri::command]
pub async fn work_org_impact(
    args: WorkOrgImpactArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<OrgImpact, IpcError> {
    routed::work_org_impact(&backend, args, &store).await
}

#[tauri::command]
pub async fn switch_session_work(
    args: SwitchSessionWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::switch_session_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn set_primary_work(
    args: SetPrimaryWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::set_primary_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn reconsider_work_link(
    args: WorkLinkDecisionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::reconsider_work_link(&backend, args, &store).await
}

#[tauri::command]
pub async fn ack_work_link(
    args: WorkLinkDecisionArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<SessionRow, IpcError> {
    routed::ack_work_link(&backend, args, &store).await
}

#[tauri::command]
pub async fn decide_work_batch(
    args: DecideWorkBatchArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<BatchResult, IpcError> {
    routed::decide_work_batch(&backend, args, &store).await
}

#[tauri::command]
pub async fn place_work(
    args: PlaceWorkArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WorkTask, IpcError> {
    routed::place_work(&backend, args, &store).await
}

#[tauri::command]
pub async fn assign_work_org(
    args: AssignWorkOrgArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WorkTask, IpcError> {
    routed::assign_work_org(&backend, args, &store).await
}

#[tauri::command]
pub async fn save_work_rule(
    args: WorkRuleArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WorkRule, IpcError> {
    routed::save_work_rule(&backend, args, &store).await
}

#[tauri::command]
pub async fn delete_work_rule(
    args: DeleteWorkRuleArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Deleted, IpcError> {
    routed::delete_work_rule(&backend, args, &store).await
}

#[tauri::command]
pub async fn save_work_view(
    args: SaveWorkViewArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WorkView, IpcError> {
    routed::save_work_view(&backend, args, &store).await
}

#[tauri::command]
pub async fn delete_work_view(
    args: DeleteWorkViewArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Deleted, IpcError> {
    routed::delete_work_view(&backend, args, &store).await
}

pub(crate) mod routed {
    use super::*;

    pub async fn work_tree(
        backend: &FleetBackend,
        args: WorkTreeCmdArgs,
        store: &Mutex<Store>,
    ) -> Result<TreePage, IpcError> {
        let wire = WorkArgs {
            filters: args.filters.clone(),
            cursor: args.cursor.clone(),
            limit: args.limit,
            per_task: args.per_task,
            sections: args.sections.clone(),
            with_review_total: args.with_review_total,
            ..read("tree")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_tree", &wire).await,
            None => view::tree(
                store,
                &internal_view(),
                &TreeArgs {
                    filters: args.filters.unwrap_or_default(),
                    cursor: args.cursor,
                    limit: args.limit,
                    per_task: args.per_task,
                    sections: args.sections.unwrap_or_default(),
                    with_review_total: args.with_review_total == Some(true),
                },
            ),
        }
    }

    pub async fn work_task(
        backend: &FleetBackend,
        args: WorkTaskCmdArgs,
        store: &Mutex<Store>,
    ) -> Result<TaskDetail, IpcError> {
        let wire = WorkArgs {
            task_id: Some(args.task_id.clone()),
            ..read("task")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_task", &wire).await,
            None => {
                let mut detail = view::task(store, &internal_view(), &args.task_id)?;
                // A standalone desktop writes as LOCAL_ACTOR: its own comments
                // are the ones it may delete.
                for c in &mut detail.comments {
                    c.mark_mine(None, LOCAL_ACTOR);
                }
                Ok(detail)
            }
        }
    }

    pub async fn work_session_tasks(
        backend: &FleetBackend,
        args: WorkSessionTasksArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionTasks, IpcError> {
        let wire = WorkArgs {
            session_id: Some(args.session_id),
            ..read("session_tasks")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_session_tasks", &wire).await,
            None => view::session_tasks(store, &internal_view(), args.session_id),
        }
    }

    pub async fn work_review(
        backend: &FleetBackend,
        args: WorkReviewArgs,
        store: &Mutex<Store>,
    ) -> Result<ReviewPage, IpcError> {
        let wire = WorkArgs {
            cursor: args.cursor.clone(),
            limit: args.limit,
            ..read("review")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_review", &wire).await,
            None => view::review(store, &internal_view(), args.cursor.as_deref(), args.limit),
        }
    }

    pub async fn work_rules(
        backend: &FleetBackend,
        _args: NoArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<WorkRule>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("work_rules", &read("rules")).await,
            None => structure::rules(store, &OrgScope::All),
        }
    }

    pub async fn work_rule_preview(
        backend: &FleetBackend,
        args: WorkRuleArgs,
        store: &Mutex<Store>,
    ) -> Result<RulePreview, IpcError> {
        let wire = WorkArgs {
            rule: Some(args.rule.clone()),
            ..read("rule_preview")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_rule_preview", &wire).await,
            None => structure::rule_preview(store, &OrgScope::All, &args.rule),
        }
    }

    pub async fn work_views(
        backend: &FleetBackend,
        _args: NoArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<WorkView>, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("work_views", &read("views")).await,
            None => structure::views(store, &OrgScope::All),
        }
    }

    pub async fn work_org_impact(
        backend: &FleetBackend,
        args: WorkOrgImpactArgs,
        store: &Mutex<Store>,
    ) -> Result<OrgImpact, IpcError> {
        let wire = WorkArgs {
            task_id: Some(args.task_id.clone()),
            org_id: Some(args.org_id),
            ..read("org_impact")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_org_impact", &wire).await,
            None => {
                structure::org_impact(store, &internal_view(), &args.task_id, Some(args.org_id))
            }
        }
    }

    pub async fn set_primary_work(
        backend: &FleetBackend,
        args: SetPrimaryWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let wire = WorkLinkArgs {
            expected_primary: args.expected_primary,
            ..decide("set_primary", args.session_id, args.link_id)
        };
        match backend.hub() {
            Some(hub) => hub.route("set_primary_work", &wire).await,
            None => work::work_link(&wire, store, &OrgScope::All),
        }
    }

    pub async fn switch_session_work(
        backend: &FleetBackend,
        args: SwitchSessionWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let wire = WorkLinkArgs {
            key: args.key,
            item_id: args.item_id,
            expected_primary: args.expected_primary,
            ack_live: args.ack_live,
            force_cross_org: args.force_cross_org.then_some(true),
            ..decide("switch", args.session_id, args.link_id)
        };
        match backend.hub() {
            Some(hub) => hub.route("switch_session_work", &wire).await,
            None => {
                work::check_live_elsewhere(&wire, store, &internal_view())?;
                work::work_link(&wire, store, &OrgScope::All)
            }
        }
    }

    pub async fn reconsider_work_link(
        backend: &FleetBackend,
        args: WorkLinkDecisionArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let wire = WorkLinkArgs {
            expected_version: args.expected_version,
            ..decide("reconsider", args.session_id, args.link_id)
        };
        match backend.hub() {
            Some(hub) => hub.route("reconsider_work_link", &wire).await,
            None => work::work_link(&wire, store, &OrgScope::All),
        }
    }

    pub async fn ack_work_link(
        backend: &FleetBackend,
        args: WorkLinkDecisionArgs,
        store: &Mutex<Store>,
    ) -> Result<SessionRow, IpcError> {
        let wire = WorkLinkArgs {
            expected_version: args.expected_version,
            ..decide("ack", args.session_id, args.link_id)
        };
        match backend.hub() {
            Some(hub) => hub.route("ack_work_link", &wire).await,
            None => work::work_link(&wire, store, &OrgScope::All),
        }
    }

    pub async fn decide_work_batch(
        backend: &FleetBackend,
        args: DecideWorkBatchArgs,
        store: &Mutex<Store>,
    ) -> Result<BatchResult, IpcError> {
        let wire = WorkLinkArgs {
            action: "decide_batch".into(),
            decisions: Some(args.decisions.clone()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("decide_work_batch", &wire).await,
            // The desktop is the master: every session passes its gate.
            None => structure::decide_batch(
                store,
                &OrgScope::All,
                Decider::Person,
                &args.decisions,
                &|_| Ok(()),
            ),
        }
    }

    pub async fn place_work(
        backend: &FleetBackend,
        args: PlaceWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<WorkTask, IpcError> {
        let wire = WorkLinkArgs {
            action: "place".into(),
            task_id: Some(args.task_id.clone()),
            group: Some(args.group.clone()),
            note: args.note.clone(),
            expected_version: Some(args.expected_version),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("place_work", &wire).await,
            None => structure::place(
                store,
                &internal_view(),
                &args.task_id,
                Some(&args.group),
                args.note.as_deref(),
                Some(args.expected_version),
                LOCAL_ACTOR,
            ),
        }
    }

    pub async fn assign_work_org(
        backend: &FleetBackend,
        args: AssignWorkOrgArgs,
        store: &Mutex<Store>,
    ) -> Result<WorkTask, IpcError> {
        let wire = WorkLinkArgs {
            action: "assign_org".into(),
            task_id: Some(args.task_id.clone()),
            org_id: Some(args.org_id),
            impact_token: Some(args.impact_token.clone()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("assign_work_org", &wire).await,
            None => structure::assign_org(
                store,
                &internal_view(),
                &args.task_id,
                Some(args.org_id),
                Some(&args.impact_token),
            ),
        }
    }

    pub async fn save_work_rule(
        backend: &FleetBackend,
        args: WorkRuleArgs,
        store: &Mutex<Store>,
    ) -> Result<WorkRule, IpcError> {
        let wire = WorkLinkArgs {
            action: "rule_save".into(),
            rule: Some(args.rule.clone()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("save_work_rule", &wire).await,
            None => structure::rule_save(store, &OrgScope::All, &args.rule),
        }
    }

    pub async fn delete_work_rule(
        backend: &FleetBackend,
        args: DeleteWorkRuleArgs,
        store: &Mutex<Store>,
    ) -> Result<Deleted, IpcError> {
        let wire = WorkLinkArgs {
            action: "rule_delete".into(),
            rule_id: Some(args.rule_id),
            expected_version: args.expected_version,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("delete_work_rule", &wire).await,
            None => {
                structure::rule_delete(store, &OrgScope::All, args.rule_id, args.expected_version)
            }
        }
    }

    pub async fn save_work_view(
        backend: &FleetBackend,
        args: SaveWorkViewArgs,
        store: &Mutex<Store>,
    ) -> Result<WorkView, IpcError> {
        let wire = WorkLinkArgs {
            action: "view_save".into(),
            view: Some(args.view.clone()),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("save_work_view", &wire).await,
            None => structure::view_save(store, &OrgScope::All, &args.view),
        }
    }

    pub async fn delete_work_view(
        backend: &FleetBackend,
        args: DeleteWorkViewArgs,
        store: &Mutex<Store>,
    ) -> Result<Deleted, IpcError> {
        let wire = WorkLinkArgs {
            action: "view_delete".into(),
            view_id: Some(args.view_id),
            expected_version: args.expected_version,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("delete_work_view", &wire).await,
            None => {
                structure::view_delete(store, &OrgScope::All, args.view_id, args.expected_version)
            }
        }
    }
    pub async fn work_buckets(
        backend: &FleetBackend,
        args: WorkBucketsArgs,
        store: &Mutex<Store>,
    ) -> Result<Vec<BucketRow>, IpcError> {
        let wire = WorkArgs {
            kind: args.kind.clone(),
            ..read("buckets")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_buckets", &wire).await,
            None => buckets::buckets(store, &internal_view(), args.kind.as_deref()),
        }
    }

    pub async fn work_bucket(
        backend: &FleetBackend,
        args: WorkBucketArgs,
        store: &Mutex<Store>,
    ) -> Result<BucketDetail, IpcError> {
        let wire = WorkArgs {
            bucket_id: Some(args.bucket_id),
            ..read("bucket")
        };
        match backend.hub() {
            Some(hub) => hub.route("work_bucket", &wire).await,
            None => buckets::bucket(store, &internal_view(), args.bucket_id),
        }
    }

    pub async fn work_bucket_admin(
        backend: &FleetBackend,
        args: WorkAdminArgs,
        store: &Mutex<Store>,
    ) -> Result<serde_json::Value, IpcError> {
        match BucketAction::parse(&args.action) {
            Some(BucketAction::Create)
            | Some(BucketAction::Update)
            | Some(BucketAction::Close)
            | Some(BucketAction::Delete) => match backend.hub() {
                Some(hub) => {
                    let wire = WorkLinkArgs {
                        action: "bucket_admin".into(),
                        bucket_op: Some(args),
                        ..Default::default()
                    };
                    hub.route("work_bucket_admin", &wire).await
                }
                None => tracker_admin::admin_sync(&args, store),
            },
            _ => Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "work_bucket_admin is bucket_create, bucket_update, bucket_close or \
                     bucket_delete, not {:?}",
                    args.action
                ),
            )),
        }
    }

    fn member(action: &str, args: &BucketMemberArgs) -> WorkLinkArgs {
        WorkLinkArgs {
            action: action.into(),
            bucket_id: Some(args.bucket_id),
            item_id: Some(args.item_id),
            ..Default::default()
        }
    }

    pub async fn add_work_to_bucket(
        backend: &FleetBackend,
        args: BucketMemberArgs,
        store: &Mutex<Store>,
    ) -> Result<BucketRow, IpcError> {
        let wire = member("bucket_add", &args);
        match backend.hub() {
            Some(hub) => hub.route("add_work_to_bucket", &wire).await,
            None => buckets::bucket_add(&wire, store, &internal_view()),
        }
    }

    pub async fn remove_work_from_bucket(
        backend: &FleetBackend,
        args: BucketMemberArgs,
        store: &Mutex<Store>,
    ) -> Result<BucketRow, IpcError> {
        let wire = member("bucket_remove", &args);
        match backend.hub() {
            Some(hub) => hub.route("remove_work_from_bucket", &wire).await,
            None => buckets::bucket_remove(&wire, store, &internal_view()),
        }
    }

    pub async fn comment_on_work(
        backend: &FleetBackend,
        args: CommentOnWorkArgs,
        store: &Mutex<Store>,
    ) -> Result<CommentRow, IpcError> {
        let wire = WorkLinkArgs {
            action: "comment".into(),
            item_id: Some(args.item_id),
            notes: Some(args.body),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("comment_on_work", &wire).await,
            None => work::local::comment(&wire, store, &OrgScope::All, LOCAL_ACTOR, None),
        }
    }

    pub async fn delete_work_comment(
        backend: &FleetBackend,
        args: DeleteWorkCommentArgs,
        store: &Mutex<Store>,
    ) -> Result<CommentRow, IpcError> {
        let wire = WorkLinkArgs {
            action: "comment_delete".into(),
            comment_id: Some(args.comment_id),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("delete_work_comment", &wire).await,
            None => work::local::comment_delete(&wire, store, &OrgScope::All, LOCAL_ACTOR, None),
        }
    }
}
