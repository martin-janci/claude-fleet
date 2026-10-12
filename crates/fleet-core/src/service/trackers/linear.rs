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
    check_http, description_and_len, map_transport, retry_after, CallKind, Caps, Fetched,
    Incremental, ItemRef, Page, RefCtx, StatusSnapshot, TrackerComment, TrackerError, TrackerInfo,
    TrackerProvider, ViewDef, WorkItemSnapshot, WriteOp, WriteOutcome, NOT_FOUND_OR_NO_PERMISSION,
};
use crate::net::https::{HttpTransport, Request};
use crate::store::{Estimate, TrackerConfig, TrackerCredential};
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
     parent { id identifier } cycle { number name isActive } projectMilestone { name } \
     priority estimate labels(first: 20) { nodes { name } } }";

/// Where Linear shows things: a comment URL from an answer is kept only
/// under it.
pub const WEB_PREFIX: &str = "https://linear.app/";

/// PURE: an issue id or identifier fleet may send as a GraphQL variable (a
/// uuid, or `ENG-123`).
fn issue_ref_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// PURE: a Linear team key (`ENG`, `A1`): a letter, then letters and
/// digits, at most ten.
pub fn team_key_ok(k: &str) -> bool {
    k.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && k.len() <= 10
        && k.bytes().all(|b| b.is_ascii_alphanumeric())
}

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

/// Linear's `priority` (0 none, 1 urgent, 2 high, 3 medium, 4 low) →
/// fleet's priority.
pub fn linear_priority(v: &Value) -> Option<String> {
    let p = match v.as_i64()? {
        1 => "urgent",
        2 => "high",
        3 => "medium",
        4 => "low",
        _ => return None,
    };
    Some(p.to_string())
}

/// `ENG-123`: a team key (letters and digits), a dash, a number — the shape
/// of Linear's `identifier` and `previousIdentifiers`. Anything else the
/// tracker sends as a key is not kept as one.
pub fn is_identifier(s: &str) -> bool {
    let Some((team, n)) = s.split_once('-') else {
        return false;
    };
    (1..=10).contains(&team.len())
        && team.chars().all(|c| c.is_ascii_alphanumeric())
        && (1..=9).contains(&n.len())
        && n.chars().all(|c| c.is_ascii_digit())
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

    /// One GraphQL exchange: the whole answer (`data` and `errors`), once
    /// the errors that speak for the whole call (rate limit, auth, a
    /// forbidden view) and the HTTP status are mapped.
    async fn gql_envelope(
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
        v.ok_or_else(|| TrackerError::Invalid("not a GraphQL answer".into()))
    }

    /// The `data` of one GraphQL exchange; an answer without it fails.
    async fn gql(
        &self,
        query: &str,
        variables: Value,
        call: CallKind,
    ) -> Result<Value, TrackerError> {
        let v = self.gql_envelope(query, variables, call).await?;
        if v["data"].is_null() {
            return Err(Self::no_data(&v));
        }
        Ok(v["data"].clone())
    }

    /// The error for an answer without `data`: its first message.
    fn no_data(v: &Value) -> TrackerError {
        let m = v["errors"][0]["message"]
            .as_str()
            .unwrap_or("a GraphQL answer without data");
        TrackerError::Invalid(crate::logging::redact(m).chars().take(200).collect())
    }

    /// Up to [`FETCH_MAX`] issues by id or identifier, one aliased
    /// `issue(id:)` lookup each; `None` for one Linear does not have (or the
    /// key may not read).
    ///
    /// `Query.issue` is non-null, so one missing issue does not come back
    /// as a `null` next to the others: it nulls the root `data`, with a
    /// field error whose `path` names the alias. Those aliases are
    /// unavailable, and the rest are asked for again without them; each
    /// round drops at least one, so the rounds are bounded by the chunk.
    async fn lookup_chunk(
        &self,
        ids: &[String],
    ) -> Result<Vec<Option<WorkItemSnapshot>>, TrackerError> {
        let mut found: Vec<Option<WorkItemSnapshot>> = vec![None; ids.len()];
        let mut pending: Vec<usize> = (0..ids.len()).collect();
        for _ in 0..ids.len() {
            if pending.is_empty() {
                break;
            }
            let mut decls = Vec::new();
            let mut fields = Vec::new();
            let mut vars = Map::new();
            for (k, i) in pending.iter().enumerate() {
                decls.push(format!("$i{k}: String!"));
                fields.push(format!("i{k}: issue(id: $i{k}) {{ ...I }}"));
                vars.insert(format!("i{k}"), json!(ids[*i]));
            }
            let query = format!(
                "query({}) {{ {} }} {ISSUE_FIELDS}",
                decls.join(", "),
                fields.join(" ")
            );
            let v = self
                .gql_envelope(&query, Value::Object(vars), CallKind::Other)
                .await?;
            if let Some(d) = v["data"].as_object() {
                for (k, i) in pending.iter().enumerate() {
                    // An answer missing an alias is that issue not found,
                    // never a panic on a map index.
                    found[*i] = d.get(&format!("i{k}")).and_then(|n| self.snapshot(n));
                }
                break;
            }
            // No data: which aliases do the field errors name?
            let failed: Vec<usize> = v["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["path"][0].as_str())
                .filter_map(|alias| alias.strip_prefix('i')?.parse::<usize>().ok())
                .filter(|k| *k < pending.len())
                .collect();
            if failed.is_empty() {
                // Not a per-field failure: the whole call went wrong.
                return Err(Self::no_data(&v));
            }
            pending = pending
                .iter()
                .enumerate()
                .filter(|(k, _)| !failed.contains(k))
                .map(|(_, i)| *i)
                .collect();
        }
        Ok(found)
    }

    /// Normalise one issue node.
    pub fn snapshot(&self, n: &Value) -> Option<WorkItemSnapshot> {
        let id = n["id"].as_str()?.to_string();
        // Keys only in a key's shape: they are shown, and read by fleet,
        // as its own text.
        let key = n["identifier"]
            .as_str()
            .map(str::to_ascii_uppercase)
            .filter(|k| is_identifier(k));
        let aliases: Vec<String> = n["previousIdentifiers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_str())
            .map(str::to_ascii_uppercase)
            .filter(|p| is_identifier(p) && key.as_deref() != Some(p.as_str()))
            .collect();
        let parent = &n["parent"];
        let cycle = &n["cycle"];
        let (description, description_chars) =
            description_and_len(n["description"].as_str().map(str::trim).unwrap_or_default());
        Some(WorkItemSnapshot {
            external_id: id,
            key,
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
            parent_key: parent["identifier"]
                .as_str()
                .map(str::to_ascii_uppercase)
                .filter(|k| is_identifier(k)),
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
            // E8: a release is a project milestone (it carries a target
            // date; a Linear project is closer to an epic).
            // No due date read from this tracker yet.
            due_at: None,
            priority: linear_priority(&n["priority"]),
            labels: n["labels"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l["name"].as_str())
                .map(str::to_string)
                .collect(),
            // A team's estimates are points, whatever its scale.
            estimate: n["estimate"].as_f64().and_then(Estimate::new),
            estimate_unit: n["estimate"]
                .as_f64()
                .and_then(Estimate::new)
                .map(|_| "points".to_string()),
            versions: n["projectMilestone"]["name"]
                .as_str()
                .map(|t| vec![t.to_string()])
                .unwrap_or_default(),
            updated: n["updatedAt"].as_str().and_then(super::parse_timestamp),
            description,
            description_chars,
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
    /// Create an issue from a fleet task ([`WriteOp::CreateItem`]): a
    /// sub-issue of `parent` in its team, or an issue of team `container`
    /// (its key).
    async fn create_issue(
        &self,
        container: Option<&str>,
        parent: Option<&super::CreateParent>,
        title: &str,
        description: &str,
    ) -> Result<WriteOutcome, TrackerError> {
        let (team, parent_id) = match parent {
            Some(p) => {
                let id = p
                    .external_id
                    .clone()
                    .or_else(|| self.lookup_id(&ItemRef::parse(&p.key)))
                    .filter(|id| issue_ref_ok(id))
                    .ok_or_else(|| TrackerError::Invalid("not a Linear issue".into()))?;
                let d = self
                    .gql(
                        "query($id: String!) { issue(id: $id) { id team { id } } }",
                        json!({ "id": id }),
                        CallKind::Other,
                    )
                    .await?;
                let team = d["issue"]["team"]["id"].as_str().map(str::to_string);
                let parent_id = d["issue"]["id"].as_str().map(str::to_string);
                (team.ok_or(TrackerError::NotFound)?, parent_id)
            }
            None => {
                let key = container
                    .filter(|k| team_key_ok(k))
                    .map(str::to_ascii_uppercase)
                    .ok_or_else(|| {
                        TrackerError::Invalid("no Linear team to create it in".into())
                    })?;
                let d = self
                    .gql(
                        "query($k: String!) { teams(filter: { key: { eq: $k } }) { nodes { id } } }",
                        json!({ "k": key }),
                        CallKind::Other,
                    )
                    .await?;
                let team = d["teams"]["nodes"][0]["id"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or(TrackerError::NotFound)?;
                (team, None)
            }
        };
        let title: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut input = json!({ "teamId": team, "title": title });
        if !description.trim().is_empty() {
            input["description"] = Value::String(description.to_string());
        }
        if let Some(p) = parent_id.filter(|id| issue_ref_ok(id)) {
            input["parentId"] = Value::String(p);
        }
        let data = self
            .gql(
                "mutation($input: IssueCreateInput!) { issueCreate(input: $input) { success \
                 issue { id identifier url } } }",
                json!({ "input": input }),
                CallKind::Other,
            )
            .await?;
        let created = &data["issueCreate"];
        let issue = &created["issue"];
        let id = issue["id"].as_str().filter(|id| issue_ref_ok(id));
        let key = issue["identifier"]
            .as_str()
            .filter(|k| {
                k.split_once('-').is_some_and(|(t, n)| {
                    team_key_ok(t)
                        && !n.is_empty()
                        && n.len() <= 9
                        && n.bytes().all(|b| b.is_ascii_digit())
                })
            })
            .map(str::to_ascii_uppercase);
        match (created["success"].as_bool(), id, key) {
            (Some(true), Some(id), Some(key)) => Ok(WriteOutcome {
                remote_id: Some(id.to_string()),
                url: issue["url"]
                    .as_str()
                    .filter(|u| u.starts_with(WEB_PREFIX) && u.len() <= 2048)
                    .map(str::to_string),
                key: Some(key),
            }),
            _ => Err(TrackerError::Invalid(
                "Linear created the issue but did not say which one".into(),
            )),
        }
    }

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
            versions: true,
            human_keys: true,
            repo_relative: false,
            multi_container: false,
            incremental: Incremental::Watermark,
            write: true,
            // Linear keeps the default here: it degrades to "open the
            // ticket", which is what the capability is for.
            describe: false,
            comments: true,
            attachments: false,
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
            let ids: Vec<String> = chunk.iter().map(|(_, id)| id.clone()).collect();
            let got = self.lookup_chunk(&ids).await?;
            for ((i, id), s) in chunk.iter().zip(got) {
                found[*i] = s.map(|mut s| {
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

    async fn comments(
        &self,
        r: &ItemRef,
        limit: usize,
    ) -> Result<Vec<TrackerComment>, TrackerError> {
        let limit = limit.clamp(1, super::TRACKER_COMMENTS_MAX);
        let Some(id) = self.lookup_id(r).filter(|id| issue_ref_ok(id)) else {
            return Err(TrackerError::Invalid("not a Linear issue".into()));
        };
        let data = self
            .gql(
                &format!(
                    "query($id: String!) {{ issue(id: $id) {{ comments(last: {limit}) \
                     {{ nodes {{ id body url createdAt user {{ name }} }} }} }} }}"
                ),
                json!({ "id": id }),
                CallKind::Other,
            )
            .await?;
        if data["issue"].is_null() {
            return Err(TrackerError::NotFound);
        }
        let out = data["issue"]["comments"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                TrackerComment::from_parts(
                    c["id"].as_str()?,
                    c["user"]["name"].as_str().unwrap_or("Linear"),
                    c["body"].as_str().unwrap_or_default(),
                    c["createdAt"].as_str()?,
                    c["url"].as_str(),
                    WEB_PREFIX,
                )
            })
            .collect();
        Ok(super::newest_comments(out, limit))
    }

    async fn write(&self, op: &WriteOp) -> Result<WriteOutcome, TrackerError> {
        if let WriteOp::CreateItem {
            container,
            parent,
            title,
            description,
        } = op
        {
            return self
                .create_issue(container.as_deref(), parent.as_ref(), title, description)
                .await;
        }
        let WriteOp::Comment {
            key,
            external_id,
            body,
        } = op
        else {
            return Err(TrackerError::Refused(
                "Linear takes comments and new issues only from fleet".into(),
            ));
        };
        let id = external_id
            .clone()
            .or_else(|| self.lookup_id(&ItemRef::parse(key)))
            .filter(|id| issue_ref_ok(id))
            .ok_or_else(|| TrackerError::Invalid("not a Linear issue".into()))?;
        let data = self
            .gql(
                "mutation($id: String!, $body: String!) { commentCreate(input: { issueId: $id, \
                 body: $body }) { success comment { id url } } }",
                json!({ "id": id, "body": body }),
                CallKind::Other,
            )
            .await?;
        let created = &data["commentCreate"];
        if created["success"].as_bool() != Some(true) {
            return Err(TrackerError::Invalid(
                "Linear did not add the comment".into(),
            ));
        }
        Ok(WriteOutcome {
            remote_id: created["comment"]["id"]
                .as_str()
                .filter(|id| issue_ref_ok(id))
                .map(str::to_string),
            url: created["comment"]["url"]
                .as_str()
                .filter(|u| u.starts_with(WEB_PREFIX) && u.len() <= 2048)
                .map(str::to_string),
            key: None,
        })
    }
}

#[cfg(test)]
#[path = "tests_linear.rs"]
mod tests;
