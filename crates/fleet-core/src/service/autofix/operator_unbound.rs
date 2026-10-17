//! Detector 1: the operator's `ux-agent` client token carries no person, so
//! `Caller::view_scope` gives it a refusing scope. It then sees no sessions,
//! not even its own, and `whoami` answers E_NOTFOUND.

use super::{token_label, Finding, FindingBody, Fix, FixPlan, Safety, ScanCx, Severity};
use crate::service::operator::OPERATOR_CLIENT_NAME;

pub fn scan(cx: &ScanCx) -> Vec<Finding> {
    let Some(t) = cx.client_tokens.iter().find(|t| {
        t.name == OPERATOR_CLIENT_NAME && t.revoked_at.is_none() && t.person_id.is_none()
    }) else {
        return Vec::new();
    };
    let fix = cx.owner.map(|_| Fix {
        summary: "Link the agent's token to the hub's owner".into(),
        changes: vec![format!(
            "Sets the owner of the client token `{OPERATOR_CLIENT_NAME}` to the hub's owner. Nothing else changes."
        )],
        safety: Safety::NeedsApproval,
        reversible: true,
        auto: false,
        plan: FixPlan::BindOperator,
    });
    let explanation = if fix.is_some() {
        "Fleet shows each token only the sessions of the person it belongs to. The agent's \
         token belongs to nobody, so the agent sees no sessions, not even its own, and cannot \
         find itself (`whoami` fails). Linking the token to the hub's owner lets the agent \
         see what the owner sees."
    } else {
        "Fleet shows each token only the sessions of the person it belongs to. The agent's \
         token belongs to nobody, and this hub has no owner to link it to. Set the hub's owner \
         in Settings › People first; the fix appears here once there is one."
    };
    vec![Finding {
        kind: "operator_unbound".into(),
        subject: format!("token:{OPERATOR_CLIENT_NAME}"),
        host_alias: None,
        severity: Severity::Blocking,
        body: FindingBody {
            title: "Control can't see any sessions".into(),
            explanation: explanation.into(),
            evidence: vec![format!(
                "client token {} has no owner (person_id is empty)",
                token_label(&t.name, &t.token_sha256)
            )],
            fix,
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::{FixPlan, Safety, ScanCx};
    use crate::service::operator::OPERATOR_CLIENT_NAME;
    use crate::store::Store;

    fn cx(s: &Store) -> ScanCx {
        ScanCx {
            now: 100,
            owner: s.personal_owner_id().unwrap(),
            client_tokens: s.active_client_tokens().unwrap(),
            hosts: vec![],
            health: vec![],
        }
    }

    /// The 2026-10-17 case: the operator's token was minted with no person,
    /// so it saw no sessions and `whoami` answered E_NOTFOUND.
    #[test]
    fn an_unbound_operator_token_is_a_blocking_finding_with_a_bind_fix() {
        let s = Store::open_in_memory().unwrap();
        s.insert_client_token(OPERATOR_CLIENT_NAME, "deadbeefcafe0123", "full")
            .unwrap();
        let found = scan(&cx(&s));
        assert_eq!(found.len(), 1);
        let f = &found[0];
        assert_eq!(
            (f.kind.as_str(), f.subject.as_str()),
            ("operator_unbound", "token:ux-agent")
        );
        let fix = f.body.fix.as_ref().expect("a fix");
        assert_eq!(fix.plan, FixPlan::BindOperator);
        assert_eq!(fix.safety, Safety::NeedsApproval);
        assert!(!fix.auto);
        assert!(
            f.body
                .evidence
                .iter()
                .all(|e| !e.contains("deadbeefcafe0123")),
            "no full hash in evidence"
        );
    }

    #[test]
    fn a_bound_operator_token_is_healthy() {
        let s = Store::open_in_memory().unwrap();
        s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full")
            .unwrap();
        s.set_client_person(OPERATOR_CLIENT_NAME, s.personal_owner_id().unwrap())
            .unwrap();
        assert!(scan(&cx(&s)).is_empty());
    }

    #[test]
    fn no_operator_token_is_healthy() {
        let s = Store::open_in_memory().unwrap();
        assert!(scan(&cx(&s)).is_empty());
    }

    #[test]
    fn a_hub_without_an_owner_gets_an_explanation_and_no_fix() {
        let s = Store::open_in_memory().unwrap();
        s.insert_client_token(OPERATOR_CLIENT_NAME, "aa11", "full")
            .unwrap();
        let mut c = cx(&s);
        c.owner = None;
        let found = scan(&c);
        assert_eq!(found.len(), 1);
        assert!(found[0].body.fix.is_none());
    }
}
