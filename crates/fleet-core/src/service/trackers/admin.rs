//! `work_admin` (work graph M3.1): add, configure, test and remove trackers.
//!
//! Master-only on the MCP surface, so on a paired desktop every command that
//! reaches here is `LocalOnly` ("configure on the hub", review C17); the hub's
//! operator has `fleet-hub tracker …`, which calls this same tool over
//! loopback. The credential arrives as `secret` (never echoed: the audit
//! summary drops the key, [`WorkAdminArgs`]'s `Debug` masks it) or as a
//! `credential_ref` the hub reads at use.
//!
//! No `.await` holds the store lock: each action reads what it needs, drops
//! the guard, talks to the tracker, then locks again to write.

use super::{needs_credential, provider_for, TrackerError, TrackerNet};
use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{Store, TrackerRow};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

#[derive(Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "WorkAdminParams")]
pub struct WorkAdminArgs {
    /// list|add|update|set_credential|test|remove|list_orgs|add_org|update_org|remove_org|add_rule|remove_rule|assign_host|unassign_host|assign_tracker
    pub action: String,
    /// Tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    /// jira|github|asana|linear|jira_dc (default: from the URL)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Site or any ticket URL on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    /// basic|bearer
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_kind: Option<String>,
    /// Account email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// API token; never returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// env:NAME|file:/path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    /// direct|via_host:HOST|via_cli:HOST
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// Provider settings object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    /// For remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_nonce: Option<String>,
    /// Org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// Hex colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// D7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolate_sessions: Option<bool>,
    /// Rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    /// Rule or host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// Rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<i64>,
    /// on|off|inherit
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_tidy: Option<String>,
}

impl fmt::Debug for WorkAdminArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkAdminArgs")
            .field("action", &self.action)
            .field("tracker_id", &self.tracker_id)
            .field("provider", &self.provider)
            .field("name", &self.name)
            .field("site_url", &self.site_url)
            .field("auth_kind", &self.auth_kind)
            .field("username", &self.username)
            .field(
                "secret",
                &self.secret.as_ref().map(|_| crate::logging::REDACTED),
            )
            .field("credential_ref", &self.credential_ref)
            .field("transport", &self.transport)
            .field("settings", &self.settings)
            .field("org_id", &self.org_id)
            .field("color", &self.color)
            .field("isolate_sessions", &self.isolate_sessions)
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("path_prefix", &self.path_prefix)
            .field("host_alias", &self.host_alias)
            .field("rule_id", &self.rule_id)
            .field("auto_tidy", &self.auto_tidy)
            .finish()
    }
}

impl WorkAdminArgs {
    /// The one-line audit summary: never the secret, not even its length.
    pub fn audit_summary(&self) -> String {
        format!(
            "action={} tracker_id={:?} provider={:?} site_url={:?} auth_kind={:?} \
             credential_ref={:?} transport={:?} settings={} secret={} org_id={:?} \
             rule_id={:?} host_alias={:?} owner={:?} repo={:?} path_prefix={:?} \
             isolate_sessions={:?} auto_tidy={:?}",
            self.action,
            self.tracker_id,
            self.provider,
            self.site_url,
            self.auth_kind,
            self.credential_ref,
            self.transport,
            if self.settings.is_some() {
                "set"
            } else {
                "unset"
            },
            if self.secret.is_some() {
                "set"
            } else {
                "unset"
            },
            self.org_id,
            self.rule_id,
            self.host_alias,
            self.owner,
            self.repo,
            self.path_prefix,
            self.isolate_sessions,
            self.auto_tidy,
        )
    }

    fn tracker(&self) -> Result<i64, IpcError> {
        self.tracker_id.ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!("{} needs tracker_id", self.action),
            )
        })
    }
}

/// The `work_admin` actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminAction {
    List,
    Add,
    Update,
    SetCredential,
    Test,
    Remove,
    /// Work graph M5: org administration (`service::orgs::admin`).
    Org(crate::service::orgs::OrgAction),
}

impl AdminAction {
    /// Every action name `parse` accepts (aliases aside), for the isolation
    /// matrix and the error sentence.
    pub const NAMES: &'static [&'static str] = &[
        "list",
        "add",
        "update",
        "set_credential",
        "test",
        "remove",
        "list_orgs",
        "add_org",
        "update_org",
        "remove_org",
        "add_rule",
        "remove_rule",
        "assign_host",
        "unassign_host",
        "assign_tracker",
    ];

    /// Actions that destroy something and pass the confirmation gate.
    pub fn is_removal(self) -> bool {
        matches!(
            self,
            AdminAction::Remove
                | AdminAction::Org(crate::service::orgs::OrgAction::RemoveOrg)
                | AdminAction::Org(crate::service::orgs::OrgAction::RemoveRule)
        )
    }

    pub fn parse(s: &str) -> Result<Self, IpcError> {
        if let Some(o) = crate::service::orgs::OrgAction::parse(s) {
            return Ok(AdminAction::Org(o));
        }
        Ok(match s {
            "list" | "list_trackers" => AdminAction::List,
            "add" | "add_tracker" => AdminAction::Add,
            "update" | "update_tracker" => AdminAction::Update,
            "set_credential" => AdminAction::SetCredential,
            "test" => AdminAction::Test,
            "remove" | "remove_tracker" => AdminAction::Remove,
            other => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "unknown work_admin action {other:?}; one of {}",
                        AdminAction::NAMES.join(", ")
                    ),
                ))
            }
        })
    }
}

/// `test`'s answer: the tracker as it now stands, and what the probe found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReport {
    pub tracker: TrackerRow,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// The views a sync will run.
    #[serde(default)]
    pub views: Vec<String>,
}

/// The provider a pasted URL names, by its host (M6.6's "paste any ticket
/// URL"): `*.atlassian.net` Jira Cloud, `github.com` GitHub,
/// `app.asana.com` Asana, `linear.app` Linear.
pub fn infer_provider(raw: &str) -> Option<&'static str> {
    let rest = raw.trim().split_once("://")?.1;
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?
        .to_ascii_lowercase();
    match host.as_str() {
        h if h.ends_with(".atlassian.net") => Some("jira"),
        "github.com" | "www.github.com" => Some("github"),
        "app.asana.com" => Some("asana"),
        "linear.app" => Some("linear"),
        _ => None,
    }
}

/// A site URL, or any ticket URL on the site ("connect by paste").
pub fn site_from_input(provider: &str, raw: &str) -> Result<String, IpcError> {
    if provider == "jira" {
        if let Some((site, _)) = super::jira::parse_ticket_url(raw) {
            return Ok(site);
        }
    }
    crate::store::normalize_provider_site(provider, raw)
}

/// The name a new tracker gets when none is given.
fn default_name(provider: &str, site: &str) -> String {
    let host_path = site.trim_start_matches("https://");
    match provider {
        "jira" => host_path.trim_end_matches(".atlassian.net").to_string(),
        "github" => host_path
            .strip_prefix("github.com/")
            .map(|o| format!("{o} (GitHub)"))
            .unwrap_or_else(|| "GitHub".into()),
        "asana" => "Asana".into(),
        "linear" => host_path
            .strip_prefix("linear.app/")
            .map(|w| format!("{w} (Linear)"))
            .unwrap_or_else(|| "Linear".into()),
        _ => host_path.to_string(),
    }
}

/// What a provider may be reached through: GitHub only through `gh` on a
/// host (fleet never holds a GitHub token); a CLI only where one is known.
fn check_transport(provider: &str, transport: &str) -> Result<(), IpcError> {
    let cli = transport.starts_with("via_cli:");
    match provider {
        "github" if !cli => Err(IpcError::new(
            codes::E_INVALID,
            "GitHub is read through gh on a host with its own login: \
             pass transport via_cli:<host with gh>",
        )),
        "github" => Ok(()),
        p if cli => Err(IpcError::new(
            codes::E_INVALID,
            format!("no trusted CLI is known for {p} trackers; use direct or via_host:<host>"),
        )),
        _ => Ok(()),
    }
}

/// A host-side transport (`via_host:` curl, `via_cli:` gh) runs its
/// command with the request on stdin, which a host reached through
/// fleet-agent cannot do: every request would fail `E_UNSUPPORTED` and the
/// tick would retry forever, so it is refused when the tracker is set up.
fn check_host_transport(s: &Store, transport: &str) -> Result<(), IpcError> {
    let Some(alias) = transport
        .strip_prefix("via_host:")
        .or_else(|| transport.strip_prefix("via_cli:"))
    else {
        return Ok(());
    };
    if s.agent_host_alias(alias)?.is_some() {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{alias} is reached through fleet-agent, which cannot run curl or gh for a \
                 tracker; pick a host fleet reaches over SSH"
            ),
        ));
    }
    Ok(())
}

/// The reverse of [`check_host_transport`], for the host side: a host some
/// tracker runs curl or gh on (`via_host:` / `via_cli:`) cannot be switched
/// to fleet-agent, which can run neither — the tracker would fail every
/// request and the tick would retry forever. `hosts::add_host` asks before
/// it writes `transport = agent`; the refusal names the tracker to move
/// first (`work_admin { action: update, transport }`).
pub fn refuse_agent_transport_on_tracker_host(s: &Store, alias: &str) -> Result<(), IpcError> {
    for t in s.list_trackers()? {
        let (tool, host) = if let Some(h) = t.transport.strip_prefix("via_host:") {
            ("curl", h)
        } else if let Some(h) = t.transport.strip_prefix("via_cli:") {
            ("gh", h)
        } else {
            continue;
        };
        if host == alias {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "tracker {} ({}) runs {tool} on {alias}, which fleet-agent cannot; point \
                     the tracker at a host fleet reaches over SSH (work_admin update, \
                     transport) before switching {alias} to agent",
                    t.id, t.name
                ),
            ));
        }
    }
    Ok(())
}

/// `settings` as a person sent it → validated for `provider`.
fn parse_settings(
    provider: &str,
    v: &serde_json::Value,
) -> Result<crate::store::TrackerSettings, IpcError> {
    let s: crate::store::TrackerSettings = serde_json::from_value(v.clone())
        .map_err(|e| IpcError::new(codes::E_INVALID, format!("settings: {e}")))?;
    crate::store::validate_tracker_settings(provider, s)
}

/// The non-network actions. `Test` is [`test_tracker`]; `Remove` is here
/// once the caller has passed its confirmation gate.
pub fn admin_sync(
    args: &WorkAdminArgs,
    store: &Mutex<Store>,
) -> Result<serde_json::Value, IpcError> {
    let s = lock(store)?;
    match AdminAction::parse(&args.action)? {
        AdminAction::List => json(&s.list_trackers()?),
        AdminAction::Add => {
            let raw = args
                .site_url
                .as_deref()
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "add needs site_url"))?;
            let provider = match args.provider.as_deref() {
                Some(p) => p,
                None => infer_provider(raw).unwrap_or("jira"),
            };
            let site = site_from_input(provider, raw)?;
            let transport = args
                .transport
                .as_deref()
                .map(crate::store::validate_tracker_transport)
                .transpose()?;
            check_transport(provider, transport.as_deref().unwrap_or("direct"))?;
            check_host_transport(&s, transport.as_deref().unwrap_or("direct"))?;
            let settings = args
                .settings
                .as_ref()
                .map(|v| parse_settings(provider, v))
                .transpose()?;
            let name = args
                .name
                .clone()
                .unwrap_or_else(|| default_name(provider, &site));
            let mut row = s.add_tracker(provider, &name, &site)?;
            if let Some(t) = transport {
                row = s.set_tracker_transport(row.id, &t)?;
            }
            if let Some(st) = settings {
                s.set_tracker_settings(row.id, &st)?;
                row = s.require_tracker(row.id)?;
            }
            s.emit_tracker(row.id)?;
            json(&row)
        }
        AdminAction::Update => {
            let id = args.tracker()?;
            let row = s.require_tracker(id)?;
            if args.name.is_none() && args.transport.is_none() && args.settings.is_none() {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "update needs name, transport or settings",
                ));
            }
            let settings = args
                .settings
                .as_ref()
                .map(|v| parse_settings(&row.provider, v))
                .transpose()?;
            if let Some(n) = args.name.as_deref() {
                s.rename_tracker(id, n)?;
            }
            if let Some(t) = args.transport.as_deref() {
                let t = crate::store::validate_tracker_transport(t)?;
                check_transport(&row.provider, &t)?;
                check_host_transport(&s, &t)?;
                s.set_tracker_transport(id, &t)?;
            }
            if let Some(st) = settings {
                s.set_tracker_settings(id, &st)?;
            }
            s.emit_tracker(id)?;
            json(&s.require_tracker(id)?)
        }
        AdminAction::SetCredential => {
            let id = args.tracker()?;
            let row = s.set_tracker_credential(
                id,
                args.auth_kind.as_deref().unwrap_or("basic"),
                args.username.as_deref(),
                args.secret.as_deref(),
                args.credential_ref.as_deref(),
            )?;
            s.emit_tracker(id)?;
            json(&row)
        }
        AdminAction::Remove => {
            let id = args.tracker()?;
            let removed = s.remove_tracker(id)?;
            if !removed {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("tracker {id} not found"),
                ));
            }
            s.emit_tracker_removed(id);
            json(&serde_json::json!({ "removed": id }))
        }
        AdminAction::Test => Err(IpcError::new(
            codes::E_INTERNAL,
            "test is asynchronous; use test_tracker",
        )),
        AdminAction::Org(o) => crate::service::orgs::admin(o, args, &s),
    }
}

/// `work_admin { action: test }`: probe the site, store what it learned
/// (instance id, account, key prefixes, sprint projects, views) and the
/// resulting state. The tracker row comes back either way; `ok` says which.
pub async fn test_tracker(
    id: i64,
    store: &Mutex<Store>,
    net: &TrackerNet,
) -> Result<TestReport, IpcError> {
    let (row, cred) = {
        let s = lock(store)?;
        (s.require_tracker(id)?, s.resolve_tracker_credential(id)?)
    };
    let probed = async {
        if cred.is_none() && needs_credential(&row) {
            return Err(TrackerError::Unconfigured);
        }
        let provider = provider_for(&row, cred, net)?;
        let info = provider.probe().await?;
        let views = provider.views(&info.config).await?;
        Ok((info, views))
    }
    .await;
    let s = lock(store)?;
    let report = match probed {
        Ok((info, views)) => {
            let mut changed = s.set_tracker_probe(id, info.instance_id.as_deref(), &info.config)?;
            s.sync_tracker_views(
                id,
                &views
                    .iter()
                    .map(|v| (v.id.clone(), v.label.clone(), v.query.clone()))
                    .collect::<Vec<_>>(),
            )?;
            // A view the sync disabled on a 403 runs again after a person
            // tests the tracker: nothing else ever re-enables it.
            changed |= s.enable_tracker_views(id)?;
            // The tracker answered: a 429 back-off still on the row is over.
            s.set_tracker_not_before(id, None)?;
            changed |= s.set_tracker_state(id, "ok", None)?;
            if changed {
                s.emit_tracker(id)?;
            }
            TestReport {
                tracker: s.require_tracker(id)?,
                ok: true,
                error: None,
                views: views.into_iter().map(|v| v.label).collect(),
            }
        }
        Err(e) => {
            record_failure(&s, id, &e)?;
            TestReport {
                tracker: s.require_tracker(id)?,
                ok: false,
                error: Some(crate::logging::redact(&e.explain()).into_owned()),
                views: Vec::new(),
            }
        }
    };
    Ok(report)
}

/// Store the state a failure leaves the tracker in (and emit on change).
pub(crate) fn record_failure(s: &Store, id: i64, e: &TrackerError) -> Result<(), IpcError> {
    let state = e.state().unwrap_or("ok");
    if s.set_tracker_state(id, state, Some(&e.explain()))? {
        s.emit_tracker(id)?;
    }
    Ok(())
}

fn json<T: Serialize>(v: &T) -> Result<serde_json::Value, IpcError> {
    serde_json::to_value(v).map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::https::{FakeTransport, Method, Response};
    use serde_json::json;
    use std::sync::Arc;

    const TOKEN: &str = "ATATT3xFfGF0-admin-test-token-not-real";

    fn args(action: &str) -> WorkAdminArgs {
        WorkAdminArgs {
            action: action.into(),
            ..Default::default()
        }
    }

    fn fixture(name: &str) -> serde_json::Value {
        let p = format!(
            "{}/src/service/trackers/testdata/jira/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn added() -> (Mutex<Store>, i64) {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let v = admin_sync(
            &WorkAdminArgs {
                site_url: Some("https://acme.atlassian.net/browse/ABC-1".into()),
                ..args("add")
            },
            &st,
        )
        .unwrap();
        let id = v["id"].as_i64().unwrap();
        admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                username: Some("dev@example.com".into()),
                secret: Some(TOKEN.into()),
                ..args("set_credential")
            },
            &st,
        )
        .unwrap();
        (st, id)
    }

    #[test]
    fn add_infers_the_site_from_a_ticket_url_and_no_answer_carries_the_secret() {
        let (st, id) = added();
        let listed = admin_sync(&args("list"), &st).unwrap();
        let text = listed.to_string();
        assert!(text.contains("https://acme.atlassian.net"));
        assert!(text.contains("\"name\":\"acme\""));
        assert!(!text.contains(TOKEN), "{text}");
        assert!(text.contains("\"has_credential\":true"));
        let dbg = format!(
            "{:?}",
            WorkAdminArgs {
                secret: Some(TOKEN.into()),
                ..args("set_credential")
            }
        );
        assert!(!dbg.contains(TOKEN), "{dbg}");
        let audit = WorkAdminArgs {
            secret: Some(TOKEN.into()),
            ..args("set_credential")
        }
        .audit_summary();
        assert!(
            !audit.contains(TOKEN) && audit.contains("secret=set"),
            "{audit}"
        );
        // Rename; remove.
        let v = admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                name: Some("Acme Jira".into()),
                ..args("update")
            },
            &st,
        )
        .unwrap();
        assert_eq!(v["name"], "Acme Jira");
        let v = admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                ..args("remove")
            },
            &st,
        )
        .unwrap();
        assert_eq!(v["removed"], id);
        assert_eq!(
            admin_sync(
                &WorkAdminArgs {
                    tracker_id: Some(id),
                    ..args("remove")
                },
                &st
            )
            .unwrap_err()
            .code,
            codes::E_NOTFOUND
        );
    }

    /// A host reached through fleet-agent cannot run curl or gh with the
    /// request on stdin: `via_host` / `via_cli` on it is refused at add and
    /// update time, naming fleet-agent, while an SSH host is fine.
    #[test]
    fn a_host_side_transport_on_a_fleet_agent_host_is_refused() {
        let (st, id) = added();
        {
            let s = st.lock().unwrap();
            s.upsert_host("agentbox").unwrap();
            s.set_host_transport("agentbox", "agent").unwrap();
            s.upsert_host("sshbox").unwrap();
        }
        let e = admin_sync(
            &WorkAdminArgs {
                site_url: Some("https://beta.atlassian.net".into()),
                transport: Some("via_host:agentbox".into()),
                ..args("add")
            },
            &st,
        )
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(e.message.contains("fleet-agent"), "{}", e.message);
        assert_eq!(
            admin_sync(&args("list"), &st)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let e = admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                transport: Some("via_host:agentbox".into()),
                ..args("update")
            },
            &st,
        )
        .unwrap_err();
        assert!(e.message.contains("fleet-agent"), "{}", e.message);
        assert_eq!(
            st.lock().unwrap().require_tracker(id).unwrap().transport,
            "direct"
        );
        let v = admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                transport: Some("via_host:sshbox".into()),
                ..args("update")
            },
            &st,
        )
        .unwrap();
        assert_eq!(v["transport"], "via_host:sshbox");
    }

    /// The reverse of the rule above: a host a tracker already runs curl or
    /// gh on cannot be switched to fleet-agent. `add_host` re-adding it
    /// with `transport: agent` is refused before the probe and any write,
    /// naming the tracker; the row keeps its transport, and a host no
    /// tracker runs on is free to become an agent host.
    #[tokio::test]
    async fn switching_a_tracker_host_to_fleet_agent_is_refused() {
        use crate::service::hosts::{add_host, AddHostArgs};
        let (st, id) = added();
        st.lock().unwrap().upsert_host("sshbox").unwrap();
        admin_sync(
            &WorkAdminArgs {
                tracker_id: Some(id),
                transport: Some("via_host:sshbox".into()),
                ..args("update")
            },
            &st,
        )
        .unwrap();
        let fake = crate::ssh_fake::FakeSsh::new();
        let agent = |alias: &str| AddHostArgs {
            alias: alias.into(),
            ssh_alias: alias.into(),
            transport: Some("agent".into()),
        };
        let e = add_host(agent("sshbox"), &st, &fake).await.unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(
            e.message.contains(&format!("tracker {id}"))
                && e.message.contains("curl on sshbox")
                && e.message.contains("fleet-agent"),
            "{}",
            e.message
        );
        assert!(fake.calls().is_empty(), "refused before the probe");
        {
            let s = st.lock().unwrap();
            let host = s
                .list_hosts()
                .unwrap()
                .into_iter()
                .find(|h| h.alias == "sshbox")
                .unwrap();
            assert_eq!(host.transport, "ssh");
            assert_eq!(s.require_tracker(id).unwrap().transport, "via_host:sshbox");
        }
        add_host(agent("other"), &st, &fake)
            .await
            .expect("no tracker runs on it");
    }

    #[test]
    fn the_site_fence_holds_on_add() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        for url in [
            "https://intranet.corp/browse/ABC-1",
            "http://acme.atlassian.net",
            "https://169.254.169.254/latest/meta-data",
        ] {
            let e = admin_sync(
                &WorkAdminArgs {
                    site_url: Some(url.into()),
                    ..args("add")
                },
                &st,
            )
            .unwrap_err();
            assert_eq!(e.code, codes::E_INVALID, "{url}");
        }
        assert_eq!(
            admin_sync(&args("nope"), &st).unwrap_err().code,
            codes::E_INVALID
        );
    }

    /// One successful Jira Cloud probe: identity, tenant, projects, fields,
    /// sprint check, favourite filters.
    fn probe_ok(f: &FakeTransport) {
        f.once(
            Method::Get,
            "/myself",
            Ok(Response::json(200, &fixture("myself.json"))),
        )
        .once(
            Method::Get,
            "/_edge/tenant_info",
            Ok(Response::json(200, &fixture("tenant_info.json"))),
        )
        .once(
            Method::Get,
            "startAt=0",
            Ok(Response::json(200, &fixture("project_search_p2.json"))),
        )
        .once(
            Method::Get,
            "/rest/api/3/field",
            Ok(Response::json(200, &fixture("fields.json"))),
        )
        .once(
            Method::Post,
            "/search/jql",
            Ok(Response::json(200, &json!({"issues": [], "isLast": true}))),
        )
        .once(
            Method::Get,
            "/filter/favourite",
            Ok(Response::json(200, &fixture("filter_favourite.json"))),
        );
    }

    #[tokio::test]
    async fn test_stores_the_probe_and_views_and_marks_the_tracker_ok() {
        let (st, id) = added();
        let f = FakeTransport::new();
        probe_ok(&f);
        let r = test_tracker(id, &st, &TrackerNet::fake(Arc::new(f.clone())))
            .await
            .unwrap();
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(r.tracker.state, "ok");
        assert_eq!(r.tracker.config.key_prefixes, vec!["TEAM"]);
        assert_eq!(
            r.views,
            vec!["My work", "Recent", "Team bugs", "Release blockers"],
            "no project has an open sprint: no sprint view"
        );
        assert_eq!(st.lock().unwrap().list_tracker_views(id).unwrap().len(), 4);
    }

    /// A view the sync disabled on a 403 is not disabled for good: a
    /// successful test enables it again (a failed one leaves it alone).
    #[tokio::test]
    async fn a_successful_test_enables_the_views_a_403_disabled() {
        let (st, id) = added();
        {
            let s = st.lock().unwrap();
            s.sync_tracker_views(
                id,
                &[
                    ("mine".into(), "My work".into(), "assignee = me".into()),
                    ("filter:9".into(), "Secret".into(), "filter = 9".into()),
                ],
            )
            .unwrap();
            assert!(s.set_tracker_view_enabled(id, "mine", false).unwrap());
        }
        let disabled = |st: &Mutex<Store>| -> Vec<String> {
            st.lock()
                .unwrap()
                .list_tracker_views(id)
                .unwrap()
                .into_iter()
                .filter(|v| !v.enabled)
                .map(|v| v.view_id)
                .collect()
        };
        let f = FakeTransport::new();
        f.once(Method::Get, "/myself", Ok(Response::new(401, "")));
        let r = test_tracker(id, &st, &TrackerNet::fake(Arc::new(f)))
            .await
            .unwrap();
        assert!(!r.ok);
        assert_eq!(disabled(&st), vec!["mine"], "a failed test changes nothing");
        let f = FakeTransport::new();
        probe_ok(&f);
        let r = test_tracker(id, &st, &TrackerNet::fake(Arc::new(f)))
            .await
            .unwrap();
        assert!(r.ok, "{:?}", r.error);
        let views = st.lock().unwrap().list_tracker_views(id).unwrap();
        assert!(
            views.iter().any(|v| v.view_id == "mine" && v.enabled),
            "{views:?}"
        );
        assert!(disabled(&st).is_empty(), "every view runs again");
    }

    #[tokio::test]
    async fn a_failed_test_records_the_state_and_a_redacted_reason() {
        let (st, id) = added();
        let f = FakeTransport::new();
        f.once(Method::Get, "/myself", Ok(Response::new(401, "")));
        let r = test_tracker(id, &st, &TrackerNet::fake(Arc::new(f)))
            .await
            .unwrap();
        assert!(!r.ok);
        assert_eq!(r.tracker.state, "auth_failed");
        let err = r.error.unwrap();
        assert!(err.contains("expire"), "{err}");
        assert!(!serde_json::to_string(&r.tracker).unwrap().contains(TOKEN));
    }
}
