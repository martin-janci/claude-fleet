//! Findings as the MCP tool `autofix` and the desktop's commands show them:
//! one shape for both, so the panel, the `fix` card and Control read the
//! same thing.

use super::apply::body_of;
use super::{ClientAction, Fix};
use crate::ipc_error::{codes, IpcError};
use crate::store::{AutofixRow, Store};

/// How far back `fixed` reaches: the panel's "fixed automatically" list.
pub const FIXED_WINDOW_SECS: i64 = 86_400;
/// `dismiss` without `until`: Later, a day.
pub const DISMISS_DEFAULT_SECS: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FindingView {
    pub id: i64,
    pub kind: String,
    pub subject: String,
    #[serde(default)]
    pub host_alias: Option<String>,
    pub severity: String,
    pub state: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub attempts: i64,
    #[serde(default)]
    pub last_attempt_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub dismissed_until: Option<i64>,
    /// `None` only when the stored body cannot be read.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub fix: Option<Fix>,
    #[serde(default)]
    pub action: Option<ClientAction>,
}

impl From<&AutofixRow> for FindingView {
    fn from(r: &AutofixRow) -> Self {
        let body = body_of(r).ok();
        FindingView {
            id: r.id,
            kind: r.kind.clone(),
            subject: r.subject.clone(),
            host_alias: r.host_alias.clone(),
            severity: r.severity.clone(),
            state: r.state.clone(),
            first_seen_at: r.first_seen_at,
            last_seen_at: r.last_seen_at,
            attempts: r.attempts,
            last_attempt_at: r.last_attempt_at,
            last_error: r.last_error.clone(),
            dismissed_until: r.dismissed_until,
            title: body.as_ref().map(|b| b.title.clone()),
            explanation: body.as_ref().map(|b| b.explanation.clone()),
            evidence: body
                .as_ref()
                .map(|b| b.evidence.clone())
                .unwrap_or_default(),
            action: body.as_ref().and_then(|b| b.action.clone()),
            fix: body.and_then(|b| b.fix),
        }
    }
}

/// What a subject names, in words: `host:mercury` → "host mercury".
pub fn subject_words(subject: &str) -> String {
    match subject.split_once(':') {
        Some(("host", v)) => format!("host {v}"),
        Some(("session", v)) => format!("session #{v}"),
        Some(("token", v)) => format!("token {v}"),
        _ => subject.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Findings {
    /// The live episodes (open, verifying, apply_failed, dismissed).
    pub findings: Vec<FindingView>,
    /// Cleared in the last [`FIXED_WINDOW_SECS`] after a fix was tried.
    #[serde(default)]
    pub fixed: Vec<FindingView>,
    /// Whether this caller may apply and dismiss.
    #[serde(default)]
    pub can_apply: bool,
}

pub fn list(
    s: &Store,
    state: Option<&str>,
    can_apply: bool,
    now: i64,
) -> Result<Findings, IpcError> {
    let findings = s
        .live_autofix_findings()?
        .iter()
        .filter(|r| state.is_none_or(|st| r.state == st))
        .map(FindingView::from)
        .collect();
    let fixed = s
        .recently_fixed_autofix_findings(now - FIXED_WINDOW_SECS)?
        .iter()
        .map(FindingView::from)
        .collect();
    Ok(Findings {
        findings,
        fixed,
        can_apply,
    })
}

pub fn get(s: &Store, id: i64) -> Result<FindingView, IpcError> {
    s.autofix_finding(id)?
        .as_ref()
        .map(FindingView::from)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no finding {id}")))
}

/// Later: hide a live finding until `until` (default a day from now).
pub fn dismiss(s: &Store, id: i64, until: Option<i64>, now: i64) -> Result<FindingView, IpcError> {
    let until = until.unwrap_or(now + DISMISS_DEFAULT_SECS);
    if until <= now {
        return Err(IpcError::new(
            codes::E_INVALID,
            "until must be in the future",
        ));
    }
    if !s.dismiss_autofix_finding(id, until)? {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no live finding {id}"),
        ));
    }
    get(s, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::sync::{sync, Scope};
    use crate::service::autofix::{Finding, FindingBody, Severity};

    fn seed(s: &Store) -> i64 {
        let f = Finding {
            kind: "stuck.auth_menu".into(),
            subject: "session:1".into(),
            host_alias: Some("h1".into()),
            severity: Severity::Blocking,
            body: FindingBody {
                title: "t".into(),
                explanation: "e".into(),
                evidence: vec!["x".into()],
                fix: None,
                action: Some(ClientAction::Login { host: "h1".into() }),
            },
        };
        sync(s, &[f], Scope::All, 10).unwrap().opened[0]
    }

    #[test]
    fn a_view_carries_the_body_and_the_login_action() {
        let s = Store::open_in_memory().unwrap();
        let id = seed(&s);
        let v = get(&s, id).unwrap();
        assert_eq!(v.title.as_deref(), Some("t"));
        assert_eq!(v.evidence, vec!["x".to_string()]);
        assert_eq!(v.action, Some(ClientAction::Login { host: "h1".into() }));
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["action"]["action"], "login");
        assert_eq!(j["action"]["host"], "h1");
    }

    #[test]
    fn dismiss_defaults_to_a_day_and_refuses_the_past() {
        let s = Store::open_in_memory().unwrap();
        let id = seed(&s);
        assert_eq!(
            dismiss(&s, id, Some(5), 10).unwrap_err().code,
            codes::E_INVALID
        );
        let v = dismiss(&s, id, None, 10).unwrap();
        assert_eq!(
            (v.state.as_str(), v.dismissed_until),
            ("dismissed", Some(10 + DISMISS_DEFAULT_SECS))
        );
        assert_eq!(
            dismiss(&s, 999, None, 10).unwrap_err().code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn list_filters_by_state_and_reports_the_recently_fixed() {
        let s = Store::open_in_memory().unwrap();
        let id = seed(&s);
        s.mark_autofix_attempt(id, 11).unwrap();
        assert_eq!(list(&s, Some("open"), true, 12).unwrap().findings.len(), 0);
        assert_eq!(
            list(&s, Some("verifying"), true, 12)
                .unwrap()
                .findings
                .len(),
            1
        );
        sync(&s, &[], Scope::All, 20).unwrap();
        let l = list(&s, None, false, 30).unwrap();
        assert!(l.findings.is_empty());
        assert_eq!(l.fixed.len(), 1);
        assert!(!l.can_apply);
    }
}
