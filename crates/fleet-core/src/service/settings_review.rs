//! Proposed settings changes and their review (declarative pages P5,
//! design §5, D-P4).
//!
//! An agent's change to a setting is a **proposal**: it is validated and
//! normalised like a write, but nothing is written until a person applies
//! it. This is the work graph's rule R11 applied to settings — an agent's
//! answer is only ever a pre-selected suggestion. A key whose `AiPolicy` is
//! `Never` (a confirmed change, a key another subsystem owns) cannot be
//! proposed at all. Applying goes through [`settings::set_by`], so the audit
//! trail names the person and the proposal.

use crate::ipc_error::{codes, IpcError};
use crate::service::settings::{self, Actor, AiPolicy};
use crate::store::{NewSettingProposal, SettingAuditRow, SettingProposalRow, Store};

/// Proposals waiting for review at once. A newer proposal for a key replaces
/// that key's pending one, so this bounds distinct keys, not attempts.
pub const MAX_PENDING: i64 = 50;
/// The longest `why` an agent may give.
pub const WHY_MAX_CHARS: usize = 500;
/// The longest history a caller may ask for.
pub const HISTORY_MAX: i64 = 100;

/// A pending proposal with the key's value now, which may have moved since
/// it was proposed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProposalView {
    #[serde(flatten)]
    pub row: SettingProposalRow,
    pub current: String,
}

/// What one review did.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Decided {
    pub applied: Vec<i64>,
    pub rejected: Vec<i64>,
    pub failed: Vec<DecideFailure>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecideFailure {
    pub id: i64,
    pub error: String,
}

/// Propose `value` for `key` on behalf of `actor`. Refused: an unknown key,
/// a key another subsystem owns, a value `key` does not accept, a key an
/// agent may never touch, a value it already has, and a queue that is full.
pub fn propose(
    s: &Store,
    key: &str,
    value: &str,
    why: Option<&str>,
    actor: Actor<'_>,
) -> Result<SettingProposalRow, IpcError> {
    let Some(spec) = settings::spec(key) else {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{key} is not a registered setting: get_settings {{ describe: true }} lists them"
            ),
        ));
    };
    let stored = settings::normalize(key, value)?;
    if spec.ai == AiPolicy::Never {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            format!(
                "{key} ({}) is changed only by a person, in Settings: it cannot be proposed",
                spec.label
            ),
        ));
    }
    let why = why.map(str::trim).filter(|w| !w.is_empty());
    if let Some(w) = why {
        if w.chars().count() > WHY_MAX_CHARS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("why is longer than {WHY_MAX_CHARS} characters"),
            ));
        }
    }
    let current = settings::effective_value(s, key).unwrap_or_default();
    if current == stored {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} is already {stored}"),
        ));
    }
    let replaces = s.pending_setting_proposals()?.iter().any(|p| p.key == key);
    if !replaces && s.count_pending_setting_proposals()? >= MAX_PENDING {
        return Err(IpcError::new(
            codes::E_RATE_LIMITED,
            format!("{MAX_PENDING} proposals are waiting for review: a person decides those first"),
        ));
    }
    let row = s.insert_setting_proposal(&NewSettingProposal {
        key,
        value: &stored,
        before: &current,
        why,
        source: actor.word(),
        source_detail: actor.detail(),
    })?;
    s.emit_settings_changed(key);
    Ok(row)
}

/// Every pending proposal, oldest first, with each key's value now.
pub fn pending(s: &Store) -> Result<Vec<ProposalView>, IpcError> {
    Ok(s.pending_setting_proposals()?
        .into_iter()
        .map(|row| ProposalView {
            current: settings::effective_value(s, &row.key).unwrap_or_default(),
            row,
        })
        .collect())
}

/// A person's review: apply the proposals in `accept`, reject those in
/// `reject`. Each is decided on its own: one that is no longer pending, or
/// whose value is refused now, is reported in `failed` and the rest go on.
pub fn decide(s: &Store, accept: &[i64], reject: &[i64]) -> Result<Decided, IpcError> {
    decide_as(s, accept, reject, Actor::Person)
}

/// [`decide`] on behalf of `actor`: a person on a paired device is recorded
/// as that device (declarative pages P6).
pub fn decide_as(
    s: &Store,
    accept: &[i64],
    reject: &[i64],
    actor: Actor<'_>,
) -> Result<Decided, IpcError> {
    if let Some(id) = accept.iter().find(|id| reject.contains(id)) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("proposal {id} is both applied and rejected"),
        ));
    }
    let mut out = Decided::default();
    for &id in accept {
        let Some(row) = s.setting_proposal(id)?.filter(|r| r.state == "pending") else {
            out.failed.push(DecideFailure {
                id,
                error: "no longer waiting for review".into(),
            });
            continue;
        };
        match settings::set_by(s, &row.key, &row.value, actor, Some(id)) {
            Ok(()) => {
                s.decide_setting_proposal(id, "applied", &decided_by(actor))?;
                out.applied.push(id);
            }
            Err(e) => out.failed.push(DecideFailure {
                id,
                error: e.message,
            }),
        }
    }
    for &id in reject {
        let row = s.setting_proposal(id)?;
        if s.decide_setting_proposal(id, "rejected", &decided_by(actor))? {
            if let Some(row) = row {
                s.emit_settings_changed(&row.key);
            }
            out.rejected.push(id);
        } else {
            out.failed.push(DecideFailure {
                id,
                error: "no longer waiting for review".into(),
            });
        }
    }
    Ok(out)
}

/// `decided_by` for a proposal: the actor, with its detail when it has one.
fn decided_by(actor: Actor<'_>) -> String {
    match actor.detail() {
        Some(d) => format!("{} ({d})", actor.word()),
        None => actor.word().to_string(),
    }
}

/// A caller's pending proposals and whether it may decide them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Pending {
    /// This caller may apply and reject (and write settings directly).
    pub can_write: bool,
    pub proposals: Vec<ProposalView>,
}

/// A registered key's writes, newest first, at most `limit` (clamped to
/// 1..=[`HISTORY_MAX`], 20 by default).
pub fn history(s: &Store, key: &str, limit: Option<i64>) -> Result<Vec<SettingAuditRow>, IpcError> {
    if settings::spec(key).is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{key} is not a registered setting"),
        ));
    }
    let limit = limit.unwrap_or(20).clamp(1, HISTORY_MAX);
    Ok(s.setting_audit(key, limit)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RecordingEventBus;

    const AGENT: Actor<'static> = Actor::Agent("control API");

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn a_proposal_writes_nothing_until_a_person_applies_it() {
        let s = store();
        let before = settings::effective_value(&s, "work.recent_days").unwrap();
        let p = propose(
            &s,
            "work.recent_days",
            " 3 ",
            Some("  a shorter window  "),
            AGENT,
        )
        .unwrap();
        assert_eq!(p.value, "3");
        assert_eq!(p.before, before);
        assert_eq!(p.why.as_deref(), Some("a shorter window"));
        assert_eq!(
            (p.source.as_str(), p.source_detail.as_deref()),
            ("agent", Some("control API"))
        );
        assert_eq!(
            settings::effective_value(&s, "work.recent_days").unwrap(),
            before
        );
        assert_eq!(pending(&s).unwrap()[0].current, before);

        let d = decide(&s, &[p.id], &[]).unwrap();
        assert_eq!(d.applied, [p.id]);
        assert_eq!(
            settings::effective_value(&s, "work.recent_days").unwrap(),
            "3"
        );
        assert!(pending(&s).unwrap().is_empty());
        let h = history(&s, "work.recent_days", None).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(
            (h[0].actor.as_str(), h[0].proposal_id),
            ("person", Some(p.id))
        );
        assert_eq!((h[0].before.as_deref(), h[0].after.as_str()), (None, "3"));
    }

    #[test]
    fn refuses_unknown_owned_confirmed_invalid_and_unchanged_values() {
        let s = store();
        let code = |r: Result<SettingProposalRow, IpcError>| r.unwrap_err().code;
        assert_eq!(
            code(propose(&s, "no.such", "1", None, AGENT)),
            codes::E_INVALID
        );
        assert_eq!(
            code(propose(&s, "mcp.port", "9000", None, AGENT)),
            codes::E_INVALID
        );
        assert_eq!(
            code(propose(&s, "work.recent_days", "many", None, AGENT)),
            codes::E_INVALID
        );
        // A confirmed change is a person's alone (AiPolicy::Never).
        assert_eq!(
            code(propose(&s, "work.auto_tidy", "true", None, AGENT)),
            codes::E_FORBIDDEN
        );
        let now = settings::effective_value(&s, "work.recent_days").unwrap();
        assert_eq!(
            code(propose(&s, "work.recent_days", &now, None, AGENT)),
            codes::E_INVALID
        );
        let long = "x".repeat(WHY_MAX_CHARS + 1);
        assert_eq!(
            code(propose(&s, "work.recent_days", "3", Some(&long), AGENT)),
            codes::E_INVALID
        );
        assert!(pending(&s).unwrap().is_empty());
    }

    #[test]
    fn the_queue_is_bounded_but_a_key_can_always_be_re_proposed() {
        let s = store();
        let seed = |key: &str| {
            s.insert_setting_proposal(&NewSettingProposal {
                key,
                value: "1",
                before: "0",
                why: None,
                source: "agent",
                source_detail: None,
            })
            .unwrap();
        };
        seed("work.recent_days");
        for i in 1..MAX_PENDING {
            seed(&format!("seed.{i}"));
        }
        let e = propose(&s, "playbooks.press_enter", "true", None, AGENT).unwrap_err();
        assert_eq!(e.code, codes::E_RATE_LIMITED);
        // Replacing a key's pending proposal does not grow the queue.
        propose(&s, "work.recent_days", "3", None, AGENT).unwrap();
        assert_eq!(s.count_pending_setting_proposals().unwrap(), MAX_PENDING);
    }

    #[test]
    fn review_decides_each_on_its_own_and_reports_what_it_could_not() {
        let s = store();
        let a = propose(&s, "work.recent_days", "3", None, AGENT).unwrap();
        let b = propose(&s, "playbooks.press_enter", "true", None, AGENT).unwrap();
        let d = decide(&s, &[a.id, 999], &[b.id]).unwrap();
        assert_eq!(d.applied, [a.id]);
        assert_eq!(d.rejected, [b.id]);
        assert_eq!(d.failed.len(), 1);
        assert_eq!(d.failed[0].id, 999);
        assert!(!settings::get_bool(&s, "playbooks.press_enter"));
        let again = decide(&s, &[], &[b.id]).unwrap();
        assert_eq!(again.failed.len(), 1);
        assert!(decide(&s, &[a.id], &[a.id]).is_err());
    }

    #[test]
    fn proposing_and_rejecting_tell_open_pages() {
        let bus = std::sync::Arc::new(RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        bus.take();
        let p = propose(&s, "work.recent_days", "3", None, AGENT).unwrap();
        decide(&s, &[], &[p.id]).unwrap();
        assert_eq!(
            bus.take(),
            [
                "settings:changed:work.recent_days",
                "settings:changed:work.recent_days"
            ]
        );
    }

    #[test]
    fn a_person_writing_through_set_by_is_audited_and_a_no_op_is_not() {
        let s = store();
        settings::set_by(&s, "work.recent_days", "5", Actor::Person, None).unwrap();
        settings::set_by(&s, "work.recent_days", "5", Actor::Person, None).unwrap();
        settings::set_by(&s, "work.recent_days", "6", AGENT, None).unwrap();
        let h = history(&s, "work.recent_days", Some(0)).unwrap();
        assert_eq!(h.len(), 1, "limit clamps to at least one");
        let h = history(&s, "work.recent_days", None).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(
            (h[0].actor.as_str(), h[0].actor_detail.as_deref()),
            ("agent", Some("control API"))
        );
        assert_eq!(h[1].before, None);
        assert!(history(&s, "no.such", None).is_err());
    }
}
