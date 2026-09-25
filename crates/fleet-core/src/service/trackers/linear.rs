//! Linear, read-only (work graph M6.4).
//!
//! * **Transport.** `Direct` GraphQL at `https://api.linear.app/graphql`
//!   only (the host fence), or `via_host`. A personal API key goes in
//!   `Authorization` as it is (Linear's convention: no `Bearer`).
//! * **Site** is `https://linear.app/<workspace urlKey>`; the probe checks
//!   the key belongs to that workspace.
//! * **Keys.** The probe's team keys are the key prefixes, which settles
//!   `ENG-123` between Jira and Linear through recognition and
//!   `tracker_claims`, never by guessing. Identity is the issue id; the key
//!   is `identifier`, and a team move keeps the id while Linear's
//!   `previousIdentifiers` (and a by-key answer under another identifier)
//!   become aliases.
//! * **Views** are root `issues` filtered to the API user (`isMe`), all
//!   filters passed as GraphQL variables: `mine` (not completed or
//!   canceled), `sprint` (the active cycle; only when a team has cycles) and
//!   `recent` (updated within 14 days). Paging is by `after` cursor.
//! * **Status** is `state.type`: triage / backlog / unstarted → todo,
//!   started → in_progress, completed → done / completed, canceled → done /
//!   not_planned.
//! * **Rate limits**: 429 or a `RATELIMITED` error (requests or complexity)
//!   back off, until `X-RateLimit-Requests-Reset` when Linear says.

use super::{
    check_http, map_transport, retry_after, CallKind, Caps, Fetched, Incremental, ItemRef, Page,
    RefCtx, StatusSnapshot, TrackerError, TrackerInfo, TrackerProvider, ViewDef, WorkItemSnapshot,
    DESCRIPTION_MAX_CHARS, NOT_FOUND_OR_NO_PERMISSION,
};
use crate::net::https::{HttpTransport, Request};
use crate::store::{TrackerConfig, TrackerCredential};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub const GRAPHQL_URL: &str = "https://api.linear.app/graphql";
pub const API_HOST: &str = "api.linear.app";
/// Page size: narrow queries keep the complexity cost low.
pub const PAGE_SIZE: usize = 50;
/// Most issues one by-reference query asks for.
pub const FETCH_MAX: usize = 25;
const RECENT_DAYS: i64 = 14;

/// The fields fleet reads of an issue, and nothing else.
const ISSUE_FIELDS: &str = "fragment I on Issue { id identifier title url updatedAt description \
     previousIdentifiers state { name type } team { key } assignee { id name } \
     parent { id identifier } cycle { number name isActive } }";

pub struct Linear {
    /// The workspace `urlKey` the site names.
    url_key: Option<String>,
    config: TrackerConfig,
    cred: Option<TrackerCredential>,
    transport: Arc<dyn HttpTransport>,
}

/// `https://linear.app/<urlKey>` → the urlKey.
pub fn site_url_key(site_url: &str) -> Option<String> {
    site_url
        .trim_end_matches('/')
        .strip_prefix("https://linear.app/")
        .filter(|k| !k.is_empty() && !k.contains('/'))
        .map(str::to_ascii_lowercase)
}

/// `state.type` → fleet's status.
pub fn status_of(state: &Value) -> StatusSnapshot {
    let name = state["name"].as_str().unwrap_or_default().to_string();
    let (category, resolution) = match state["type"].as_str() {
        Some("started") => ("in_progress", None),
        Some("completed") => ("done", Some("completed")),
        Some("canceled") => ("done", Some("not_planned")),
        // triage, backlog, unstarted, and anything newer: never claim work
        // is under way or done on a guess.
        _ => ("todo", None),
    };
    StatusSnapshot {
        name,
        category: category.into(),
        resolution: resolution.map(str::to_string),
    }
}

impl Linear {
    pub fn new(
        site_url: &str,
        config: TrackerConfig,
        cred: Option<TrackerCredential>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Linear {
            url_key: site_url_key(site_url),
            config,
            cred,
            transport,
        }
    }

    async fn gql(
        &self,
        query: &str,
        variables: Value,
        call: CallKind,
    ) -> Result<Value, TrackerError> {
        let cred = self.cred.as_ref().ok_or(TrackerError::Unconfigured)?;
        let req = Request::post_json(
            GRAPHQL_URL,
            &json!({ "query": query, "variables": variables }),
        )
        .header("Authorization", cred.secret.expose());
        let resp = self.transport.send(req).await.map_err(map_transport)?;
        let wait = retry_after(&resp);
        // Linear answers most failures as a GraphQL error (often on a 400):
        // read the body first, then the status.
        let v: Option<Value> = resp.parse_json().ok().filter(Value::is_object);
        if let Some(v) = &v {
            for e in v["errors"].as_array().into_iter().flatten() {
                let code = e["extensions"]["code"].as_str().unwrap_or_default();
                let msg = e["message"].as_str().unwrap_or_default().to_lowercase();
                if code == "RATELIMITED" || msg.contains("complexity") || msg.contains("rate limit")
                {
                    return Err(TrackerError::RateLimited {
                        retry_after_secs: wait,
                    });
                }
                if code == "AUTHENTICATION_ERROR" {
                    return Err(TrackerError::Auth("the API key was refused".into()));
                }
                if code == "FORBIDDEN" && call == CallKind::View {
                    return Err(TrackerError::Forbidden("this view is forbidden".into()));
                }
            }
        }
        check_http(&resp, call)?;
        let v = v.ok_or_else(|| TrackerError::Invalid("not a GraphQL answer".into()))?;
        if v["data"].is_null() {
            let m = v["errors"][0]["message"]
                .as_str()
                .unwrap_or("a GraphQL answer without data");
            return Err(TrackerError::Invalid(
                crate::logging::redact(m).chars().take(200).collect(),
            ));
        }
        Ok(v["data"].clone())
    }

    /// Normalise one issue node.
    pub fn snapshot(&self, n: &Value) -> Option<WorkItemSnapshot> {
        let id = n["id"].as_str()?.to_string();
        let key = n["identifier"].as_str()?.to_ascii_uppercase();
        let aliases: Vec<String> = n["previousIdentifiers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str())
            .map(str::to_ascii_uppercase)
            .filter(|p| *p != key)
            .collect();
        let parent = &n["parent"];
        let cycle = &n["cycle"];
        Some(WorkItemSnapshot {
            external_id: id,
            key: Some(key),
            aliases,
            title: n["title"].as_str().unwrap_or_default().to_string(),
            url: n["url"]
                .as_str()
                .filter(|u| u.starts_with("https://linear.app/"))
                .map(str::to_string),
            kind: Some(
                if parent.is_object() {
                    "Sub-issue"
                } else {
                    "Issue"
                }
                .into(),
            ),
            hierarchy_level: Some(if parent.is_object() { -1 } else { 0 }),
            status: status_of(&n["state"]),
            parent_external_id: parent["id"].as_str().map(str::to_string),
            parent_key: parent["identifier"].as_str().map(str::to_ascii_uppercase),
            containers: n["team"]["key"]
                .as_str()
                .map(|k| vec![k.to_ascii_uppercase()])
                .unwrap_or_default(),
            assignees: n["assignee"]["name"]
                .as_str()
                .map(|a| vec![a.to_string()])
                .unwrap_or_default(),
            assignee_id: n["assignee"]["id"].as_str().map(str::to_string),
            iteration: cycle.is_object().then(|| {
                cycle["name"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("Cycle {}", cycle["number"].as_u64().unwrap_or(0)))
            }),
            iteration_active: cycle["isActive"].as_bool().unwrap_or(false),
            updated: n["updatedAt"].as_str().and_then(super::parse_timestamp),
            description: n["description"]
                .as_str()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(|d| d.chars().take(DESCRIPTION_MAX_CHARS).collect()),
        })
    }

    /// The filter a view reads, as a GraphQL variable.
    fn filter(view: &ViewDef, since: Option<i64>) -> Result<Value, TrackerError> {
        let mut f = json!({ "assignee": { "isMe": { "eq": true } } });
        match view.query.as_str() {
            "mine" => {
                f["state"] = json!({ "type": { "nin": ["completed", "canceled"] } });
            }
            "sprint" => {
                f["cycle"] = json!({ "isActive": { "eq": true } });
            }
            "recent" => {}
            q => return Err(TrackerError::Invalid(format!("unknown Linear view {q:?}"))),
        }
        let since = since.or_else(|| {
            (view.query == "recent")
                .then(|| crate::service::catalog::now_secs() - RECENT_DAYS * 86_400)
        });
        if let Some(s) = since {
            f["updatedAt"] = json!({ "gte": super::format_timestamp(s) });
        }
        Ok(f)
    }

    /// A reference → what `issue(id:)` takes: the id, or an identifier.
    fn lookup_id(&self, r: &ItemRef) -> Option<String> {
        match r {
            ItemRef::Id(id) => Some(id.clone()),
            ItemRef::Key(k) if !k.contains("://") => Some(k.to_ascii_uppercase()),
            ItemRef::Key(u) | ItemRef::Url(u) => match self.recognize(u, RefCtx::default()).first()
            {
                Some(ItemRef::Key(k)) => Some(k.clone()),
                _ => None,
            },
            ItemRef::RepoNumber { .. } => None,
        }
    }
}

#[async_trait::async_trait]
impl TrackerProvider for Linear {
    fn caps(&self) -> Caps {
        Caps {
            query_lang: Some("gql".into()),
            hierarchy: true,
            iterations: true,
            human_keys: true,
            repo_relative: false,
            multi_container: false,
            incremental: Incremental::Watermark,
            write: false,
        }
    }

    async fn probe(&self) -> Result<TrackerInfo, TrackerError> {
        let d = self
            .gql(
                "query { viewer { id name } organization { id urlKey } \
                 teams(first: 100) { nodes { key cyclesEnabled activeCycle { id } } } }",
                json!({}),
                CallKind::Identity,
            )
            .await?;
        let me = d["viewer"]["id"]
            .as_str()
            .ok_or_else(|| TrackerError::Invalid("no viewer in the answer".into()))?;
        let org = &d["organization"];
        let url_key = org["urlKey"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if let Some(k) = &self.url_key {
            if *k != url_key {
                return Err(TrackerError::Invalid(format!(
                    "this API key belongs to the workspace {url_key:?}, not {k:?}"
                )));
            }
        }
        let mut key_prefixes = Vec::new();
        let mut with_cycles = Vec::new();
        for t in d["teams"]["nodes"].as_array().into_iter().flatten() {
            if let Some(k) = t["key"].as_str() {
                let k = k.to_ascii_uppercase();
                if t["cyclesEnabled"].as_bool().unwrap_or(false) && t["activeCycle"].is_object() {
                    with_cycles.push(k.clone());
                }
                key_prefixes.push(k);
            }
        }
        key_prefixes.sort();
        key_prefixes.dedup();
        with_cycles.sort();
        Ok(TrackerInfo {
            instance_id: org["id"].as_str().map(str::to_string),
            config: TrackerConfig {
                account_id: Some(me.to_string()),
                display_name: d["viewer"]["name"].as_str().map(str::to_string),
                tz: Some("UTC".into()),
                key_prefixes,
                // Teams with an active cycle: Linear's sprints.
                sprint_projects: with_cycles,
                workspace: org["id"].as_str().map(str::to_string),
                ..Default::default()
            },
        })
    }

    async fn views(&self, config: &TrackerConfig) -> Result<Vec<ViewDef>, TrackerError> {
        let mut v = vec![ViewDef {
            id: "mine".into(),
            label: "My issues".into(),
            query: "mine".into(),
        }];
        if !config.sprint_projects.is_empty() {
            v.push(ViewDef {
                id: "sprint".into(),
                label: "Current cycle".into(),
                query: "sprint".into(),
            });
        }
        v.push(ViewDef {
            id: "recent".into(),
            label: "Recent".into(),
            query: "recent".into(),
        });
        Ok(v)
    }

    async fn list(
        &self,
        view: &ViewDef,
        since: Option<i64>,
        cursor: Option<String>,
    ) -> Result<Page, TrackerError> {
        let filter = Self::filter(view, since)?;
        let query = format!(
            "query($filter: IssueFilter, $after: String) {{ issues(filter: $filter, first: {PAGE_SIZE}, \
             after: $after, orderBy: updatedAt) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...I }} }} }} \
             {ISSUE_FIELDS}"
        );
        let d = self
            .gql(
                &query,
                json!({ "filter": filter, "after": cursor }),
                CallKind::View,
            )
            .await?;
        let issues = &d["issues"];
        let next = issues["pageInfo"]["endCursor"]
            .as_str()
            .filter(|_| issues["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false))
            .map(str::to_string);
        Ok(Page {
            items: issues["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|n| self.snapshot(n))
                .collect(),
            next,
        })
    }

    async fn fetch(&self, refs: &[ItemRef]) -> Result<Vec<Fetched>, TrackerError> {
        let asked: Vec<Option<String>> = refs.iter().map(|r| self.lookup_id(r)).collect();
        let mut found: Vec<Option<WorkItemSnapshot>> = vec![None; refs.len()];
        let askable: Vec<(usize, String)> = asked
            .iter()
            .enumerate()
            .filter_map(|(i, a)| a.clone().map(|a| (i, a)))
            .collect();
        for chunk in askable.chunks(FETCH_MAX) {
            let mut decls = Vec::new();
            let mut fields = Vec::new();
            let mut vars = Map::new();
            for (k, (_, id)) in chunk.iter().enumerate() {
                decls.push(format!("$i{k}: String!"));
                fields.push(format!("i{k}: issue(id: $i{k}) {{ ...I }}"));
                vars.insert(format!("i{k}"), json!(id));
            }
            let query = format!(
                "query({}) {{ {} }} {ISSUE_FIELDS}",
                decls.join(", "),
                fields.join(" ")
            );
            // A missing issue is a per-field error next to the others' data.
            let d = self
                .gql(&query, Value::Object(vars), CallKind::Other)
                .await?;
            for (k, (i, id)) in chunk.iter().enumerate() {
                found[*i] = self.snapshot(&d[format!("i{k}")]).map(|mut s| {
                    // Asked under an identifier it no longer has: moved.
                    let asked = id.to_ascii_uppercase();
                    if matches!(refs[*i], ItemRef::Key(_) | ItemRef::Url(_))
                        && s.key.as_deref() != Some(asked.as_str())
                        && !s.aliases.contains(&asked)
                    {
                        s.aliases.push(asked);
                    }
                    s
                });
            }
        }
        Ok(refs
            .iter()
            .zip(found)
            .map(|(r, s)| match s {
                Some(s) => Fetched::Found(Box::new(s)),
                None => Fetched::Unavailable {
                    reference: r.reference(),
                    reason: NOT_FOUND_OR_NO_PERMISSION.into(),
                },
            })
            .collect())
    }

    fn recognize(&self, text: &str, _ctx: RefCtx<'_>) -> Vec<ItemRef> {
        use crate::service::work::recognize::{recognize, MatchKind, RecognizeCtx};
        let prefixes = &self.config.key_prefixes;
        if prefixes.is_empty() {
            return Vec::new();
        }
        let ctx = RecognizeCtx {
            prefixes: prefixes.clone(),
            ..Default::default()
        };
        let known = |k: &str| {
            k.split_once('-')
                .is_some_and(|(p, _)| prefixes.iter().any(|x| x.eq_ignore_ascii_case(p)))
        };
        let mut out = Vec::new();
        for m in recognize(text, &ctx) {
            let ok = match m.kind {
                MatchKind::Url => m.provider.as_deref() == Some("linear") && known(&m.key),
                MatchKind::Key => known(&m.key),
                MatchKind::RepoIssue => false,
            };
            let r = ItemRef::Key(m.key);
            if ok && !out.contains(&r) {
                out.push(r);
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "tests_linear.rs"]
mod tests;
