//! Findings → episodes in `autofix_findings`.

use super::{detector_of, Finding};
use crate::ipc_error::IpcError;
use crate::store::{NewAutofixFinding, Store};

/// How long a fix has to make its finding disappear before it counts as failed.
pub const AUTOFIX_VERIFY_SECS: i64 = 600;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub opened: Vec<i64>,
    pub cleared: Vec<i64>,
    pub failed: Vec<i64>,
}

/// Which kinds a scan is allowed to clear.
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    All,
    Detector(&'a str),
}

impl Scope<'_> {
    fn covers(&self, kind: &str) -> bool {
        match self {
            Scope::All => true,
            Scope::Detector(id) => detector_of(kind).is_some_and(|d| d.id == *id),
        }
    }
}

pub fn sync(
    s: &Store,
    found: &[Finding],
    scope: Scope<'_>,
    now: i64,
) -> Result<SyncOutcome, IpcError> {
    let mut out = SyncOutcome::default();
    let mut seen = std::collections::BTreeSet::new();
    for f in found {
        let body = serde_json::to_string(&f.body).map_err(|e| {
            IpcError::new(
                crate::ipc_error::codes::E_INTERNAL,
                format!("finding body: {e}"),
            )
        })?;
        let (id, opened) = s.upsert_autofix_finding(
            &NewAutofixFinding {
                kind: &f.kind,
                subject: &f.subject,
                host_alias: f.host_alias.as_deref(),
                severity: f.severity.as_str(),
                body: &body,
            },
            now,
        )?;
        seen.insert(id);
        if opened {
            out.opened.push(id);
        }
    }
    for row in s.live_autofix_findings()? {
        if !scope.covers(&row.kind) {
            continue;
        }
        if !seen.contains(&row.id) {
            s.set_autofix_state(row.id, "cleared", None, now)?;
            out.cleared.push(row.id);
            continue;
        }
        match row.state.as_str() {
            "dismissed" if row.dismissed_until.is_some_and(|u| u <= now) => {
                s.set_autofix_state(row.id, "open", None, now)?;
            }
            "verifying"
                if row
                    .last_attempt_at
                    .is_some_and(|t| now - t >= AUTOFIX_VERIFY_SECS) =>
            {
                s.set_autofix_state(
                    row.id,
                    "apply_failed",
                    Some("the fix ran, but the problem is still there"),
                    now,
                )?;
                out.failed.push(row.id);
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::{FindingBody, Severity};
    use crate::store::Store;

    fn f(kind: &str) -> Finding {
        Finding {
            kind: kind.into(),
            subject: "hub".into(),
            host_alias: None,
            severity: Severity::Info,
            body: FindingBody {
                title: "t".into(),
                explanation: "e".into(),
                evidence: vec![],
                fix: None,
            },
        }
    }

    #[test]
    fn found_opens_and_vanished_clears() {
        let s = Store::open_in_memory().unwrap();
        let o = sync(&s, &[f("operator_unbound")], Scope::All, 10).unwrap();
        assert_eq!(o.opened.len(), 1);
        let o = sync(&s, &[], Scope::All, 20).unwrap();
        assert_eq!(o.cleared.len(), 1);
        assert!(s.live_autofix_findings().unwrap().is_empty());
    }

    #[test]
    fn a_detector_rescan_leaves_other_detectors_alone() {
        let s = Store::open_in_memory().unwrap();
        sync(
            &s,
            &[f("operator_unbound"), f("host.hooks_silent")],
            Scope::All,
            10,
        )
        .unwrap();
        let o = sync(&s, &[], Scope::Detector("operator_unbound"), 20).unwrap();
        assert_eq!(o.cleared.len(), 1);
        let live = s.live_autofix_findings().unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].kind, "host.hooks_silent");
    }

    #[test]
    fn an_expired_dismissal_reopens() {
        let s = Store::open_in_memory().unwrap();
        let id = sync(&s, &[f("k")], Scope::All, 10).unwrap().opened[0];
        s.dismiss_autofix_finding(id, 50).unwrap();
        sync(&s, &[f("k")], Scope::All, 40).unwrap();
        assert_eq!(s.autofix_finding(id).unwrap().unwrap().state, "dismissed");
        sync(&s, &[f("k")], Scope::All, 60).unwrap();
        assert_eq!(s.autofix_finding(id).unwrap().unwrap().state, "open");
    }

    #[test]
    fn a_fix_that_did_not_help_fails_after_the_verify_window() {
        let s = Store::open_in_memory().unwrap();
        let id = sync(&s, &[f("k")], Scope::All, 10).unwrap().opened[0];
        s.mark_autofix_attempt(id, 100).unwrap();
        sync(&s, &[f("k")], Scope::All, 100 + AUTOFIX_VERIFY_SECS - 1).unwrap();
        assert_eq!(s.autofix_finding(id).unwrap().unwrap().state, "verifying");
        let o = sync(&s, &[f("k")], Scope::All, 100 + AUTOFIX_VERIFY_SECS).unwrap();
        assert_eq!(o.failed, vec![id]);
        let row = s.autofix_finding(id).unwrap().unwrap();
        assert_eq!(row.state, "apply_failed");
        assert!(row.last_error.unwrap().contains("still there"));
    }

    #[test]
    fn a_verifying_row_that_vanished_is_cleared() {
        let s = Store::open_in_memory().unwrap();
        let id = sync(&s, &[f("k")], Scope::All, 10).unwrap().opened[0];
        s.mark_autofix_attempt(id, 100).unwrap();
        let o = sync(&s, &[], Scope::All, 101).unwrap();
        assert_eq!(o.cleared, vec![id]);
    }
}
