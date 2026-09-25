//! GitHub Issues, read-only (work graph M6.1).
//!
//! * **Transport.** GraphQL at `https://api.github.com/graphql`, only
//!   through `gh` on a host (`via_cli:<alias>`,
//!   [`crate::net::via_host::GhCliTransport`]): the host's own `gh` login
//!   is used and fleet never reads, stores or sends a GitHub token.
//! * **Scope.** The site is `https://github.com/<owner>` (or all of
//!   `https://github.com`); `settings.repos` narrows it to named
//!   repositories. Views are `assignee:@me` searches inside that scope —
//!   never an org-wide crawl.
//! * **Identity** is the issue's node id, which survives a transfer; the
//!   key is `owner/repo#n` (lower case). A by-number fetch answered from
//!   another repository is a transfer, and the old key becomes an alias.
//! * **Status**: `OPEN` is todo, or in_progress when GitHub itself links a
//!   branch or an open pull request that closes it (never guessed);
//!   `CLOSED` is done with `stateReason` → completed / not_planned /
//!   duplicate.
//! * **Hierarchy**: sub-issues' `parent`.

use super::{
    check_http, map_transport, CallKind, Caps, Fetched, Incremental, ItemRef, Page, RefCtx,
    StatusSnapshot, TrackerError, TrackerInfo, TrackerProvider, ViewDef, WorkItemSnapshot,
    DESCRIPTION_MAX_CHARS, NOT_FOUND_OR_NO_PERMISSION,
};
use crate::net::https::{HttpTransport, Request};
use crate::store::{TrackerConfig, TrackerSettings};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub const GRAPHQL_URL: &str = "https://api.github.com/graphql";
/// Search page size.
pub const PAGE_SIZE: usize = 50;
/// Most references one by-id / by-number query asks for.
pub const FETCH_MAX: usize = 50;
/// The `recent` view's window when listed whole.
const RECENT_DAYS: i64 = 14;

/// The fields fleet reads of an issue, and nothing else.
const ISSUE_FIELDS: &str = "fragment I on Issue { id number title url state stateReason \
     updatedAt body repository { nameWithOwner } assignees(first: 10) { nodes { login } } \
     issueType { name } parent { id number repository { nameWithOwner } } \
     linkedBranches(first: 1) { totalCount } \
     closedByPullRequestsReferences(first: 1, includeClosedPrs: false) { totalCount } }";

pub struct GitHub {
    /// `Some(owner)` for `https://github.com/<owner>`.
    owner: Option<String>,
    config: TrackerConfig,
    settings: TrackerSettings,
    transport: Arc<dyn HttpTransport>,
}

/// `https://github.com[/<owner>]` → the owner, if any.
pub fn site_owner(site_url: &str) -> Option<String> {
    site_url
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")
        .filter(|o| !o.is_empty())
        .map(str::to_ascii_lowercase)
}

/// An owner or repository name: `[A-Za-z0-9_.-]`, not starting with a dot
/// or a dash (so it can never read as a search qualifier or a flag).
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with(['.', '-'])
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `owner/repo`, both halves [`valid_name`], lower-cased.
pub fn normalize_repo(s: &str) -> Option<String> {
    let (o, r) = s.trim().split_once('/')?;
    (valid_name(o) && valid_name(r)).then(|| format!("{o}/{r}").to_ascii_lowercase())
}

/// The key of issue `n` in `repo`.
pub fn issue_key(repo: &str, n: u64) -> String {
    format!("{}#{n}", repo.to_ascii_lowercase())
}

impl GitHub {
    pub fn new(
        site_url: &str,
        config: TrackerConfig,
        settings: TrackerSettings,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        GitHub {
            owner: site_owner(site_url),
            config,
            settings,
            transport,
        }
    }

    /// The repository `repo` is inside this tracker's scope.
    pub fn in_scope(&self, repo: &str) -> bool {
        let repo = repo.to_ascii_lowercase();
        if !self.settings.repos.is_empty() {
            return self
                .settings
                .repos
                .iter()
                .any(|r| r.eq_ignore_ascii_case(&repo));
        }
        match &self.owner {
            Some(o) => repo.split_once('/').is_some_and(|(ro, _)| ro == o),
            None => true,
        }
    }

    /// The search qualifiers that pin a query to the scope.
    fn scope_query(&self) -> String {
        if !self.settings.repos.is_empty() {
            return self
                .settings
                .repos
                .iter()
                .filter_map(|r| normalize_repo(r))
                .map(|r| format!("repo:{r}"))
                .collect::<Vec<_>>()
                .join(" ");
        }
        match &self.owner {
            Some(o) if valid_name(o) => format!("user:{o}"),
            _ => String::new(),
        }
    }

    async fn gql(
        &self,
        query: &str,
        variables: Value,
        call: CallKind,
    ) -> Result<Value, TrackerError> {
        let req = Request::post_json(
            GRAPHQL_URL,
            &json!({ "query": query, "variables": variables }),
        );
        let resp = self.transport.send(req).await.map_err(map_transport)?;
        check_http(&resp, call)?;
        let v: Value = resp.parse_json().map_err(TrackerError::Invalid)?;
        if !v.is_object() {
            return Err(TrackerError::Invalid("not a GraphQL answer".into()));
        }
        // Errors next to data are per field (a missing node is NOT_FOUND);
        // only the ones that say something about the whole call count.
        for e in v["errors"].as_array().into_iter().flatten() {
            match e["type"].as_str() {
                Some("RATE_LIMITED") => {
                    return Err(TrackerError::RateLimited {
                        retry_after_secs: None,
                    })
                }
                Some("FORBIDDEN") if call == CallKind::View => {
                    return Err(TrackerError::Forbidden(
                        "this view's search is forbidden".into(),
                    ))
                }
                Some("NOT_FOUND") => {}
                _ if v["data"].is_null() => {
                    let m = e["message"].as_str().unwrap_or("GraphQL error");
                    return Err(TrackerError::Invalid(
                        crate::logging::redact(m).chars().take(200).collect(),
                    ));
                }
                _ => {}
            }
        }
        if v["data"].is_null() {
            return Err(TrackerError::Invalid(
                "a GraphQL answer without data".into(),
            ));
        }
        Ok(v["data"].clone())
    }

    /// Normalise one issue node.
    pub fn snapshot(&self, n: &Value) -> Option<WorkItemSnapshot> {
        let id = n["id"].as_str()?.to_string();
        let number = n["number"].as_u64()?;
        let repo = n["repository"]["nameWithOwner"]
            .as_str()?
            .to_ascii_lowercase();
        let assignees: Vec<String> = n["assignees"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["login"].as_str().map(str::to_string))
            .collect();
        let me = self.config.account_id.as_deref();
        let assignee_id = assignees
            .iter()
            .find(|a| Some(a.as_str()) == me)
            .or(assignees.first())
            .cloned();
        let parent = &n["parent"];
        Some(WorkItemSnapshot {
            external_id: id,
            key: Some(issue_key(&repo, number)),
            aliases: Vec::new(),
            title: n["title"].as_str().unwrap_or_default().to_string(),
            url: n["url"].as_str().map(str::to_string),
            kind: Some(
                n["issueType"]["name"]
                    .as_str()
                    .unwrap_or("Issue")
                    .to_string(),
            ),
            hierarchy_level: None,
            status: status_of(n),
            parent_external_id: parent["id"].as_str().map(str::to_string),
            parent_key: match (
                parent["repository"]["nameWithOwner"].as_str(),
                parent["number"].as_u64(),
            ) {
                (Some(r), Some(k)) => Some(issue_key(r, k)),
                _ => None,
            },
            containers: vec![repo],
            assignees,
            assignee_id,
            iteration: None,
            iteration_active: false,
            updated: n["updatedAt"].as_str().and_then(super::parse_timestamp),
            description: n["body"]
                .as_str()
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(|b| b.chars().take(DESCRIPTION_MAX_CHARS).collect()),
        })
    }

    /// `(owner, name, number)` a reference names, when it is an issue of a
    /// repository in scope.
    fn repo_number(&self, r: &ItemRef) -> Option<(String, String, u64)> {
        let (repo, n) = match r {
            ItemRef::RepoNumber { repo, n } => (repo.clone(), *n),
            ItemRef::Key(k) | ItemRef::Url(k) => match self.recognize(k, RefCtx::default()).first()
            {
                Some(ItemRef::RepoNumber { repo, n }) => (repo.clone(), *n),
                _ => return None,
            },
            ItemRef::Id(_) => return None,
        };
        let repo = normalize_repo(&repo)?;
        let (o, name) = repo.split_once('/')?;
        Some((o.to_string(), name.to_string(), n))
    }
}

/// An issue node's status.
pub fn status_of(n: &Value) -> StatusSnapshot {
    let linked = n["linkedBranches"]["totalCount"].as_u64().unwrap_or(0) > 0
        || n["closedByPullRequestsReferences"]["totalCount"]
            .as_u64()
            .unwrap_or(0)
            > 0;
    match n["state"].as_str() {
        Some("CLOSED") => {
            let (name, resolution) = match n["stateReason"].as_str() {
                Some("NOT_PLANNED") => ("Closed as not planned", "not_planned"),
                Some("DUPLICATE") => ("Closed as duplicate", "duplicate"),
                _ => ("Closed", "completed"),
            };
            StatusSnapshot {
                name: name.into(),
                category: "done".into(),
                resolution: Some(resolution.into()),
            }
        }
        _ if linked => StatusSnapshot {
            name: "Open · linked pull request".into(),
            category: "in_progress".into(),
            resolution: None,
        },
        _ => StatusSnapshot {
            name: "Open".into(),
            category: "todo".into(),
            resolution: None,
        },
    }
}

#[async_trait::async_trait]
impl TrackerProvider for GitHub {
    fn caps(&self) -> Caps {
        Caps {
            query_lang: Some("gql".into()),
            hierarchy: true,
            iterations: false,
            human_keys: false,
            repo_relative: true,
            multi_container: false,
            incremental: Incremental::Watermark,
            write: false,
        }
    }

    async fn probe(&self) -> Result<TrackerInfo, TrackerError> {
        let data = self
            .gql("query { viewer { login } }", json!({}), CallKind::Identity)
            .await?;
        let login = data["viewer"]["login"]
            .as_str()
            .ok_or_else(|| TrackerError::Invalid("no viewer in the answer".into()))?;
        if let Some(o) = &self.owner {
            let d = self
                .gql(
                    "query($o: String!) { repositoryOwner(login: $o) { login } }",
                    json!({ "o": o }),
                    CallKind::Other,
                )
                .await?;
            if d["repositoryOwner"].is_null() {
                return Err(TrackerError::Invalid(format!(
                    "GitHub has no user or organisation {o:?} visible to {login}"
                )));
            }
        }
        Ok(TrackerInfo {
            instance_id: Some("github.com".into()),
            config: TrackerConfig {
                account_id: Some(login.to_string()),
                display_name: Some(login.to_string()),
                tz: Some("UTC".into()),
                ..Default::default()
            },
        })
    }

    async fn views(&self, _config: &TrackerConfig) -> Result<Vec<ViewDef>, TrackerError> {
        let scope = self.scope_query();
        let q = |rest: &str| {
            ["is:issue assignee:@me", rest, scope.as_str()]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        Ok(vec![
            ViewDef {
                id: "mine".into(),
                label: "My issues".into(),
                query: q("is:open"),
            },
            ViewDef {
                id: "recent".into(),
                label: "Recent".into(),
                query: q(""),
            },
        ])
    }

    async fn list(
        &self,
        view: &ViewDef,
        since: Option<i64>,
        cursor: Option<String>,
    ) -> Result<Page, TrackerError> {
        let since = since.or_else(|| {
            (view.id == "recent")
                .then(|| crate::service::catalog::now_secs() - RECENT_DAYS * 86_400)
        });
        let mut q = view.query.clone();
        if let Some(s) = since {
            q.push_str(&format!(" updated:>={}", super::format_timestamp(s)));
        }
        q.push_str(" sort:updated-desc");
        let query = format!(
            "query($q: String!, $after: String) {{ search(query: $q, type: ISSUE, first: {PAGE_SIZE}, \
             after: $after) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ...I }} }} }} {ISSUE_FIELDS}"
        );
        let data = self
            .gql(&query, json!({ "q": q, "after": cursor }), CallKind::View)
            .await?;
        let s = &data["search"];
        let next = s["pageInfo"]["endCursor"]
            .as_str()
            .filter(|_| s["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false))
            .map(str::to_string);
        Ok(Page {
            items: s["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|n| self.snapshot(n))
                .collect(),
            next,
        })
    }

    async fn fetch(&self, refs: &[ItemRef]) -> Result<Vec<Fetched>, TrackerError> {
        let mut out: Vec<Option<WorkItemSnapshot>> = vec![None; refs.len()];
        // By node id.
        let ids: Vec<(usize, &str)> = refs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                ItemRef::Id(id) => Some((i, id.as_str())),
                _ => None,
            })
            .collect();
        for chunk in ids.chunks(FETCH_MAX) {
            let query =
                format!("query($ids: [ID!]!) {{ nodes(ids: $ids) {{ ...I }} }} {ISSUE_FIELDS}");
            let data = self
                .gql(
                    &query,
                    json!({ "ids": chunk.iter().map(|(_, id)| *id).collect::<Vec<_>>() }),
                    CallKind::Other,
                )
                .await?;
            let nodes = data["nodes"].as_array().cloned().unwrap_or_default();
            for (k, (i, id)) in chunk.iter().enumerate() {
                out[*i] = nodes
                    .get(k)
                    .and_then(|n| self.snapshot(n))
                    .filter(|s| s.external_id == *id);
            }
        }
        // By repository and number (keys, URLs, repo numbers).
        let nums: Vec<(usize, (String, String, u64))> = refs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| self.repo_number(r).map(|t| (i, t)))
            .collect();
        for chunk in nums.chunks(FETCH_MAX) {
            let mut decls = Vec::new();
            let mut fields = Vec::new();
            let mut vars = Map::new();
            for (k, (_, (o, name, n))) in chunk.iter().enumerate() {
                decls.push(format!("$o{k}: String!, $r{k}: String!, $n{k}: Int!"));
                fields.push(format!(
                    "i{k}: repository(owner: $o{k}, name: $r{k}) {{ issue(number: $n{k}) {{ ...I }} }}"
                ));
                vars.insert(format!("o{k}"), json!(o));
                vars.insert(format!("r{k}"), json!(name));
                vars.insert(format!("n{k}"), json!(n));
            }
            let query = format!(
                "query({}) {{ {} }} {ISSUE_FIELDS}",
                decls.join(", "),
                fields.join(" ")
            );
            let data = self
                .gql(&query, Value::Object(vars), CallKind::Other)
                .await?;
            for (k, (i, (o, name, n))) in chunk.iter().enumerate() {
                out[*i] = self.snapshot(&data[format!("i{k}")]["issue"]).map(|mut s| {
                    // Answered from another repository or number: transferred.
                    let asked = issue_key(&format!("{o}/{name}"), *n);
                    if s.key.as_deref() != Some(asked.as_str()) && !s.aliases.contains(&asked) {
                        s.aliases.push(asked);
                    }
                    s
                });
            }
        }
        Ok(refs
            .iter()
            .zip(out)
            .map(|(r, s)| match s {
                Some(s) => Fetched::Found(Box::new(s)),
                None => Fetched::Unavailable {
                    reference: r.reference(),
                    reason: NOT_FOUND_OR_NO_PERMISSION.into(),
                },
            })
            .collect())
    }

    fn recognize(&self, text: &str, ctx: RefCtx<'_>) -> Vec<ItemRef> {
        use crate::service::work::recognize::{recognize, RecognizeCtx};
        let rctx = RecognizeCtx {
            repo: ctx.repo.and_then(normalize_repo),
            ..Default::default()
        };
        let mut out = Vec::new();
        for m in recognize(text, &rctx) {
            if m.provider.as_deref() != Some("github") {
                continue;
            }
            let Some((repo, n)) = m.key.rsplit_once('#') else {
                continue;
            };
            let (Some(repo), Ok(n)) = (normalize_repo(repo), n.parse::<u64>()) else {
                continue;
            };
            if !self.in_scope(&repo) {
                continue;
            }
            let r = ItemRef::RepoNumber { repo, n };
            if !out.contains(&r) {
                out.push(r);
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "tests_github.rs"]
mod tests;
