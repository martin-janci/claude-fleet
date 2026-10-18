//! Detector 4: a host whose fleet files are stale, whose hooks went silent,
//! or whose agent or Claude Code is behind.

use super::{Finding, FindingBody, Fix, FixPlan, Safety, ScanCx, Severity};

fn finding(
    alias: &str,
    variant: &str,
    severity: Severity,
    title: String,
    explanation: &str,
    evidence: Vec<String>,
    fix: Option<Fix>,
) -> Finding {
    Finding {
        kind: format!("host.{variant}"),
        subject: format!("host:{alias}"),
        host_alias: Some(alias.to_string()),
        severity,
        body: FindingBody {
            title,
            explanation: explanation.into(),
            evidence,
            fix,
        },
    }
}

fn provision(alias: &str, auto: bool) -> Fix {
    Fix {
        summary: format!("Refresh fleet's files on {alias}"),
        changes: vec![format!(
            "Rewrites fleet's own skill, hooks and helper files on {alias}. Your files and \
             `~/.claude.json` are not touched."
        )],
        safety: Safety::Safe,
        reversible: true,
        auto,
        plan: FixPlan::ProvisionContent {
            host: alias.to_string(),
        },
    }
}

pub fn scan(cx: &ScanCx) -> Vec<Finding> {
    let mut out = Vec::new();
    for h in cx.health.iter().filter(|h| h.reachable) {
        let alias = h.alias.as_str();
        let row = cx.hosts.iter().find(|r| r.alias == alias);
        if row.is_some_and(|r| r.provision_stale) {
            out.push(finding(
                alias,
                "provision_stale",
                Severity::Degraded,
                format!("Fleet's files on {alias} are out of date"),
                "This hub ships newer helper files than the ones on this host. Until they are \
                 refreshed, new fleet features may not work there. Fleet refreshes stale hosts \
                 on its own every few minutes; this lets you do it now.",
                vec!["the host's provisioning fingerprint differs from this hub's".into()],
                Some(provision(alias, false)),
            ));
        }
        if h.hooks_silent {
            out.push(finding(
                alias,
                "hooks_silent",
                Severity::Degraded,
                format!("{alias} stopped reporting session events"),
                "Claude Code on this host is not calling fleet's hooks, so fleet learns about \
                 finished or waiting sessions late or not at all. Reinstalling fleet's files \
                 restores the hooks.",
                vec!["no hook call from this host recently, while it has live sessions".into()],
                Some(provision(alias, true)),
            ));
        }
        if h.agent_behind {
            out.push(finding(
                alias,
                "agent_behind",
                Severity::Degraded,
                format!("The fleet agent on {alias} is older than the hub"),
                "An older agent may not understand what this hub asks of it. Updating installs \
                 the hub's release on that host and restarts the agent; its sessions keep running.",
                vec![format!(
                    "agent version: {}",
                    h.agent_version.as_deref().unwrap_or("unknown")
                )],
                Some(Fix {
                    summary: format!("Update the agent on {alias}"),
                    changes: vec![format!(
                        "Pins the agent on {alias} to the hub's recommended release and wakes \
                         its updater, which restarts the agent."
                    )],
                    safety: Safety::NeedsApproval,
                    reversible: false,
                    auto: false,
                    plan: FixPlan::UpdateAgent {
                        host: alias.to_string(),
                    },
                }),
            ));
        }
        if h.claude_behind {
            out.push(finding(
                alias,
                "claude_behind",
                Severity::Info,
                format!("Claude Code on {alias} is behind"),
                "A newer Claude Code is in use on other hosts. Fleet cannot update Claude Code \
                 for you: run `claude update` on that host when no session there is busy.",
                vec![format!(
                    "claude version: {}",
                    h.claude_version.as_deref().unwrap_or("unknown")
                )],
                None,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::autofix::{FixPlan, Safety, ScanCx};
    use crate::service::health::HostHealthRow;
    use crate::store::{HostRow, Store};

    fn host(s: &Store, alias: &str) -> HostRow {
        s.insert_host(alias, Some(alias)).unwrap();
        s.set_host_provisioned(alias, true).unwrap();
        s.list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == alias)
            .unwrap()
    }

    fn health(alias: &str) -> HostHealthRow {
        HostHealthRow {
            alias: alias.into(),
            reachable: true,
            ..Default::default()
        }
    }

    fn kinds(cx: &ScanCx) -> Vec<String> {
        let mut k: Vec<String> = scan(cx).into_iter().map(|f| f.kind).collect();
        k.sort();
        k
    }

    #[test]
    fn a_healthy_host_has_no_findings() {
        let s = Store::open_in_memory().unwrap();
        let mut h = host(&s, "h1");
        h.reachable = true;
        let cx = ScanCx {
            hosts: vec![h],
            health: vec![health("h1")],
            ..Default::default()
        };
        assert!(scan(&cx).is_empty());
    }

    #[test]
    fn each_flag_is_its_own_finding_with_the_right_safety() {
        let s = Store::open_in_memory().unwrap();
        let mut h = host(&s, "h1");
        h.reachable = true;
        h.provision_stale = true;
        let mut hh = health("h1");
        hh.hooks_silent = true;
        hh.agent_behind = true;
        hh.claude_behind = true;
        let cx = ScanCx {
            hosts: vec![h],
            health: vec![hh],
            ..Default::default()
        };
        assert_eq!(
            kinds(&cx),
            [
                "host.agent_behind",
                "host.claude_behind",
                "host.hooks_silent",
                "host.provision_stale"
            ]
        );
        let by = |k: &str| scan(&cx).into_iter().find(|f| f.kind == k).unwrap();
        let stale = by("host.provision_stale").body.fix.unwrap();
        assert_eq!(
            (stale.safety, stale.auto),
            (Safety::Safe, false),
            "the reprovision loop owns it"
        );
        assert_eq!(stale.plan, FixPlan::ProvisionContent { host: "h1".into() });
        let hooks = by("host.hooks_silent").body.fix.unwrap();
        assert_eq!((hooks.safety, hooks.auto), (Safety::Safe, true));
        let agent = by("host.agent_behind").body.fix.unwrap();
        assert_eq!(agent.safety, Safety::NeedsApproval);
        assert_eq!(agent.plan, FixPlan::UpdateAgent { host: "h1".into() });
        assert!(
            by("host.claude_behind").body.fix.is_none(),
            "fleet has no claude update path"
        );
    }

    #[test]
    fn an_unreachable_host_reports_nothing() {
        let s = Store::open_in_memory().unwrap();
        let mut h = host(&s, "h1");
        h.reachable = false;
        h.provision_stale = true;
        let mut hh = health("h1");
        hh.reachable = false;
        hh.hooks_silent = true;
        let cx = ScanCx {
            hosts: vec![h],
            health: vec![hh],
            ..Default::default()
        };
        assert!(
            scan(&cx).is_empty(),
            "host_down is attention's job, not a fixable state"
        );
    }
}
