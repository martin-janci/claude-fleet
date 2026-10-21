//! Detector 3: a live session whose pane shows a stuck state
//! (`pane_intel::StuckKind`). It lists the playbooks' states as findings
//! and delegates the keystrokes to the playbook executor:
//!
//! | kind          | fix                         | safety                                   |
//! |---------------|-----------------------------|------------------------------------------|
//! | reconnect     | press Enter                 | NeedsApproval                            |
//! | trust_prompt  | press Enter (Yes, proceed)  | NeedsApproval                            |
//! | auth_menu     | none: a login on the host   | n/a                                      |
//! | press_enter   | press Enter                 | Safe when `playbooks.press_enter` is on (the playbook runs it), else NeedsApproval |
//! | oom           | recreate                    | Safe when `playbooks.oom_recreate` is on (the playbook runs it), else NeedsApproval |
//!
//! The controller is never sent keys: its findings carry no fix.
//!
//! `reconnect` is matched on a substring anywhere in the pane's tail (tool
//! output that says "Reconnecting to db…" matches too), so its Enter is never
//! pressed by itself: in a working session, or one at a permission prompt,
//! that Enter would submit queued input or approve the preselected choice.

use super::{ClientAction, Finding, FindingBody, Fix, FixPlan, Safety, ScanCx, Severity};
use crate::store::SessionRow;

fn name(r: &SessionRow) -> &str {
    r.friendly_name
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(&r.tmux_name)
}

fn press(r: &SessionRow, kind: &str, safety: Safety, auto: bool, what: &str) -> Fix {
    Fix {
        summary: format!("Press Enter in {}", name(r)),
        changes: vec![format!(
            "Presses Enter once in the session's pane on {}: {what} Nothing is typed when \
             someone is attached to the session.",
            r.host_alias
        )],
        safety,
        reversible: false,
        auto,
        plan: FixPlan::PressEnter {
            session_id: r.id,
            host: r.host_alias.clone(),
            tmux_name: r.tmux_name.clone(),
            stuck_kind: kind.to_string(),
        },
    }
}

const CONTROLLER: &str = " This is Control's own session, and fleet never types into it: open it \
                          and answer the prompt yourself.";

pub fn scan(cx: &ScanCx) -> Vec<Finding> {
    let mut out = Vec::new();
    for r in &cx.sessions {
        if r.status != "running" || crate::store::has_no_pane(&r.kind) {
            continue;
        }
        let Some(kind) = r.stuck_kind.as_deref() else {
            continue;
        };
        // An unreachable host's pane text is stale: say nothing about it.
        if !cx
            .health
            .iter()
            .any(|h| h.alias == r.host_alias && h.reachable)
        {
            continue;
        }
        let controller = cx
            .controller
            .as_ref()
            .is_some_and(|(h, t)| h == &r.host_alias && t == &r.tmux_name);
        let n = name(r);
        let host = r.host_alias.as_str();
        let pb = &cx.playbooks;
        let (severity, title, explanation, fix, action): (
            Severity,
            String,
            String,
            Option<Fix>,
            _,
        ) = match kind {
            "reconnect" => (
                Severity::Degraded,
                format!("{n} lost its connection and is stuck reconnecting"),
                "The session's connection to Claude dropped and it is waiting to retry. \
                     Pressing Enter usually wakes it up; nothing it was doing is lost. Look at \
                     the pane first: if it shows a question or typed text instead, Enter \
                     answers or sends that, so this waits for you."
                    .to_string(),
                Some(press(
                    r,
                    kind,
                    Safety::NeedsApproval,
                    false,
                    "it nudges the reconnect.",
                )),
                None,
            ),
            "trust_prompt" => (
                Severity::Blocking,
                format!("{n} asks whether to trust its folder"),
                "Claude Code will not work in a folder until someone says it trusts the files \
                     there, so the session waits. The fix answers \"Yes, proceed\" (the \
                     preselected choice) for you. That lets Claude Code read and run what is in \
                     that folder, so it waits for you."
                    .to_string(),
                Some(press(
                    r,
                    kind,
                    Safety::NeedsApproval,
                    false,
                    "it accepts the preselected \"Yes, proceed\".",
                )),
                None,
            ),
            "auth_menu" => (
                Severity::Blocking,
                format!("{n} needs you to sign in"),
                format!(
                    "Claude Code on {host} lost its sign-in and is asking which account to \
                         use. Fleet cannot sign in for you: log in on {host}, then the session \
                         carries on."
                ),
                None,
                Some(ClientAction::Login {
                    host: host.to_string(),
                }),
            ),
            "press_enter" => (
                Severity::Degraded,
                format!("{n} is waiting for Enter"),
                if pb.press_enter {
                    "The session shows a \"Press Enter to continue\" prompt. The Press Enter \
                         playbook is on, so fleet presses it by itself; you can also do it now."
                        .to_string()
                } else {
                    "The session shows a \"Press Enter to continue\" prompt and does nothing \
                         until someone presses it. The Press Enter playbook is off, so this \
                         waits for you (turn it on in Settings › Automation to let fleet do it)."
                        .to_string()
                },
                Some(press(
                    r,
                    kind,
                    if pb.press_enter {
                        Safety::Safe
                    } else {
                        Safety::NeedsApproval
                    },
                    false,
                    "it answers the \"Press Enter to continue\" prompt.",
                )),
                None,
            ),
            "oom" => (
                Severity::Blocking,
                format!("{n} ran out of memory"),
                if pb.oom_recreate {
                    "Claude Code in this session crashed for lack of memory. The \
                         out-of-memory playbook is on, so fleet restarts it by itself (within its \
                         limits); you can also do it now."
                        .to_string()
                } else {
                    "Claude Code in this session crashed for lack of memory, so the session \
                         does nothing. Recreating it starts Claude Code again and resumes the same \
                         conversation. The out-of-memory playbook is off, so this waits for you."
                        .to_string()
                },
                Some(Fix {
                    summary: format!("Recreate {n}"),
                    changes: vec![format!(
                        "Starts the session's Claude Code again on {host}, resuming the same \
                             conversation. Whatever was running in the pane is closed."
                    )],
                    safety: if pb.oom_recreate {
                        Safety::Safe
                    } else {
                        Safety::NeedsApproval
                    },
                    reversible: false,
                    auto: false,
                    plan: FixPlan::Recreate { session_id: r.id },
                }),
                None,
            ),
            _ => continue,
        };
        let (explanation, fix) = if controller && fix.is_some() {
            (format!("{explanation}{CONTROLLER}"), None)
        } else {
            (explanation, fix)
        };
        out.push(Finding {
            kind: format!("stuck.{kind}"),
            subject: format!("session:{}", r.id),
            host_alias: Some(host.to_string()),
            severity,
            body: FindingBody {
                title,
                explanation,
                evidence: vec![format!(
                    "the pane of {} on {host} shows `{kind}`",
                    r.tmux_name
                )],
                fix,
                action,
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::health::HostHealthRow;
    use crate::service::playbooks::PlaybookConfig;
    use crate::store::Store;

    fn session(kind: &str) -> SessionRow {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h1", Some("h1")).unwrap();
        s.upsert_session("work-1", "h1", None, None, 0, 0, "running", None)
            .unwrap();
        let mut r = s.list_all_sessions().unwrap().remove(0);
        r.stuck_kind = Some(kind.into());
        r.stuck_since = Some(50);
        r
    }

    fn cx(kind: &str) -> ScanCx {
        ScanCx {
            now: 100,
            health: vec![HostHealthRow {
                alias: "h1".into(),
                reachable: true,
                ..Default::default()
            }],
            sessions: vec![session(kind)],
            ..Default::default()
        }
    }

    fn only(cx: &ScanCx) -> Finding {
        let mut f = scan(cx);
        assert_eq!(f.len(), 1, "{f:?}");
        f.remove(0)
    }

    fn press(r: &SessionRow, kind: &str) -> FixPlan {
        FixPlan::PressEnter {
            session_id: r.id,
            host: "h1".into(),
            tmux_name: "work-1".into(),
            stuck_kind: kind.into(),
        }
    }

    #[test]
    fn a_session_that_is_not_stuck_is_healthy() {
        let mut c = cx("reconnect");
        c.sessions[0].stuck_kind = None;
        assert!(scan(&c).is_empty());
    }

    #[test]
    fn reconnect_waits_for_approval() {
        let c = cx("reconnect");
        let f = only(&c);
        assert_eq!(f.kind, "stuck.reconnect");
        assert_eq!(f.subject, format!("session:{}", c.sessions[0].id));
        assert_eq!(f.host_alias.as_deref(), Some("h1"));
        let fix = f.body.fix.unwrap();
        assert_eq!(
            (fix.safety, fix.auto),
            (Safety::NeedsApproval, false),
            "a substring match is never enough to press Enter unattended"
        );
        assert_eq!(fix.plan, press(&c.sessions[0], "reconnect"));
    }

    #[test]
    fn a_trust_prompt_waits_for_approval() {
        let c = cx("trust_prompt");
        let f = only(&c);
        assert_eq!(f.kind, "stuck.trust_prompt");
        let fix = f.body.fix.unwrap();
        assert_eq!((fix.safety, fix.auto), (Safety::NeedsApproval, false));
        assert_eq!(fix.plan, press(&c.sessions[0], "trust_prompt"));
    }

    #[test]
    fn an_auth_menu_explains_and_offers_a_login() {
        let f = only(&cx("auth_menu"));
        assert_eq!(f.kind, "stuck.auth_menu");
        assert!(f.body.fix.is_none());
        assert_eq!(
            f.body.action,
            Some(ClientAction::Login { host: "h1".into() })
        );
    }

    #[test]
    fn press_enter_follows_its_playbook_toggle() {
        let mut c = cx("press_enter");
        let fix = only(&c).body.fix.unwrap();
        assert_eq!((fix.safety, fix.auto), (Safety::NeedsApproval, false));
        c.playbooks = PlaybookConfig {
            press_enter: true,
            ..Default::default()
        };
        let fix = only(&c).body.fix.unwrap();
        assert_eq!(
            (fix.safety, fix.auto),
            (Safety::Safe, false),
            "the playbook runs it, not autofix"
        );
        assert_eq!(fix.plan, press(&c.sessions[0], "press_enter"));
    }

    #[test]
    fn oom_follows_its_playbook_toggle_and_recreates() {
        let mut c = cx("oom");
        let fix = only(&c).body.fix.unwrap();
        assert_eq!(fix.safety, Safety::NeedsApproval);
        assert_eq!(
            fix.plan,
            FixPlan::Recreate {
                session_id: c.sessions[0].id
            }
        );
        c.playbooks.oom_recreate = true;
        let fix = only(&c).body.fix.unwrap();
        assert_eq!((fix.safety, fix.auto), (Safety::Safe, false));
    }

    #[test]
    fn the_controller_is_explained_but_never_sent_keys() {
        for kind in ["reconnect", "trust_prompt", "press_enter", "oom"] {
            let mut c = cx(kind);
            c.controller = Some(("h1".into(), "work-1".into()));
            let f = only(&c);
            assert!(f.body.fix.is_none(), "{kind}");
            assert!(f.body.explanation.contains("Control"), "{kind}");
        }
    }

    #[test]
    fn stopped_paneless_or_unreachable_sessions_are_not_judged() {
        let mut c = cx("reconnect");
        c.sessions[0].status = "stopped".into();
        assert!(scan(&c).is_empty(), "not running");
        let mut c = cx("reconnect");
        c.sessions[0].kind = "bg".into();
        assert!(scan(&c).is_empty(), "no pane");
        let mut c = cx("reconnect");
        c.health[0].reachable = false;
        assert!(
            scan(&c).is_empty(),
            "host unreachable: the pane text is stale"
        );
    }

    #[test]
    fn an_unknown_stuck_kind_is_ignored() {
        assert!(scan(&cx("something_new")).is_empty());
    }
}
