//! Jira Cloud, read-only (work graph M3.2), per the review's C23–C29:
//!
//! * **Search** is `POST /rest/api/3/search/jql` (the old `/search` is gone):
//!   always with an explicit `fields` list, paged by `nextPageToken`, which
//!   is never persisted and is guarded against repeating
//!   ([`super::list_all`]).
//! * **Identity** is the numeric id; keys are attributes. By-id (or by-key)
//!   reads are `POST /rest/api/3/issue/bulkfetch`, at most
//!   [`BULK_MAX`] per call; an id missing from the answer is *unavailable*,
//!   never *gone* (C25). A by-key answer under a different key reveals a
//!   moved issue, whose old key becomes an alias (C24).
//! * **Status** is `statusCategory.key` (`new`/`indeterminate`/`done`, and
//!   `undefined` → todo, C26) plus `resolution`, told apart conservatively by
//!   name.
//! * **Hierarchy** comes from `issuetype.hierarchyLevel` and the unified
//!   `parent` field, never from type names (C28).
//! * **Sprints exist per project** (C28): the probe records which projects
//!   have an open sprint, and the `sprint` view exists only when one does.
//! * **Favourite filters** are wrapped as `filter = <id>`, never by
//!   concatenating their JQL, which may end in `ORDER BY` (C28).
//! * **Incremental** listings use a *relative* window (`updated >= -Nm`),
//!   which Jira evaluates in its own clock and the API user's timezone, so
//!   no timezone conversion happens here (C27). The caller adds the overlap
//!   and dedupes on `(id, updated)`.
//! * **Descriptions** are ADF; only a plain-text excerpt is kept.

use super::{
    Caps, Fetched, ItemRef, Page, StatusSnapshot, TrackerError, TrackerInfo, TrackerProvider,
    ViewDef, WorkItemSnapshot, DESCRIPTION_MAX_CHARS, NOT_FOUND_OR_NO_PERMISSION,
};
use crate::net::https::{HttpTransport, Request, Response, TransportError};
use crate::store::{TrackerConfig, TrackerCredential};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// `bulkfetch`'s limit.
pub const BULK_MAX: usize = 100;
/// Page size for `search/jql`.
pub const PAGE_SIZE: usize = 100;
/// The sprint field's `schema.custom`.
pub const SPRINT_FIELD_SCHEMA: &str = "com.pyxis.greenhopper.jira:gh-sprint";

/// Built-in views (plan M3.2). The `sprint` one only where sprints exist.
pub const VIEW_MINE: &str = "assignee = currentUser() AND statusCategory != Done";
pub const VIEW_SPRINT: &str = "sprint in openSprints() AND assignee = currentUser()";
pub const VIEW_RECENT: &str = "assignee = currentUser() AND updated >= -14d";

/// Which call a response answers; decides what a 403 means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Call {
    /// `/myself`: a 403 here is an auth failure.
    Identity,
    /// A view's search: a 403 disables that view only.
    View,
    Other,
}

pub struct JiraCloud {
    site: String,
    config: TrackerConfig,
    cred: Option<TrackerCredential>,
    transport: Arc<dyn HttpTransport>,
}

impl JiraCloud {
    pub fn new(
        site_url: &str,
        config: TrackerConfig,
        cred: Option<TrackerCredential>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        JiraCloud {
            site: site_url.trim_end_matches('/').to_string(),
            config,
            cred,
            transport,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.site)
    }

    /// The fields every search and fetch asks for (C23: `search/jql`
    /// returns only ids without them).
    fn fields(&self) -> Vec<String> {
        let mut f: Vec<String> = [
            "summary",
            "status",
            "resolution",
            "issuetype",
            "parent",
            "assignee",
            "updated",
            "project",
            "description",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(sprint) = &self.config.sprint_field {
            f.push(sprint.clone());
        }
        f
    }

    async fn call(&self, req: Request, call: Call) -> Result<Value, TrackerError> {
        let cred = self.cred.as_ref().ok_or(TrackerError::Unconfigured)?;
        let req = req.header("Authorization", cred.authorization().expose());
        let resp = self.transport.send(req).await.map_err(map_transport)?;
        check(&resp, call)?;
        resp.parse_json::<Value>().map_err(TrackerError::Invalid)
    }

    async fn search_page(
        &self,
        jql: &str,
        fields: Vec<String>,
        cursor: Option<String>,
        call: Call,
    ) -> Result<(Vec<Value>, Option<String>), TrackerError> {
        let mut body = json!({
            "jql": jql,
            "fields": fields,
            "maxResults": PAGE_SIZE,
        });
        if let Some(c) = cursor {
            body["nextPageToken"] = Value::String(c);
        }
        let v = self
            .call(
                Request::post_json(self.url("/rest/api/3/search/jql"), &body),
                call,
            )
            .await?;
        let issues = v["issues"].as_array().cloned().unwrap_or_default();
        let last = v["isLast"].as_bool().unwrap_or(true);
        let next = v["nextPageToken"]
            .as_str()
            .filter(|t| !t.is_empty() && !last)
            .map(str::to_string);
        Ok((issues, next))
    }

    /// Key prefixes, across every page of `project/search` (at most 10).
    async fn project_keys(&self) -> Result<Vec<String>, TrackerError> {
        let mut keys = Vec::new();
        let mut start = 0;
        for _ in 0..10 {
            let v = self
                .call(
                    Request::get(self.url(&format!(
                        "/rest/api/3/project/search?maxResults=100&startAt={start}"
                    ))),
                    Call::Other,
                )
                .await?;
            let values = v["values"].as_array().cloned().unwrap_or_default();
            for p in &values {
                if let Some(k) = p["key"].as_str() {
                    keys.push(k.to_ascii_uppercase());
                }
            }
            if v["isLast"].as_bool().unwrap_or(true) || values.is_empty() {
                break;
            }
            start += values.len();
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// Projects with an open sprint right now (one bounded search).
    async fn sprint_projects(&self) -> Result<Vec<String>, TrackerError> {
        let (issues, _) = self
            .search_page(
                "sprint in openSprints()",
                vec!["project".into()],
                None,
                Call::Other,
            )
            .await?;
        let mut keys: Vec<String> = issues
            .iter()
            .filter_map(|i| i["fields"]["project"]["key"].as_str())
            .map(str::to_ascii_uppercase)
            .collect();
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// Normalise one issue.
    pub fn snapshot(&self, issue: &Value) -> Option<WorkItemSnapshot> {
        let f = &issue["fields"];
        let external_id = match &issue["id"] {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        };
        let key = issue["key"].as_str().map(str::to_ascii_uppercase);
        let status = &f["status"];
        let resolution = f["resolution"]["name"].as_str().map(normalize_resolution);
        let (iteration, iteration_active) = self
            .config
            .sprint_field
            .as_deref()
            .map(|sf| current_sprint(&f[sf]))
            .unwrap_or((None, false));
        Some(WorkItemSnapshot {
            url: key.as_ref().map(|k| format!("{}/browse/{k}", self.site)),
            external_id,
            key,
            aliases: Vec::new(),
            title: f["summary"].as_str().unwrap_or_default().to_string(),
            kind: f["issuetype"]["name"].as_str().map(str::to_string),
            hierarchy_level: f["issuetype"]["hierarchyLevel"].as_i64(),
            status: StatusSnapshot {
                name: status["name"].as_str().unwrap_or_default().to_string(),
                category: map_status_category(status["statusCategory"]["key"].as_str()).into(),
                resolution,
            },
            parent_external_id: match &f["parent"]["id"] {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            },
            parent_key: f["parent"]["key"].as_str().map(str::to_ascii_uppercase),
            containers: f["project"]["key"]
                .as_str()
                .map(|k| vec![k.to_ascii_uppercase()])
                .unwrap_or_default(),
            assignees: f["assignee"]["displayName"]
                .as_str()
                .map(|n| vec![n.to_string()])
                .unwrap_or_default(),
            assignee_id: f["assignee"]["accountId"].as_str().map(str::to_string),
            iteration,
            iteration_active,
            updated: f["updated"].as_str().and_then(super::parse_timestamp),
            description: adf_excerpt(&f["description"]),
        })
    }
}

fn map_transport(e: TransportError) -> TrackerError {
    match e {
        TransportError::Refused(m) => TrackerError::Refused(m),
        TransportError::Connect(m) => TrackerError::Unreachable(m),
        TransportError::Timeout => TrackerError::Unreachable("timed out".into()),
        TransportError::TooLarge(m) | TransportError::Protocol(m) => TrackerError::Invalid(m),
    }
}

/// Map a non-2xx answer (C25–C28 and the plan's error list).
fn check(resp: &Response, call: Call) -> Result<(), TrackerError> {
    if resp.is_success() {
        return Ok(());
    }
    let captcha = resp
        .header("X-Seraph-LoginReason")
        .is_some_and(|r| r.contains("AUTHENTICATION_DENIED"));
    let retry_after = resp
        .header("Retry-After")
        .and_then(|v| v.trim().parse::<u64>().ok());
    Err(match resp.status {
        401 | 403 if captcha => TrackerError::Captcha,
        401 => TrackerError::Auth("401 Unauthorized".into()),
        403 if call == Call::Identity => TrackerError::Auth("403 on /myself".into()),
        403 if call == Call::View => TrackerError::Forbidden("403 on this view's query".into()),
        403 => TrackerError::Forbidden("403".into()),
        404 => TrackerError::NotFound,
        429 => TrackerError::RateLimited {
            retry_after_secs: retry_after,
        },
        503 if retry_after.is_some() => TrackerError::RateLimited {
            retry_after_secs: retry_after,
        },
        300..=399 => TrackerError::Invalid(format!(
            "{} redirect (not followed){}",
            resp.status,
            resp.header("Location")
                .map(|l| format!(" to {}", crate::logging::redact(l)))
                .unwrap_or_default()
        )),
        s => TrackerError::Invalid(format!("HTTP {s}")),
    })
}

/// `statusCategory.key` → fleet's category. `undefined` (C26) and anything
/// unknown count as todo: never claim work is under way or done on a guess.
pub fn map_status_category(key: Option<&str>) -> &'static str {
    match key {
        Some("indeterminate") => "in_progress",
        Some("done") => "done",
        _ => "todo",
    }
}

/// A resolution name → completed | not_planned | duplicate. Conservative:
/// only names that plainly say "not done" or "duplicate" are told apart;
/// everything else (Done, Fixed, a custom name) is `completed`.
pub fn normalize_resolution(name: &str) -> String {
    let n = name.trim().to_lowercase().replace('\u{2019}', "'");
    if n.contains("duplicate") {
        return "duplicate".into();
    }
    const NOT_PLANNED: &[&str] = &[
        "won't do",
        "wont do",
        "won't fix",
        "wont fix",
        "declined",
        "cancelled",
        "canceled",
        "rejected",
        "obsolete",
    ];
    if NOT_PLANNED.contains(&n.as_str()) {
        "not_planned".into()
    } else {
        "completed".into()
    }
}

/// The sprint field's value → (the current sprint's name, it is active). An
/// active sprint wins; else the newest future one; closed ones are history.
fn current_sprint(v: &Value) -> (Option<String>, bool) {
    let Some(list) = v.as_array() else {
        return (None, false);
    };
    let name = |s: &Value| s["name"].as_str().map(str::to_string);
    if let Some(active) = list.iter().find(|s| s["state"] == "active") {
        return (name(active), true);
    }
    (
        list.iter()
            .rev()
            .find(|s| s["state"] == "future")
            .and_then(name),
        false,
    )
}

/// Plain text out of an Atlassian Document Format value, at most
/// [`DESCRIPTION_MAX_CHARS`] characters. A plain string (API v2, or a
/// renderer) is taken as is.
pub fn adf_excerpt(v: &Value) -> Option<String> {
    let mut out = String::new();
    match v {
        Value::String(s) => out.push_str(s),
        Value::Object(_) => adf_walk(v, &mut out),
        _ => return None,
    }
    let text = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(DESCRIPTION_MAX_CHARS).collect())
}

fn adf_walk(node: &Value, out: &mut String) {
    if out.chars().count() > DESCRIPTION_MAX_CHARS {
        return;
    }
    let attrs = &node["attrs"];
    match node["type"].as_str().unwrap_or_default() {
        "text" => out.push_str(node["text"].as_str().unwrap_or_default()),
        "hardBreak" => out.push('\n'),
        "mention" => out.push_str(attrs["text"].as_str().unwrap_or("@someone")),
        "emoji" => out.push_str(attrs["shortName"].as_str().unwrap_or_default()),
        "inlineCard" | "blockCard" => out.push_str(attrs["url"].as_str().unwrap_or_default()),
        "status" => out.push_str(attrs["text"].as_str().unwrap_or_default()),
        "listItem" => out.push_str("- "),
        _ => {}
    }
    if let Some(children) = node["content"].as_array() {
        for c in children {
            adf_walk(c, out);
        }
    }
    if matches!(
        node["type"].as_str(),
        Some("paragraph" | "heading" | "codeBlock" | "blockquote" | "rule" | "listItem")
    ) && !out.ends_with('\n')
    {
        out.push('\n');
    }
}

/// `https://<site>.atlassian.net/browse/ABC-123` (or a board URL with
/// `selectedIssue=ABC-123`) → `(site_url, "ABC-123")`, the site fenced by
/// `normalize_site_url`. For "connect by pasting a ticket URL".
pub fn parse_ticket_url(url: &str) -> Option<(String, String)> {
    let t = url.trim();
    let rest = t.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let site = crate::store::normalize_site_url(&format!("https://{host}")).ok()?;
    let key = key_in_path(path)?;
    Some((site, key))
}

/// The issue key a Jira URL path names.
fn key_in_path(path: &str) -> Option<String> {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let path = path.split('#').next().unwrap_or(path);
    if let Some(k) = path
        .strip_prefix("browse/")
        .map(|k| k.split('/').next().unwrap_or(k))
    {
        if is_key(k) {
            return Some(k.to_ascii_uppercase());
        }
    }
    query
        .split(['&', '#'])
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "selectedIssue")
        .map(|(_, v)| v)
        .filter(|v| is_key(v))
        .map(str::to_ascii_uppercase)
}

/// `ABC-123`: a letter, then letters/digits/underscore, a dash, digits.
fn is_key(s: &str) -> bool {
    let Some((p, n)) = s.split_once('-') else {
        return false;
    };
    p.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && (2..=10).contains(&p.len())
        && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && (1..=7).contains(&n.len())
        && n.chars().all(|c| c.is_ascii_digit())
}

/// Keys with one of `prefixes` in free text, case-insensitive, bounded by
/// non-alphanumerics (C11), upper-cased, in order, without repeats.
pub fn keys_in_text(text: &str, prefixes: &[String]) -> Vec<String> {
    if prefixes.is_empty() {
        return Vec::new();
    }
    let alternation = prefixes
        .iter()
        .map(|p| regex::escape(p))
        .collect::<Vec<_>>()
        .join("|");
    // The regex crate has no lookaround; the boundary is checked by hand.
    let Ok(re) = regex::Regex::new(&format!(r"(?i)({alternation})-(\d{{1,7}})")) else {
        return Vec::new();
    };
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    for m in re.find_iter(text) {
        let before_ok = m.start() == 0 || !bytes[m.start() - 1].is_ascii_alphanumeric();
        let after_ok = m.end() == bytes.len() || !bytes[m.end()].is_ascii_alphanumeric();
        if before_ok && after_ok {
            let k = m.as_str().to_ascii_uppercase();
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

#[async_trait::async_trait]
impl TrackerProvider for JiraCloud {
    fn caps(&self) -> Caps {
        Caps {
            query_lang: Some("jql".into()),
            hierarchy: true,
            iterations: true,
            human_keys: true,
            incremental: true,
            write: false,
        }
    }

    async fn probe(&self) -> Result<TrackerInfo, TrackerError> {
        let me = self
            .call(Request::get(self.url("/rest/api/3/myself")), Call::Identity)
            .await?;
        // The cloud id is nice to have (webhooks, later): never fatal.
        let instance_id = match self
            .call(Request::get(self.url("/_edge/tenant_info")), Call::Other)
            .await
        {
            Ok(v) => v["cloudId"].as_str().map(str::to_string),
            Err(e @ (TrackerError::Auth(_) | TrackerError::Captcha)) => return Err(e),
            Err(_) => None,
        };
        let key_prefixes = self.project_keys().await?;
        let fields = self
            .call(Request::get(self.url("/rest/api/3/field")), Call::Other)
            .await?;
        let sprint_field = fields.as_array().and_then(|fs| {
            fs.iter()
                .find(|f| f["schema"]["custom"] == SPRINT_FIELD_SCHEMA)
                .and_then(|f| f["id"].as_str())
                .map(str::to_string)
        });
        let sprint_projects = if sprint_field.is_some() {
            match self.sprint_projects().await {
                Ok(p) => p,
                Err(e @ (TrackerError::Auth(_) | TrackerError::Captcha)) => return Err(e),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        Ok(TrackerInfo {
            instance_id,
            config: TrackerConfig {
                account_id: me["accountId"].as_str().map(str::to_string),
                display_name: me["displayName"].as_str().map(str::to_string),
                tz: me["timeZone"].as_str().map(str::to_string),
                key_prefixes,
                sprint_projects,
                sprint_field,
            },
        })
    }

    async fn views(&self, config: &TrackerConfig) -> Result<Vec<ViewDef>, TrackerError> {
        let mut v = vec![ViewDef {
            id: "mine".into(),
            label: "My work".into(),
            query: VIEW_MINE.into(),
        }];
        if config.sprint_field.is_some() && !config.sprint_projects.is_empty() {
            v.push(ViewDef {
                id: "sprint".into(),
                label: "Current sprint".into(),
                query: VIEW_SPRINT.into(),
            });
        }
        v.push(ViewDef {
            id: "recent".into(),
            label: "Recent".into(),
            query: VIEW_RECENT.into(),
        });
        let favs = self
            .call(
                Request::get(self.url("/rest/api/3/filter/favourite")),
                Call::Other,
            )
            .await?;
        for f in favs.as_array().into_iter().flatten() {
            let id = match &f["id"] {
                Value::String(s) if s.bytes().all(|b| b.is_ascii_digit()) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            let name = f["name"].as_str().unwrap_or("Filter").trim();
            v.push(ViewDef {
                id: format!("filter:{id}"),
                label: name.chars().take(80).collect(),
                // C28: by reference, never by pasting its JQL.
                query: format!("filter = {id}"),
            });
        }
        Ok(v)
    }

    async fn list(
        &self,
        view: &ViewDef,
        since: Option<i64>,
        cursor: Option<String>,
    ) -> Result<Page, TrackerError> {
        let jql = match since {
            Some(since) => {
                let mins = ((crate::service::catalog::now_secs() - since).max(0) + 59) / 60;
                format!(
                    "({}) AND updated >= -{}m ORDER BY updated DESC",
                    view.query,
                    mins.max(1)
                )
            }
            None => format!("({}) ORDER BY updated DESC", view.query),
        };
        let (issues, next) = self
            .search_page(&jql, self.fields(), cursor, Call::View)
            .await?;
        Ok(Page {
            items: issues.iter().filter_map(|i| self.snapshot(i)).collect(),
            next,
        })
    }

    async fn fetch(&self, refs: &[ItemRef]) -> Result<Vec<Fetched>, TrackerError> {
        let mut out = Vec::with_capacity(refs.len());
        for chunk in refs.chunks(BULK_MAX) {
            let body = json!({
                "issueIdsOrKeys": chunk.iter().map(ItemRef::as_str).collect::<Vec<_>>(),
                "fields": self.fields(),
            });
            let v = self
                .call(
                    Request::post_json(self.url("/rest/api/3/issue/bulkfetch"), &body),
                    Call::Other,
                )
                .await?;
            let mut by_id: HashMap<String, WorkItemSnapshot> = HashMap::new();
            let mut by_key: HashMap<String, String> = HashMap::new();
            for issue in v["issues"].as_array().into_iter().flatten() {
                if let Some(s) = self.snapshot(issue) {
                    if let Some(k) = &s.key {
                        by_key.insert(k.clone(), s.external_id.clone());
                    }
                    by_id.insert(s.external_id.clone(), s);
                }
            }
            for r in chunk {
                let hit = match r {
                    ItemRef::Id(id) => by_id.get(id).cloned(),
                    ItemRef::Key(k) => {
                        let k = k.to_ascii_uppercase();
                        match by_key.get(&k) {
                            Some(id) => by_id.get(id).cloned(),
                            // Asked for a key, answered under another: the
                            // issue moved. Jira resolves old keys, so the one
                            // unmatched answer of a single-key chunk is it.
                            None if chunk.len() == 1 && by_id.len() == 1 => {
                                by_id.values().next().cloned().map(|mut s| {
                                    if !s.aliases.contains(&k) {
                                        s.aliases.push(k.clone());
                                    }
                                    s
                                })
                            }
                            None => None,
                        }
                    }
                };
                out.push(match hit {
                    Some(s) => Fetched::Found(Box::new(s)),
                    None => Fetched::Unavailable {
                        reference: r.as_str().to_string(),
                        reason: NOT_FOUND_OR_NO_PERMISSION.into(),
                    },
                });
            }
        }
        Ok(out)
    }

    fn recognize(&self, text: &str) -> Vec<ItemRef> {
        let mut out: Vec<ItemRef> = Vec::new();
        let site_host = self.site.trim_start_matches("https://");
        for word in
            text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')'))
        {
            if let Some(rest) = word.strip_prefix("https://") {
                let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
                if host.eq_ignore_ascii_case(site_host) {
                    if let Some(k) = key_in_path(path) {
                        let r = ItemRef::Key(k);
                        if !out.contains(&r) {
                            out.push(r);
                        }
                    }
                }
            }
        }
        for k in keys_in_text(text, &self.config.key_prefixes) {
            let r = ItemRef::Key(k);
            if !out.contains(&r) {
                out.push(r);
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "tests_jira.rs"]
mod tests;
