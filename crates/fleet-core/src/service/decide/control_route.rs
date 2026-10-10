//! K2 `control_route`: which mission or session a message typed in Control
//! is about (redesign step 9.9; the test map's card K2, there called
//! `operator_thread`). After the person sends a message in Control's chat,
//! the desktop asks this adapter one Choice over the active missions, the
//! running sessions and Control itself, and shows the answer under the
//! message as a receipt: *For "Hub federation v2" · Proposed by Jev ·
//! Change*.
//!
//! * **Rule first.** A message too short to route ([`unclear`]: fewer than
//!   [`MIN_WORDS`] words) is never sent to the model: in `assist` Control
//!   asks where it goes instead, with the same targets and nothing
//!   pre-selected. A slash command is never routed at all.
//! * **Asked after the send, never before.** The message has already
//!   reached Control's agent; nothing here moves, forwards or holds it. The
//!   receipt says what it is about; "Change" puts it right.
//! * **Shadow / assist only.** In `shadow` the answer is only recorded; in
//!   `assist` a usable answer naming a target becomes the receipt, and
//!   `unsure` or a weak answer becomes the question. `control` (the message
//!   is for Control itself) shows nothing.
//! * **What is sent.** The message's first [`MESSAGE_CHARS`] characters,
//!   each mission's name and the start of its goal, each session's name and
//!   project — through the envelope's redaction.
//! * **What is recorded.** A run about subject `control_message`
//!   `msg:<HMAC>` (the message is never stored as is); the options are
//!   `m<id>`, `s<id>`, `control` and `unsure`.
//! * **Follow-up.** "Change" marks the run `corrected` to the person's pick
//!   ([`follow`]); opening the proposed target marks it `confirmed`. A shadow
//!   answer nobody saw is never marked.

use super::{decide, gate_at, DecideCtx, DecideRequest, Feature, JevRequest, Mode, Question};
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::view_scope::{ViewScope, Visibility};
use crate::store::{DecisionProposal, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The question's version: bump it when the question below changes.
pub const QUESTION_VERSION: &str = "control_route.v1";
/// What a run is about: one message typed in Control.
pub const SUBJECT_KIND: &str = "control_message";
/// Below this confidence an answer is recorded, not suggested.
pub const MIN_CONFIDENCE: f64 = 0.5;
/// The option that suggests nothing.
pub const UNSURE: &str = "unsure";
/// The option that keeps the message with Control itself.
pub const CONTROL: &str = "control";
/// Characters of the message sent, at most.
pub const MESSAGE_CHARS: usize = 1000;
/// Characters of a mission's goal sent, at most.
pub const GOAL_CHARS: usize = 200;
/// Targets offered, at most: active missions first, then the sessions
/// most recently active.
pub const MAX_TARGETS: usize = 8;
/// A message with fewer words is too short to route: Control asks.
pub const MIN_WORDS: usize = 4;

/// The instruction, read literally: the exact condition and what to use.
pub const INSTRUCTIONS: &str = "state.message is one message a person typed to the Control \
     agent, which coordinates their missions and coding sessions. Each option m<id> is one \
     mission (a goal several sessions work toward) and each option s<id> is one running coding \
     session. Decide which one mission or session the message is about, using only the \
     message's text and the option descriptions. Choose control when the message is a request \
     for the Control agent itself and is not about one listed mission or session. Choose unsure \
     when the message does not clearly point to one option.";

/// What a target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Mission,
    Session,
}

/// One place a message can be about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub kind: TargetKind,
    pub id: i64,
    /// What the receipt names it.
    pub name: String,
    /// What the model reads besides the name: a mission's goal, a
    /// session's project. Never sent to a device.
    #[serde(skip)]
    pub detail: String,
    /// The target's org, whose consent (D31) its name and detail need
    /// before they go to the model. Never sent to a device.
    #[serde(skip)]
    pub org_id: Option<i64>,
}

impl Target {
    /// PURE: this target's option word.
    pub fn option(&self) -> String {
        option_of(self.kind, self.id)
    }
}

/// What the receipt under a Control message shows. The default is `none`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRoute {
    /// `proposed` (a target, pre-selected), `ask` (Control asks where it
    /// goes, nothing pre-selected) or `none` (show nothing).
    pub outcome: String,
    /// The proposed target's option word (`m<id>` / `s<id>`), with
    /// `proposed` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<DecisionProposal>,
    /// The targets "Change" and the question offer, in order.
    #[serde(default)]
    pub targets: Vec<Target>,
    /// The recorded run a follow-up marks, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<i64>,
}

impl Default for ControlRoute {
    fn default() -> Self {
        ControlRoute::none()
    }
}

impl ControlRoute {
    fn none() -> Self {
        ControlRoute {
            outcome: "none".into(),
            target: None,
            proposal: None,
            targets: Vec::new(),
            run_id: None,
        }
    }

    fn ask(targets: Vec<Target>, run_id: Option<i64>) -> Self {
        ControlRoute {
            outcome: "ask".into(),
            target: None,
            proposal: None,
            targets,
            run_id,
        }
    }
}

/// PURE: an option word.
pub fn option_of(kind: TargetKind, id: i64) -> String {
    match kind {
        TargetKind::Mission => format!("m{id}"),
        TargetKind::Session => format!("s{id}"),
    }
}

/// PURE: whether an option word is one of `targets`, `control` or
/// `unsure` — the only words a follow-up may record.
fn known_option(word: &str, targets: &[Target]) -> bool {
    word == CONTROL || word == UNSURE || targets.iter().any(|t| t.option() == word)
}

/// PURE: a slash command (`/clear`, `/compact`) is for the agent's REPL,
/// never routed.
pub fn is_command(text: &str) -> bool {
    text.trim_start().starts_with('/')
}

/// PURE: the rule before the model: a message too short to say what it is
/// about ("yes", "do it", "and the other one") is asked about, not guessed.
pub fn unclear(text: &str) -> bool {
    text.split_whitespace().count() < MIN_WORDS
}

/// PURE: the question over `targets`.
pub fn question(targets: &[Target]) -> Question {
    let mut criteria: Vec<(String, Option<Value>)> = targets
        .iter()
        .map(|t| {
            let what = match t.kind {
                TargetKind::Mission => format!(
                    "The message is about the mission \"{}\", whose goal is: {}",
                    t.name, t.detail
                ),
                TargetKind::Session => {
                    format!(
                        "The message is about the coding session \"{}\" ({}).",
                        t.name, t.detail
                    )
                }
            };
            (t.option(), Some(Value::String(what)))
        })
        .collect();
    criteria.push((
        CONTROL.to_string(),
        Some(Value::String(
            "The message is a request for the Control agent itself, not about one listed \
             mission or session."
                .into(),
        )),
    ));
    criteria.push((
        UNSURE.to_string(),
        Some(Value::String(
            "The message does not show which mission or session it is about.".into(),
        )),
    ));
    Question::Choice {
        instructions: Value::String(INSTRUCTIONS.into()),
        criteria: criteria.into_iter().collect(),
    }
}

/// PURE: the request for `text` over `targets`.
pub fn question_for(text: &str, targets: &[Target]) -> JevRequest {
    let message: String = text.chars().take(MESSAGE_CHARS).collect();
    JevRequest {
        state: json!({ "message": message }),
        question: question(targets),
    }
}

/// The targets `scope` may see, in the order they are offered: active
/// missions (newest first), then running sessions other than Control's
/// own agent (most recently active first), [`MAX_TARGETS`] in all. Neither
/// kind starves the other (review r01): missions take at most half the
/// slots while there are sessions to fill the rest, and either kind takes
/// the slots the other leaves empty.
pub fn targets(s: &Store, scope: &ViewScope) -> Result<Vec<Target>, IpcError> {
    let mut out = Vec::new();
    let mut missions = s.list_missions()?;
    missions.retain(|m| m.state == "active");
    missions.sort_by_key(|m| std::cmp::Reverse(m.updated_at));
    for m in missions {
        if !crate::service::work::missions::sees_mission(s, scope, &m)? {
            continue;
        }
        out.push(Target {
            kind: TargetKind::Mission,
            id: m.id,
            name: m.name,
            detail: m.goal.chars().take(GOAL_CHARS).collect(),
            org_id: m.org_id,
        });
    }
    let operator = crate::service::operator::operator_ref(s);
    let mut sessions = s.list_all_sessions()?;
    sessions.retain(|r| {
        r.status == "running"
            && r.lost_at.is_none()
            && !operator
                .as_ref()
                .is_some_and(|o| o.host_alias == r.host_alias && o.tmux_name == r.tmux_name)
            && !matches!(scope.sees_session_row(r), Visibility::None)
    });
    sessions.sort_by_key(|r| std::cmp::Reverse(r.last_activity_at));
    let missions_kept = out
        .len()
        .min(MAX_TARGETS - sessions.len().min(MAX_TARGETS / 2));
    out.truncate(missions_kept);
    for r in sessions {
        let project = r
            .project_id
            .and_then(|pid| s.get_project(pid).ok().flatten())
            .map(|p| format!("{}/{}", p.owner, p.repo))
            .unwrap_or_else(|| format!("on {}", r.host_alias));
        out.push(Target {
            kind: TargetKind::Session,
            id: r.id,
            name: r
                .friendly_name
                .clone()
                .unwrap_or_else(|| r.tmux_name.clone()),
            detail: project,
            org_id: r.org_id,
        });
    }
    out.truncate(MAX_TARGETS);
    Ok(out)
}

/// The targets whose org consented to the decision model (D31; a target
/// with no org needs `decide.jev.unassigned`, which the message's own gate
/// already asked): only their names and details go out. The person is
/// still offered every target.
pub fn consenting(s: &Store, targets: &[Target]) -> Vec<Target> {
    targets
        .iter()
        .filter(|t| super::consents(s, Feature::ControlRoute, t.org_id))
        .cloned()
        .collect()
}

/// PURE: the run's subject id: 16 hex digits of an HMAC of the message and
/// when it was asked, under the local fingerprint key. The message is free
/// text a person typed and is never recorded as is.
pub fn subject_id(fp_key: &crate::store::Secret, text: &str, now: i64) -> String {
    let mac = super::hmac_sha256_hex(
        fp_key.expose().as_bytes(),
        format!("control_route.msg\n{now}\n{text}").as_bytes(),
    );
    format!("msg:{}", &mac[..16])
}

/// Route one message the person has just sent in Control. Never an error
/// a send could notice: every failure is `none`. Holds the store lock only
/// for reads, never across the call.
pub async fn propose(ctx: &DecideCtx, scope: &ViewScope, text: &str) -> ControlRoute {
    if is_command(text) || text.trim().is_empty() {
        return ControlRoute::none();
    }
    let now = ctx.now();
    let (mode, targets, asked, subject) = {
        let Ok(s) = lock(&ctx.store) else {
            return ControlRoute::none();
        };
        let Ok(mode) = gate_at(&s, Feature::ControlRoute, None, now) else {
            return ControlRoute::none();
        };
        let targets = match targets(&s, scope) {
            Ok(t) if !t.is_empty() => t,
            Ok(_) => return ControlRoute::none(),
            Err(e) => {
                tracing::warn!("[decide] control_route not asked: {}", e.message);
                return ControlRoute::none();
            }
        };
        let Ok(fp_key) = s.decision_fp_key() else {
            return ControlRoute::none();
        };
        let asked = consenting(&s, &targets);
        (mode, targets, asked, subject_id(&fp_key, text, now))
    };
    if unclear(text) {
        // The rule answered: nothing to ask the model, nothing recorded.
        return if mode == Mode::Assist {
            ControlRoute::ask(targets, None)
        } else {
            ControlRoute::none()
        };
    }
    if asked.is_empty() {
        // No target's org consented: nothing of theirs goes out, and the
        // person picks, as with the feature off.
        return if mode == Mode::Assist {
            ControlRoute::ask(targets, None)
        } else {
            ControlRoute::none()
        };
    }
    let out = decide(
        ctx,
        DecideRequest {
            feature: Feature::ControlRoute,
            subject_kind: SUBJECT_KIND.into(),
            subject_id: subject,
            org_id: None,
            request: question_for(text, &asked),
            // Today nothing routes a message: it stays with Control.
            baseline: Some(CONTROL.into()),
            question_version: QUESTION_VERSION.into(),
            min_confidence: Some(MIN_CONFIDENCE),
        },
    )
    .await;
    if out.mode != Some(Mode::Assist) {
        return ControlRoute::none();
    }
    let Some(answer) = out.proposal() else {
        // Unsure, weak or failed: ask, and let the person's pick teach.
        return ControlRoute::ask(targets, out.run_id);
    };
    if answer.value == CONTROL {
        return ControlRoute::none();
    }
    if answer.value == UNSURE || !asked.iter().any(|t| t.option() == answer.value) {
        return ControlRoute::ask(targets, out.run_id);
    }
    let proposal = DecisionProposal {
        feature: Feature::ControlRoute.as_str().to_string(),
        value: answer.value.clone(),
        source: "jev".to_string(),
        reason: None,
        confidence_pct: super::start_project::pct(answer.confidence),
        run_id: out.run_id,
        at: Some(now),
        linked: None,
    };
    ControlRoute {
        outcome: "proposed".into(),
        target: Some(answer.value.clone()),
        proposal: Some(proposal),
        targets,
        run_id: out.run_id,
    }
}

/// What the person did with a receipt: `chosen` is the option they kept
/// (opening the proposed target) or picked ("Change", or an answer to the
/// question). Marks run `run_id` `confirmed` when it is the model's answer,
/// else `corrected` to it. Only an assist run of this feature that nobody
/// has decided yet is marked; returns whether one was.
///
/// `scope` must be able to see the run (review r04): `runs { list }`'s rule
/// for a run about no session ([`crate::service::runs::reach`]: the whole
/// fleet's reader), or else every target the run offered, which is what
/// [`targets`] gave the person who sent the message. A run another caller
/// cannot see is answered like one that is not there.
pub fn follow(
    s: &Store,
    scope: &ViewScope,
    run_id: i64,
    chosen: &str,
    now: i64,
) -> Result<bool, IpcError> {
    let Some(r) = s.get_decision_run(run_id)? else {
        return Ok(false);
    };
    if r.feature != Feature::ControlRoute.as_str()
        || r.mode != Mode::Assist.as_str()
        || r.followup.is_some()
        || !r.called
    {
        return Ok(false);
    }
    let offered: Vec<Target> = r
        .candidates
        .iter()
        .filter_map(|c| {
            let (kind, id) = match c.split_at(1) {
                ("m", id) => (TargetKind::Mission, id),
                ("s", id) => (TargetKind::Session, id),
                _ => return None,
            };
            Some(Target {
                kind,
                id: id.parse().ok()?,
                name: String::new(),
                detail: String::new(),
                org_id: None,
            })
        })
        .collect();
    if !sees_offered(s, scope, &offered)? {
        return Ok(false);
    }
    if !known_option(chosen, &offered) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{chosen:?} was not one of this message's targets"),
        ));
    }
    if r.answer.as_deref() == Some(chosen) {
        s.set_decision_followup(r.id, "confirmed", None, now)
    } else {
        s.set_decision_followup(r.id, "corrected", Some(chosen), now)
    }
}

/// Whether `scope` sees a run that offered `offered` (see [`follow`]).
fn sees_offered(s: &Store, scope: &ViewScope, offered: &[Target]) -> Result<bool, IpcError> {
    use crate::store::RunsReach;
    let (sessions, missions) = match crate::service::runs::reach(s, scope)? {
        RunsReach::All | RunsReach::Scoped { spend: true, .. } => return Ok(true),
        RunsReach::Scoped {
            sessions, missions, ..
        } => (sessions, missions),
    };
    Ok(offered.iter().all(|t| match t.kind {
        TargetKind::Mission => missions.contains(&t.id),
        TargetKind::Session => sessions.contains(&t.id),
    }))
}
