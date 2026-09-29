//! Flows (declarative pages P4b, layout L3): a wizard whose steps the
//! BACKEND decides, in the manner of Home Assistant's config flows. The
//! renderer shows whatever step comes back and posts its values; which step
//! is next, what it asks for and what finally happens are Rust here, never
//! logic in a page spec.
//!
//! A flow's state lives in this process under a random id, with its secret
//! fields left out: a token is used in the submit that carries it, never
//! stored in the state and never sent back in a step. A flow nobody finishes
//! expires ([`TTL`]); at most [`MAX_FLOWS`] are open at once.
//!
//! The one flow today is `tracker.connect`: paste any ticket or issue URL,
//! then give only what that provider needs; the tracker is added (or the
//! existing one updated), its credential set, and it is tested before the
//! flow says done. A failed test leaves the person on the same step with the
//! tracker's own error.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::trackers::admin::{self, WorkAdminArgs};
use crate::service::trackers::infer;
use crate::service::trackers::TrackerNet;
use crate::store::Store;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long an unfinished flow is kept.
pub const TTL: Duration = Duration::from_secs(30 * 60);
/// Most flows open at once; the oldest goes first.
pub const MAX_FLOWS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepFieldKind {
    Text {
        placeholder: &'static str,
    },
    /// Write-only: never prefilled, never echoed.
    Secret,
    Textarea,
    Bool,
    Select {
        options: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepField {
    pub name: &'static str,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(flatten)]
    pub kind: StepFieldKind,
    /// The value to start from (never a secret's).
    pub value: String,
    pub required: bool,
}

/// One step as the renderer shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub flow_id: String,
    pub flow: &'static str,
    pub step: &'static str,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    pub fields: Vec<StepField>,
    pub submit: &'static str,
    /// Why the last submit did not go through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A Back button returns to the first step, values kept.
    pub back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Outcome {
    Step(Step),
    Done {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        record_id: Option<i64>,
    },
}

/// Every flow: its id and title. A page names one by id (a resource's
/// `create_flow`).
pub const FLOWS: &[(&str, &str)] = &[("tracker.connect", "Connect a tracker")];

struct FlowState {
    flow: &'static str,
    step: &'static str,
    /// What has been entered so far, secrets left out.
    values: BTreeMap<String, String>,
    touched: Instant,
}

fn registry() -> &'static Mutex<HashMap<String, FlowState>> {
    static R: OnceLock<Mutex<HashMap<String, FlowState>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune(map: &mut HashMap<String, FlowState>) {
    map.retain(|_, f| f.touched.elapsed() < TTL);
    while map.len() >= MAX_FLOWS {
        let oldest = map
            .iter()
            .min_by_key(|(_, f)| f.touched)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
}

fn unknown(flow_id: &str) -> IpcError {
    IpcError::new(
        codes::E_NOTFOUND,
        format!("flow {flow_id} is not open (it finished, was cancelled or expired): start again"),
    )
}

/// Open `flow`, prefilled from `prefill` (e.g. a URL a chip already knows).
pub fn start(
    store: &Mutex<Store>,
    flow: &str,
    prefill: &BTreeMap<String, String>,
) -> Result<Step, IpcError> {
    let (flow, _) = FLOWS
        .iter()
        .find(|(id, _)| *id == flow)
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("unknown flow {flow}")))?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let values: BTreeMap<String, String> = prefill
        .iter()
        .filter(|(k, _)| !is_secret(flow, k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let state = FlowState {
        flow,
        step: first_step(flow),
        values,
        touched: Instant::now(),
    };
    let step = render(store, &id, &state, None)?;
    let mut map = registry().lock().unwrap_or_else(|p| p.into_inner());
    prune(&mut map);
    map.insert(id, state);
    Ok(step)
}

/// Back to the first step, what was entered kept.
pub fn back(store: &Mutex<Store>, flow_id: &str) -> Result<Step, IpcError> {
    let (flow, values) = {
        let mut map = registry().lock().unwrap_or_else(|p| p.into_inner());
        let f = map.get_mut(flow_id).ok_or_else(|| unknown(flow_id))?;
        f.step = first_step(f.flow);
        f.touched = Instant::now();
        (f.flow, f.values.clone())
    };
    let state = FlowState {
        flow,
        step: first_step(flow),
        values,
        touched: Instant::now(),
    };
    render(store, flow_id, &state, None)
}

pub fn cancel(flow_id: &str) {
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(flow_id);
}

/// Submit the current step's `values`: the next step, the same step with
/// an error, or done.
pub async fn submit(
    store: &Mutex<Store>,
    net: &TrackerNet,
    flow_id: &str,
    values: &BTreeMap<String, String>,
) -> Result<Outcome, IpcError> {
    let (flow, step, mut kept) = {
        let map = registry().lock().unwrap_or_else(|p| p.into_inner());
        let f = map.get(flow_id).ok_or_else(|| unknown(flow_id))?;
        (f.flow, f.step, f.values.clone())
    };
    // The step's own values over what came before; secrets used here only.
    let mut all = kept.clone();
    for (k, v) in values {
        all.insert(k.clone(), v.clone());
        if !is_secret(flow, k) {
            kept.insert(k.clone(), v.clone());
        }
    }
    let result = match flow {
        "tracker.connect" => tracker_connect::submit(store, net, step, &all).await,
        _ => Err(IpcError::new(
            codes::E_INVALID,
            format!("unknown flow {flow}"),
        )),
    };
    let (next, error) = match result {
        Ok(Next::Step(s)) => (s, None),
        Ok(Next::Done { message, record_id }) => {
            cancel(flow_id);
            return Ok(Outcome::Done { message, record_id });
        }
        Err(e) => (step, Some(e.message)),
    };
    let state = FlowState {
        flow,
        step: next,
        values: kept,
        touched: Instant::now(),
    };
    let rendered = render(store, flow_id, &state, error)?;
    registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(flow_id.to_string(), state);
    Ok(Outcome::Step(rendered))
}

enum Next {
    Step(&'static str),
    Done {
        message: String,
        record_id: Option<i64>,
    },
}

fn first_step(flow: &str) -> &'static str {
    match flow {
        "tracker.connect" => "url",
        _ => "",
    }
}

fn is_secret(flow: &str, field: &str) -> bool {
    matches!((flow, field), ("tracker.connect", "token"))
}

fn render(
    store: &Mutex<Store>,
    flow_id: &str,
    state: &FlowState,
    error: Option<String>,
) -> Result<Step, IpcError> {
    let mut step = match state.flow {
        "tracker.connect" => tracker_connect::render(store, state.step, &state.values)?,
        _ => return Err(IpcError::new(codes::E_INVALID, "unknown flow")),
    };
    step.flow_id = flow_id.to_string();
    step.error = error;
    Ok(step)
}

mod tracker_connect {
    use super::*;

    const FLOW: &str = "tracker.connect";

    /// `(id, label, what it needs, secret label, secret help)`; the same
    /// table as `PROVIDERS` in `src/lib/trackers.ts`.
    const PROVIDERS: &[(&str, &str, Needs, &str, &str)] = &[
        (
            "jira",
            "Jira Cloud",
            Needs::EmailToken,
            "API token",
            "Create one at id.atlassian.com → Security → API tokens.",
        ),
        (
            "jira_dc",
            "Jira Data Center",
            Needs::Token,
            "Personal access token",
            "Profile → Personal Access Tokens on your Jira server.",
        ),
        ("github", "GitHub", Needs::HostWithGh, "", ""),
        (
            "asana",
            "Asana",
            Needs::Token,
            "Personal access token",
            "Asana → Settings → Apps → Developer apps → Personal access tokens.",
        ),
        (
            "linear",
            "Linear",
            Needs::Token,
            "API key",
            "Linear → Settings → Security & access → Personal API keys.",
        ),
    ];

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Needs {
        EmailToken,
        Token,
        HostWithGh,
    }

    fn provider(
        id: &str,
    ) -> Option<&'static (
        &'static str,
        &'static str,
        Needs,
        &'static str,
        &'static str,
    )> {
        PROVIDERS.iter().find(|p| p.0 == id)
    }

    fn v<'a>(values: &'a BTreeMap<String, String>, k: &str) -> &'a str {
        values.get(k).map(|s| s.trim()).unwrap_or("")
    }

    /// The provider and site the URL step settles on: a provider picked by
    /// hand wins; Data Center (or overriding the guess) takes the URL as
    /// typed.
    fn settle(
        values: &BTreeMap<String, String>,
    ) -> Option<(&'static str, String, String, Option<String>)> {
        let url = v(values, "url");
        let picked = v(values, "picked");
        let guess = infer::infer(url);
        let prov = if picked.is_empty() {
            guess.as_ref()?.provider
        } else {
            provider(picked)?.0
        };
        match guess {
            Some(g) if g.provider == prov => Some((prov, g.site, g.key, g.hostname)),
            _ => Some((prov, url.to_string(), String::new(), None)),
        }
    }

    fn is_ghes(prov: &str, site: &str) -> bool {
        if prov != "github" {
            return false;
        }
        let host = site
            .strip_prefix("https://")
            .and_then(|r| r.split('/').next())
            .unwrap_or("")
            .to_ascii_lowercase();
        !host.is_empty() && host != "github.com" && host != "www.github.com"
    }

    pub(super) fn render(
        store: &Mutex<Store>,
        step: &'static str,
        values: &BTreeMap<String, String>,
    ) -> Result<Step, IpcError> {
        let base = |title: String, intro: Option<String>, fields, submit, back| Step {
            flow_id: String::new(),
            flow: FLOW,
            step,
            title,
            intro,
            fields,
            submit,
            error: None,
            back,
        };
        if step == "url" {
            let mut options = vec![(String::new(), "From the URL".to_string())];
            options.extend(PROVIDERS.iter().map(|p| (p.0.to_string(), p.1.to_string())));
            return Ok(base(
                "Connect a tracker".into(),
                Some("Paste any ticket or issue URL, or the site. The tracker and the site are read from it; Jira Data Center is picked by hand.".into()),
                vec![
                    StepField {
                        name: "url",
                        label: "Ticket or issue URL".into(),
                        help: None,
                        kind: StepFieldKind::Text {
                            placeholder: "https://acme.atlassian.net/browse/ABC-123",
                        },
                        value: v(values, "url").to_string(),
                        required: true,
                    },
                    StepField {
                        name: "picked",
                        label: "Tracker".into(),
                        help: None,
                        kind: StepFieldKind::Select { options },
                        value: v(values, "picked").to_string(),
                        required: false,
                    },
                ],
                "Next",
                false,
            ));
        }
        let (prov, site, key, hostname) = settle(values)
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "the URL step has no tracker yet"))?;
        let (_, label, needs, secret_label, secret_help) = *provider(prov).expect("settled");
        let intro = Some(if key.is_empty() {
            site.clone()
        } else {
            format!("{site} · {key}")
        });
        let mut fields = Vec::new();
        match needs {
            Needs::HostWithGh => {
                if is_ghes(prov, &site) {
                    fields.push(StepField {
                        name: "hostname",
                        label: "GitHub Enterprise hostname".into(),
                        help: Some("Passed to gh --hostname; add :port if the instance has one. Log gh in to it on the host below.".into()),
                        kind: StepFieldKind::Text {
                            placeholder: "ghe.example.com",
                        },
                        value: values
                            .get("hostname")
                            .cloned()
                            .or(hostname)
                            .unwrap_or_default(),
                        required: true,
                    });
                }
                let hosts = lock(store)?
                    .list_hosts()
                    .map_err(|e| IpcError::new(codes::E_SQLITE, e.to_string()))?;
                let mut options = vec![(String::new(), "Choose…".to_string())];
                options.extend(hosts.into_iter().map(|h| {
                    let label = if h.reachable {
                        h.alias.clone()
                    } else {
                        format!("{} (unreachable)", h.alias)
                    };
                    (h.alias, label)
                }));
                fields.push(StepField {
                    name: "gh_host",
                    label: "A host where gh is logged in".into(),
                    help: Some(
                        "Fleet runs gh on that host with its own login and stores no GitHub token."
                            .into(),
                    ),
                    kind: StepFieldKind::Select { options },
                    value: v(values, "gh_host").to_string(),
                    required: true,
                });
            }
            Needs::EmailToken | Needs::Token => {
                if needs == Needs::EmailToken {
                    fields.push(StepField {
                        name: "email",
                        label: "Atlassian account email".into(),
                        help: None,
                        kind: StepFieldKind::Text {
                            placeholder: "you@example.com",
                        },
                        value: v(values, "email").to_string(),
                        required: true,
                    });
                }
                fields.push(StepField {
                    name: "token",
                    label: secret_label.into(),
                    help: Some(format!(
                        "{secret_help} It is stored on this machine and never shown again."
                    )),
                    kind: StepFieldKind::Secret,
                    value: String::new(),
                    required: true,
                });
                if prov == "jira_dc" {
                    fields.push(StepField {
                        name: "extra_ca",
                        label: "Internal CA (PEM, optional)".into(),
                        help: None,
                        kind: StepFieldKind::Textarea,
                        value: v(values, "extra_ca").to_string(),
                        required: false,
                    });
                    fields.push(StepField {
                        name: "allow_private",
                        label: "The site resolves to this machine or a link-local address (off: refused)".into(),
                        help: None,
                        kind: StepFieldKind::Bool,
                        value: v(values, "allow_private").to_string(),
                        required: false,
                    });
                }
            }
        }
        Ok(base(
            format!("Connect {label}"),
            intro,
            fields,
            "Connect",
            true,
        ))
    }

    pub(super) async fn submit(
        store: &Mutex<Store>,
        net: &TrackerNet,
        step: &'static str,
        values: &BTreeMap<String, String>,
    ) -> Result<Next, IpcError> {
        let invalid = |m: &str| Err(IpcError::new(codes::E_INVALID, m.to_string()));
        if step == "url" {
            if v(values, "url").is_empty() {
                return invalid("Paste a ticket or issue URL, or the site.");
            }
            if settle(values).is_none() {
                return invalid("Not a URL fleet recognises (Jira Cloud, GitHub, GitHub Enterprise, Asana, Linear), or pick Jira Data Center.");
            }
            return Ok(Next::Step("details"));
        }
        let Some((prov, site, _, _)) = settle(values) else {
            return invalid("Start again from the URL.");
        };
        let (_, _, needs, _, _) = *provider(prov).expect("settled");
        let ghes = is_ghes(prov, &site);
        match needs {
            Needs::HostWithGh if v(values, "gh_host").is_empty() => {
                return invalid("Choose a host where gh is logged in.")
            }
            Needs::HostWithGh if ghes && v(values, "hostname").is_empty() => {
                return invalid("Give the GitHub Enterprise hostname.")
            }
            Needs::EmailToken if v(values, "email").len() < 4 => {
                return invalid("Give the Atlassian account email.")
            }
            Needs::EmailToken | Needs::Token if v(values, "token").is_empty() => {
                return invalid("Give the credential.")
            }
            _ => {}
        }
        let transport =
            (needs == Needs::HostWithGh).then(|| format!("via_cli:{}", v(values, "gh_host")));
        let extra_ca = v(values, "extra_ca");
        let allow_private = v(values, "allow_private") == "true";
        let settings = if prov == "jira_dc" && (!extra_ca.is_empty() || allow_private) {
            Some(serde_json::json!({
                "extra_ca": if extra_ca.is_empty() { None } else { Some(extra_ca) },
                "allow_private_network": allow_private,
            }))
        } else if ghes {
            Some(serde_json::json!({ "hostname": v(values, "hostname") }))
        } else {
            None
        };

        // Re-connecting a site that is already a row (say, after a failed
        // test): what the form carries beyond the credential lives on that
        // row, so it is updated there rather than dropped.
        let existing = lock(store)?
            .list_trackers()?
            .into_iter()
            .find(|t| t.provider == prov && t.site_url == site);
        let row: crate::store::TrackerRow = match existing {
            None => serde_json::from_value(admin::admin_sync(
                &WorkAdminArgs {
                    action: "add".into(),
                    // The site the URL step settled on, not the pasted URL:
                    // an enterprise issue URL may carry a port, which a
                    // site refuses (the port travels in `hostname`).
                    site_url: Some(site.clone()),
                    provider: Some(prov.to_string()),
                    transport: transport.clone(),
                    settings,
                    ..Default::default()
                },
                store,
            )?)
            .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?,
            Some(t) => {
                let changed_transport = transport.as_ref().filter(|x| **x != t.transport);
                if changed_transport.is_some() || settings.is_some() {
                    let mut merged = serde_json::to_value(&t.settings).unwrap_or_default();
                    if let (
                        Some(serde_json::Value::Object(m)),
                        Some(serde_json::Value::Object(add)),
                    ) = (Some(&mut merged), settings.as_ref())
                    {
                        for (k, val) in add {
                            m.insert(k.clone(), val.clone());
                        }
                    }
                    serde_json::from_value(admin::admin_sync(
                        &WorkAdminArgs {
                            action: "update".into(),
                            tracker_id: Some(t.id),
                            transport: changed_transport.cloned(),
                            settings: settings.is_some().then_some(merged),
                            ..Default::default()
                        },
                        store,
                    )?)
                    .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?
                } else {
                    t
                }
            }
        };
        if needs != Needs::HostWithGh {
            let email = v(values, "email");
            admin::admin_sync(
                &WorkAdminArgs {
                    action: "set_credential".into(),
                    tracker_id: Some(row.id),
                    auth_kind: Some(
                        if needs == Needs::EmailToken {
                            "basic"
                        } else {
                            "bearer"
                        }
                        .into(),
                    ),
                    username: (needs == Needs::EmailToken).then(|| email.to_string()),
                    secret: Some(v(values, "token").to_string()),
                    ..Default::default()
                },
                store,
            )?;
        }
        let report = admin::test_tracker(row.id, store, net).await?;
        if !report.ok {
            return Err(IpcError::new(
                codes::E_INVALID,
                report.error.unwrap_or_else(|| "the test failed".into()),
            ));
        }
        Ok(Next::Done {
            message: format!(
                "Connected {} — your work appears in ⌘K within one sync.",
                row.site_url
            ),
            record_id: Some(row.id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::https::{FakeTransport, Method, Response};
    use std::sync::Arc;

    fn vals(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn step(o: Outcome) -> Step {
        match o {
            Outcome::Step(s) => s,
            Outcome::Done { message, .. } => panic!("done early: {message}"),
        }
    }

    fn names(s: &Step) -> Vec<&str> {
        s.fields.iter().map(|f| f.name).collect()
    }

    fn stored(flow_id: &str) -> BTreeMap<String, String> {
        registry()
            .lock()
            .unwrap()
            .get(flow_id)
            .unwrap()
            .values
            .clone()
    }

    fn failing_net() -> TrackerNet {
        let f = FakeTransport::new();
        f.once(Method::Get, "/myself", Ok(Response::new(401, "")));
        TrackerNet::fake(Arc::new(f))
    }

    #[tokio::test]
    async fn the_url_step_reads_the_provider_and_refuses_what_it_cannot() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let s = start(&st, "tracker.connect", &BTreeMap::new()).unwrap();
        assert_eq!(s.step, "url");
        assert_eq!(names(&s), ["url", "picked"]);
        assert!(!s.back);
        let net = failing_net();

        let again = step(
            submit(
                &st,
                &net,
                &s.flow_id,
                &vals(&[("url", "https://jira.corp.example/browse/X-1")]),
            )
            .await
            .unwrap(),
        );
        assert_eq!(again.step, "url");
        assert!(again.error.unwrap().contains("pick Jira Data Center"));
        assert_eq!(
            again.fields[0].value, "https://jira.corp.example/browse/X-1",
            "what was typed stays"
        );

        let d = step(
            submit(
                &st,
                &net,
                &s.flow_id,
                &vals(&[("url", "https://acme.atlassian.net/browse/abc-12")]),
            )
            .await
            .unwrap(),
        );
        assert_eq!(d.step, "details");
        assert_eq!(d.title, "Connect Jira Cloud");
        assert_eq!(
            d.intro.as_deref(),
            Some("https://acme.atlassian.net · ABC-12")
        );
        assert_eq!(names(&d), ["email", "token"]);
        assert_eq!(d.fields[1].kind, StepFieldKind::Secret);
        assert!(d.back);
    }

    #[tokio::test]
    async fn data_center_is_picked_by_hand_and_github_asks_for_a_host() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        st.lock().unwrap().upsert_host("devbox").unwrap();
        let net = failing_net();
        let s = start(&st, "tracker.connect", &BTreeMap::new()).unwrap();
        let d = step(
            submit(
                &st,
                &net,
                &s.flow_id,
                &vals(&[("url", "https://jira.corp.example"), ("picked", "jira_dc")]),
            )
            .await
            .unwrap(),
        );
        assert_eq!(names(&d), ["token", "extra_ca", "allow_private"]);

        let s = start(
            &st,
            "tracker.connect",
            &vals(&[("url", "https://ghe.corp.example:8443/acme/api/issues/7")]),
        )
        .unwrap();
        assert_eq!(
            s.fields[0].value, "https://ghe.corp.example:8443/acme/api/issues/7",
            "prefilled"
        );
        let d = step(
            submit(&st, &net, &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        assert_eq!(names(&d), ["hostname", "gh_host"]);
        assert_eq!(d.fields[0].value, "ghe.corp.example:8443");
        let StepFieldKind::Select { options } = &d.fields[1].kind else {
            panic!()
        };
        assert!(options.iter().any(|(v, _)| v == "devbox"));
        let refused = step(
            submit(&st, &net, &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        assert!(refused.error.unwrap().contains("Choose a host"));
    }

    /// The token is used in the submit that carries it and kept nowhere: not
    /// in the flow's state, not in the step sent back. A failed test says the
    /// tracker's own error and leaves the person on the step; the second
    /// try updates the row the first one added instead of adding another.
    #[tokio::test]
    async fn a_failed_test_keeps_the_step_and_never_the_token() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let s = start(
            &st,
            "tracker.connect",
            &vals(&[("url", "https://acme.atlassian.net/browse/ABC-1")]),
        )
        .unwrap();
        let net = failing_net();
        step(
            submit(&st, &net, &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        let creds = vals(&[("email", "dev@example.com"), ("token", "ATATT-not-real")]);
        let again = step(submit(&st, &net, &s.flow_id, &creds).await.unwrap());
        assert_eq!(again.step, "details");
        assert!(again.error.is_some());
        assert!(again
            .fields
            .iter()
            .all(|f| f.name != "token" || f.value.is_empty()));
        assert_eq!(stored(&s.flow_id).get("token"), None);
        assert_eq!(
            stored(&s.flow_id).get("email").map(String::as_str),
            Some("dev@example.com")
        );
        let json = serde_json::to_string(&again).unwrap();
        assert!(!json.contains("ATATT-not-real"));
        {
            let s = st.lock().unwrap();
            let rows = s.list_trackers().unwrap();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].has_credential);
        }
        step(
            submit(&st, &failing_net(), &s.flow_id, &creds)
                .await
                .unwrap(),
        );
        assert_eq!(
            st.lock().unwrap().list_trackers().unwrap().len(),
            1,
            "updated, not added again"
        );
    }

    #[tokio::test]
    async fn a_passing_test_is_done_and_the_flow_closes() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let s = start(
            &st,
            "tracker.connect",
            &vals(&[("url", "https://acme.atlassian.net/browse/ABC-1")]),
        )
        .unwrap();
        let f = FakeTransport::new();
        crate::service::trackers::admin::tests::probe_ok(&f);
        let net = TrackerNet::fake(Arc::new(f));
        step(
            submit(&st, &net, &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        let done = submit(
            &st,
            &net,
            &s.flow_id,
            &vals(&[("email", "dev@example.com"), ("token", "t")]),
        )
        .await
        .unwrap();
        let Outcome::Done { message, record_id } = done else {
            panic!("{done:?}")
        };
        assert!(message.starts_with("Connected https://acme.atlassian.net"));
        assert!(record_id.is_some());
        let err = submit(&st, &net, &s.flow_id, &BTreeMap::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
    }

    fn only_tracker(st: &Mutex<Store>) -> crate::store::TrackerRow {
        let rows = st.lock().unwrap().list_trackers().unwrap();
        assert_eq!(rows.len(), 1, "one row, however many tries");
        rows.into_iter().next().unwrap()
    }

    /// GitHub goes through `gh` on a host: no credential is asked for or
    /// stored. Connecting the same site again with another host updates the
    /// row's transport (the WorkSettings cases of work graph M6).
    #[tokio::test]
    async fn github_goes_through_gh_on_a_host_and_a_reconnect_updates_the_transport() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        st.lock().unwrap().upsert_host("devbox").unwrap();
        st.lock().unwrap().upsert_host("other").unwrap();
        let s = start(
            &st,
            "tracker.connect",
            &vals(&[("url", "https://github.com/acme/api/issues/1")]),
        )
        .unwrap();
        let d = step(
            submit(&st, &failing_net(), &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        assert_eq!(names(&d), ["gh_host"], "no token for GitHub");
        step(
            submit(
                &st,
                &failing_net(),
                &s.flow_id,
                &vals(&[("gh_host", "devbox")]),
            )
            .await
            .unwrap(),
        );
        let t = only_tracker(&st);
        assert_eq!(t.provider, "github");
        assert_eq!(t.transport, "via_cli:devbox");
        assert!(!t.has_credential);
        step(
            submit(
                &st,
                &failing_net(),
                &s.flow_id,
                &vals(&[("gh_host", "other")]),
            )
            .await
            .unwrap(),
        );
        assert_eq!(only_tracker(&st).transport, "via_cli:other");
    }

    #[tokio::test]
    async fn a_data_center_reconnect_updates_its_ca_and_private_network_flag() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let s = start(
            &st,
            "tracker.connect",
            &vals(&[("url", "https://jira.corp.example"), ("picked", "jira_dc")]),
        )
        .unwrap();
        step(
            submit(&st, &failing_net(), &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        step(
            submit(&st, &failing_net(), &s.flow_id, &vals(&[("token", "pat")]))
                .await
                .unwrap(),
        );
        let t = only_tracker(&st);
        assert_eq!(t.provider, "jira_dc");
        assert_eq!(t.settings.extra_ca, None);
        step(
            submit(
                &st,
                &failing_net(),
                &s.flow_id,
                &vals(&[
                    ("token", "pat"),
                    ("extra_ca", "-----BEGIN CERTIFICATE-----"),
                    ("allow_private", "true"),
                ]),
            )
            .await
            .unwrap(),
        );
        let t = only_tracker(&st);
        assert_eq!(
            t.settings.extra_ca.as_deref(),
            Some("-----BEGIN CERTIFICATE-----")
        );
        assert!(t.settings.allow_private_network);
    }

    #[tokio::test]
    async fn asana_takes_a_token_and_no_email() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let s = start(
            &st,
            "tracker.connect",
            &vals(&[(
                "url",
                "https://app.asana.com/0/1200000000001001/1207000000000001",
            )]),
        )
        .unwrap();
        let d = step(
            submit(&st, &failing_net(), &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        assert_eq!(names(&d), ["token"]);
        step(
            submit(&st, &failing_net(), &s.flow_id, &vals(&[("token", "pat")]))
                .await
                .unwrap(),
        );
        let t = only_tracker(&st);
        assert!(t.has_credential);
        assert_eq!(t.username, None);
        assert_eq!(t.auth_kind.as_deref(), Some("bearer"));
    }

    /// An enterprise instance: the hostname is sent as edited, and there is
    /// no connecting without one. A URL on an unknown host that is not
    /// GitHub-shaped is not recognised at all.
    #[tokio::test]
    async fn the_enterprise_hostname_is_sent_as_edited_and_required() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        st.lock().unwrap().upsert_host("devbox").unwrap();
        let s = start(
            &st,
            "tracker.connect",
            &vals(&[("url", "https://ghe.corp.example:8443/acme/api/issues/7")]),
        )
        .unwrap();
        step(
            submit(&st, &failing_net(), &s.flow_id, &BTreeMap::new())
                .await
                .unwrap(),
        );
        let refused = step(
            submit(
                &st,
                &failing_net(),
                &s.flow_id,
                &vals(&[("hostname", ""), ("gh_host", "devbox")]),
            )
            .await
            .unwrap(),
        );
        assert!(refused.error.unwrap().contains("hostname"));
        assert!(
            st.lock().unwrap().list_trackers().unwrap().is_empty(),
            "nothing added"
        );
        let dbg = step(
            submit(
                &st,
                &failing_net(),
                &s.flow_id,
                &vals(&[("hostname", "ghe.corp.example:9443"), ("gh_host", "devbox")]),
            )
            .await
            .unwrap(),
        );
        assert_eq!(
            dbg.error.as_deref().map(|e| e.contains("a port")),
            Some(false),
            "{:?}",
            dbg.error
        );
        assert_eq!(
            only_tracker(&st).settings.hostname.as_deref(),
            Some("ghe.corp.example:9443")
        );

        let u = start(&st, "tracker.connect", &BTreeMap::new()).unwrap();
        let again = step(
            submit(
                &st,
                &failing_net(),
                &u.flow_id,
                &vals(&[("url", "https://tracker.example.com/issues")]),
            )
            .await
            .unwrap(),
        );
        assert_eq!(again.step, "url");
        assert!(again.error.is_some());
    }

    #[tokio::test]
    async fn back_keeps_what_was_typed_and_cancel_closes() {
        let st = Mutex::new(Store::open_in_memory().unwrap());
        let net = failing_net();
        let s = start(&st, "tracker.connect", &BTreeMap::new()).unwrap();
        step(
            submit(
                &st,
                &net,
                &s.flow_id,
                &vals(&[("url", "https://linear.app/acme")]),
            )
            .await
            .unwrap(),
        );
        let b = back(&st, &s.flow_id).unwrap();
        assert_eq!(b.step, "url");
        assert_eq!(b.fields[0].value, "https://linear.app/acme");
        cancel(&s.flow_id);
        assert!(back(&st, &s.flow_id).is_err());
        assert!(start(&st, "nope", &BTreeMap::new()).is_err());
    }
}
