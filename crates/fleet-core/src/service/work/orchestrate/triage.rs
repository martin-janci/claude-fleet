//! K3 mission triage in Control (redesign step 9.10): the card Missions and
//! Today's Nudge show for a stuck mission.
//!
//! * **Facts first.** Fleet's own stuck reason and counts
//!   ([`mission_triage::stuck`]) decide whether there is a card at all.
//! * **Jev proposes** the outcome and the next step
//!   ([`mission_triage::ask`]), when its feature is on; in shadow or off
//!   the card shows the facts and the steps with nothing pre-selected.
//! * **The LLM writes the card** on demand only (`refresh: true`, the
//!   card's Draft / Regenerate): a few sentences on the mission's planner
//!   host, booked in `aux_usage` with origin
//!   [`crate::store::AUX_ORIGIN_TRIAGE`]. Its owner's or an org admin's,
//!   like every run the mission pays for.
//! * **A person picks.** Nothing here changes the mission: each step goes
//!   through the action the person already has (Retry, the planner, Cancel,
//!   the mission's question card). Triage never completes a mission and
//!   never sets Verified.

use super::drafts::{run_draft, Draft, Run};
use super::{changeable, mission_id, planner_host, Deps};
use crate::ipc_error::{lock, IpcError};
use crate::service::decide::mission_triage::{self, Proposals, Stuck};
use crate::service::decide::DecideCtx;
use crate::service::settings;
use crate::service::view_scope::ViewScope;
use crate::service::work::missions;
use crate::service::work::WorkLinkArgs;
use crate::store::MissionRow;
use serde::{Deserialize, Serialize};

/// The instruction a card draft starts with.
pub const TRIAGE_PROMPT: &str = "Write a short card, three sentences at most, for the person who \
    owns the stuck mission below: what happened, what the outcome is so far, and what the next \
    step would do. Use only the facts given. Plain text, no headings, no lists, no preamble.";

/// What a stuck mission's card shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triage {
    /// Why fleet calls it stuck; `None` when it is not, and then nothing
    /// else is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stuck: Option<Stuck>,
    #[serde(flatten)]
    pub proposals: Proposals,
    /// The LLM's card, when this call drafted one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Draft>,
    /// Whether this caller may act on it (and draft the card).
    #[serde(default)]
    pub may_change: bool,
}

/// PURE: a card draft's prompt and its "from" words.
pub fn card_prompt(m: &MissionRow, s: &Stuck, p: &Proposals) -> (String, String) {
    let mut out = format!("{TRIAGE_PROMPT}\n\nMission: {}\nGoal: {}\n", m.name, m.goal);
    for line in &m.done_when {
        out.push_str(&format!("Done when: {line}\n"));
    }
    out.push_str(&format!(
        "Stuck: {}\nTasks: {} done, {} failed, {} blocked of {}\n",
        s.why, s.done, s.failed, s.blocked, s.total
    ));
    if let Some(f) = &s.last_failure {
        out.push_str(&format!("Last failure: {f}\n"));
    }
    if let Some(o) = &p.outcome {
        out.push_str(&format!("Outcome so far (proposed): {}\n", o.value));
    }
    if let Some(n) = &p.next {
        out.push_str(&format!("Next step (proposed): {}\n", n.value));
    }
    let from = format!(
        "{} task{} and the stuck reason",
        s.total,
        if s.total == 1 { "" } else { "s" }
    );
    (out, from)
}

/// `work_link { action: mission_triage, mission_id, refresh? }`: the card of
/// a stuck mission (an empty one when it is not stuck), with Jev's
/// proposals; `refresh: true` also drafts the card's words.
pub async fn triage(
    args: &WorkLinkArgs,
    deps: &Deps,
    scope: &ViewScope,
) -> Result<Triage, IpcError> {
    let ctx = DecideCtx::jev(deps.store.clone());
    triage_with(args, deps, scope, &ctx).await
}

/// [`triage`] over a given decision context (tests script the backend).
pub async fn triage_with(
    args: &WorkLinkArgs,
    deps: &Deps,
    scope: &ViewScope,
    ctx: &DecideCtx,
) -> Result<Triage, IpcError> {
    let id = mission_id(args)?;
    let detail = missions::mission(&deps.store, scope, id, None)?;
    let Some(stuck) = mission_triage::stuck(&detail) else {
        return Ok(Triage {
            may_change: detail.may_change,
            ..Default::default()
        });
    };
    let m = detail.mission;
    let proposals = mission_triage::ask(ctx, &m, &stuck).await;
    let card = if args.refresh == Some(true) {
        let (host, model) = {
            let s = lock(&deps.store)?;
            changeable(&s, scope, id)?;
            (
                planner_host(&s, &m, None)?,
                settings::get_string_for(&s, settings::WORK_SUMMARY_MODEL, m.org_id),
            )
        };
        let (prompt, from) = card_prompt(&m, &stuck, &proposals);
        let run = Run {
            origin: crate::store::AUX_ORIGIN_TRIAGE,
            host: &host,
            model: &model,
            mission_id: Some(m.id),
            org_id: m.org_id,
        };
        let (text, truncated) = run_draft(deps, &run, &prompt).await?;
        Some(Draft {
            text,
            model,
            host_alias: host,
            from,
            at: crate::store::now_unix(),
            truncated,
        })
    } else {
        None
    };
    Ok(Triage {
        stuck: Some(stuck),
        proposals,
        card,
        may_change: detail.may_change,
    })
}
