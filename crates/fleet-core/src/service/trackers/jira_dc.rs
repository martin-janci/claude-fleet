//! Jira Data Center / Server, read-only (work graph M6.5).
//!
//! * **Site** is admin-configured (`work_admin`, Master only):
//!   `https://<host>[/<context path>]`, https only, no userinfo, no port,
//!   and the transport's fence is that exact host. `Direct` resolves the
//!   host first and refuses loopback, link-local (cloud metadata) and
//!   unspecified addresses unless the admin set `allow_private_network`,
//!   then connects to the address it checked; an internal CA is trusted
//!   through `settings.extra_ca`. A site only a VPN host can reach uses
//!   `via_host` instead (curl there, with that host's trust store).
//! * **Auth** is a personal access token (`Authorization: Bearer`).
//! * **API v2**: `/rest/api/2/search` paged by `startAt` / `total` (not
//!   `search/jql`); by-reference reads are searches with
//!   `validateQuery: warn`, so a key or id the site does not have is a
//!   warning, not a failed batch.
//! * **Epic** comes from the Epic Link field, found by `schema.custom`;
//!   `parent` covers sub-tasks; `hierarchy_level` is the issue type's where
//!   the site says, else -1 for a sub-task type.
//! * Everything else — status category, resolution, sprints, views,
//!   favourite filters, CAPTCHA — is Cloud's, through [`super::jira_common`].

use super::jira_common::{
    adf_excerpt, check, current_sprint, key_in_path, keys_in_prose, map_status_category,
    normalize_resolution, EPIC_LINK_SCHEMA, SPRINT_FIELD_SCHEMA,
};
use super::{
    map_transport, CallKind, Caps, Fetched, Incremental, ItemRef, Page, RefCtx, StatusSnapshot,
    TrackerError, TrackerInfo, TrackerProvider, ViewDef, WorkItemSnapshot,
    NOT_FOUND_OR_NO_PERMISSION,
};
use crate::net::https::{HttpTransport, Request};
use crate::store::{TrackerConfig, TrackerCredential};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// `s` with the old key `k` it was asked under recorded as an alias.
fn with_alias(mut s: WorkItemSnapshot, k: &str) -> WorkItemSnapshot {
    if !s.aliases.iter().any(|a| a == k) {
        s.aliases.push(k.to_string());
    }
    s
}

/// Page size for `/search`.
pub const PAGE_SIZE: usize = 100;
/// Most references one by-reference search asks for.
pub const FETCH_MAX: usize = 50;

pub use super::jira::{VIEW_MINE, VIEW_RECENT, VIEW_SPRINT};

pub struct JiraDc {
    site: String,
    config: TrackerConfig,
    cred: Option<TrackerCredential>,
    transport: Arc<dyn HttpTransport>,
}

impl JiraDc {
    pub fn new(
        site_url: &str,
        config: TrackerConfig,
        cred: Option<TrackerCredential>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        JiraDc {
            site: site_url.trim_end_matches('/').to_string(),
            config,
            cred,
            transport,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.site)
    }

    fn host(&self) -> &str {
        let rest = self.site.trim_start_matches("https://");
        rest.split('/').next().unwrap_or(rest)
    }

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
        f.extend(self.config.sprint_field.clone());
        f.extend(self.config.epic_field.clone());
        f
    }

    async fn call(&self, req: Request, call: CallKind) -> Result<Value, TrackerError> {
        let cred = self.cred.as_ref().ok_or(TrackerError::Unconfigured)?;
        let req = req.header("Authorization", cred.authorization().expose());
        let resp = self.transport.send(req).await.map_err(map_transport)?;
        check(&resp, call)?;
        resp.parse_json::<Value>().map_err(TrackerError::Invalid)
    }

    /// One `/search` page, as answered.
    async fn search_raw(
        &self,
        jql: &str,
        fields: Vec<String>,
        start_at: usize,
        warn: bool,
        call: CallKind,
    ) -> Result<Value, TrackerError> {
        let mut body = json!({
            "jql": jql,
            "startAt": start_at,
            "maxResults": PAGE_SIZE,
            "fields": fields,
        });
        if warn {
            body["validateQuery"] = json!("warn");
        }
        let v = self
            .call(
                Request::post_json(self.url("/rest/api/2/search"), &body),
                call,
            )
            .await?;
        if !v["issues"].is_array() {
            return Err(TrackerError::Invalid(
                "a search answer without issues".into(),
            ));
        }
        Ok(v)
    }

    /// One `/search` page: `(issues, the next startAt)`.
    async fn search(
        &self,
        jql: &str,
        fields: Vec<String>,
        start_at: usize,
        warn: bool,
        call: CallKind,
    ) -> Result<(Vec<Value>, Option<usize>), TrackerError> {
        let v = self.search_raw(jql, fields, start_at, warn, call).await?;
        let issues = v["issues"].as_array().cloned().unwrap_or_default();
        let total = v["total"].as_u64().unwrap_or(0) as usize;
        let next = start_at + issues.len();
        Ok((
            issues.clone(),
            (!issues.is_empty() && next < total).then_some(next),
        ))
    }

    /// The issues `refs` (ids or keys) name, in one search with
    /// `validateQuery: warn`, and the references the site's warnings say it
    /// does not have (`An issue with key 'PLAT-999' does not exist…`),
    /// upper-cased.
    async fn by_reference(
        &self,
        refs: &[ItemRef],
    ) -> Result<(Vec<WorkItemSnapshot>, HashSet<String>), TrackerError> {
        let ids: Vec<String> = refs
            .iter()
            .filter_map(|r| match r {
                ItemRef::Id(i) => Some(i.clone()),
                _ => None,
            })
            .collect();
        let keys: Vec<String> = refs
            .iter()
            .filter_map(|r| match r {
                ItemRef::Key(k) => Some(k.clone()),
                _ => None,
            })
            .collect();
        let mut clauses = Vec::new();
        if !ids.is_empty() {
            clauses.push(format!("id in ({})", ids.join(",")));
        }
        if !keys.is_empty() {
            clauses.push(format!("key in ({})", keys.join(",")));
        }
        let v = self
            .search_raw(
                &clauses.join(" OR "),
                self.fields(),
                0,
                true,
                CallKind::Other,
            )
            .await?;
        let issues = v["issues"].as_array().cloned().unwrap_or_default();
        let missing: HashSet<String> = v["warningMessages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .flat_map(|m| {
                // Every quoted value: the key or id the warning is about.
                m.split('\'')
                    .skip(1)
                    .step_by(2)
                    .map(str::to_ascii_uppercase)
                    .collect::<Vec<_>>()
            })
            .collect();
        Ok((self.snapshots(&issues), missing))
    }

    /// Normalise one issue (v2 fields: a plain-text description, the Epic
    /// Link as a key).
    pub fn snapshot(&self, issue: &Value) -> Option<WorkItemSnapshot> {
        let f = &issue["fields"];
        let external_id = match &issue["id"] {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        };
        let key = issue["key"].as_str().map(str::to_ascii_uppercase);
        let status = &f["status"];
        let (iteration, iteration_active) = self
            .config
            .sprint_field
            .as_deref()
            .map(|sf| current_sprint(&f[sf]))
            .unwrap_or((None, false));
        let epic = self
            .config
            .epic_field
            .as_deref()
            .and_then(|ef| f[ef].as_str())
            .map(str::to_ascii_uppercase);
        let parent_id = match &f["parent"]["id"] {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        };
        let parent_key = f["parent"]["key"].as_str().map(str::to_ascii_uppercase);
        let level = f["issuetype"]["hierarchyLevel"].as_i64().or_else(|| {
            f["issuetype"]["subtask"]
                .as_bool()
                .filter(|s| *s)
                .map(|_| -1)
        });
        Some(WorkItemSnapshot {
            url: key.as_ref().map(|k| format!("{}/browse/{k}", self.site)),
            external_id,
            key,
            aliases: Vec::new(),
            title: f["summary"].as_str().unwrap_or_default().to_string(),
            kind: f["issuetype"]["name"].as_str().map(str::to_string),
            hierarchy_level: level,
            status: StatusSnapshot {
                name: status["name"].as_str().unwrap_or_default().to_string(),
                category: map_status_category(status["statusCategory"]["key"].as_str()).into(),
                resolution: f["resolution"]["name"].as_str().map(normalize_resolution),
            },
            parent_external_id: parent_id,
            parent_key: parent_key.or(epic),
            containers: f["project"]["key"]
                .as_str()
                .map(|k| vec![k.to_ascii_uppercase()])
                .unwrap_or_default(),
            assignees: f["assignee"]["displayName"]
                .as_str()
                .map(|n| vec![n.to_string()])
                .unwrap_or_default(),
            // Data Center identifies users by `name` (Cloud: accountId).
            assignee_id: f["assignee"]["name"].as_str().map(str::to_string),
            iteration,
            iteration_active,
            updated: f["updated"].as_str().and_then(super::parse_timestamp),
            description: adf_excerpt(&f["description"]),
        })
    }

    /// Snapshots of a page, epics linked by key resolved to their ids when
    /// the epic is in the same page.
    fn snapshots(&self, issues: &[Value]) -> Vec<WorkItemSnapshot> {
        let mut items: Vec<WorkItemSnapshot> =
            issues.iter().filter_map(|i| self.snapshot(i)).collect();
        let ids: HashMap<String, String> = items
            .iter()
            .filter_map(|i| Some((i.key.clone()?, i.external_id.clone())))
            .collect();
        for i in &mut items {
            if i.parent_external_id.is_none() {
                if let Some(pk) = &i.parent_key {
                    i.parent_external_id = ids.get(pk).cloned();
                }
            }
        }
        items
    }
}

#[async_trait::async_trait]
impl TrackerProvider for JiraDc {
    fn caps(&self) -> Caps {
        Caps {
            query_lang: Some("jql".into()),
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
        let me = self
            .call(
                Request::get(self.url("/rest/api/2/myself")),
                CallKind::Identity,
            )
            .await?;
        let projects = self
            .call(
                Request::get(self.url("/rest/api/2/project")),
                CallKind::Other,
            )
            .await?;
        let mut key_prefixes: Vec<String> = projects
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["key"].as_str())
            .map(str::to_ascii_uppercase)
            .collect();
        key_prefixes.sort();
        key_prefixes.dedup();
        let fields = self
            .call(Request::get(self.url("/rest/api/2/field")), CallKind::Other)
            .await?;
        let field = |schema: &str| {
            fields.as_array().and_then(|fs| {
                fs.iter()
                    .find(|f| f["schema"]["custom"] == schema)
                    .and_then(|f| f["id"].as_str())
                    .map(str::to_string)
            })
        };
        let sprint_field = field(SPRINT_FIELD_SCHEMA);
        let epic_field = field(EPIC_LINK_SCHEMA);
        let sprint_projects = if sprint_field.is_some() {
            match self
                .search(
                    "sprint in openSprints()",
                    vec!["project".into()],
                    0,
                    false,
                    CallKind::Other,
                )
                .await
            {
                Ok((issues, _)) => {
                    let mut k: Vec<String> = issues
                        .iter()
                        .filter_map(|i| i["fields"]["project"]["key"].as_str())
                        .map(str::to_ascii_uppercase)
                        .collect();
                    k.sort();
                    k.dedup();
                    k
                }
                Err(e @ (TrackerError::Auth(_) | TrackerError::Captcha)) => return Err(e),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        Ok(TrackerInfo {
            instance_id: Some(self.host().to_string()),
            config: TrackerConfig {
                account_id: me["name"].as_str().map(str::to_string),
                display_name: me["displayName"].as_str().map(str::to_string),
                tz: me["timeZone"].as_str().map(str::to_string),
                key_prefixes,
                sprint_projects,
                sprint_field,
                epic_field,
                ..Default::default()
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
                Request::get(self.url("/rest/api/2/filter/favourite")),
                CallKind::Other,
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
        let start = match cursor {
            Some(c) => c
                .parse::<usize>()
                .map_err(|_| TrackerError::Invalid("an unusable page cursor".into()))?,
            None => 0,
        };
        let (issues, next) = self
            .search(&jql, self.fields(), start, false, CallKind::View)
            .await?;
        Ok(Page {
            items: self.snapshots(&issues),
            next: next.map(|n| n.to_string()),
        })
    }

    async fn fetch(&self, refs: &[ItemRef]) -> Result<Vec<Fetched>, TrackerError> {
        // Every reference as an id (digits) or a key; anything else, or a
        // URL on another site, is unavailable without asking.
        let host = self.host().to_string();
        let norm: Vec<Option<ItemRef>> = refs
            .iter()
            .map(|r| match r {
                ItemRef::Id(id) if id.bytes().all(|b| b.is_ascii_digit()) && !id.is_empty() => {
                    Some(r.clone())
                }
                ItemRef::Key(k) if super::jira_common::is_key(k) => {
                    Some(ItemRef::Key(k.to_ascii_uppercase()))
                }
                ItemRef::Key(u) | ItemRef::Url(u) => u
                    .strip_prefix("https://")
                    .and_then(|rest| rest.split_once('/'))
                    .filter(|(h, _)| h.eq_ignore_ascii_case(&host))
                    .and_then(|(_, path)| {
                        let site_path = self.site.trim_start_matches("https://");
                        let ctx = site_path.split_once('/').map(|(_, c)| c).unwrap_or("");
                        let path = path
                            .strip_prefix(ctx)
                            .unwrap_or(path)
                            .trim_start_matches('/');
                        key_in_path(path)
                    })
                    .map(ItemRef::Key),
                _ => None,
            })
            .collect();
        let mut answers: Vec<Option<WorkItemSnapshot>> = vec![None; refs.len()];
        let askable: Vec<(usize, ItemRef)> = norm
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.clone().map(|r| (i, r)))
            .collect();
        for chunk in askable.chunks(FETCH_MAX) {
            let refs: Vec<ItemRef> = chunk.iter().map(|(_, r)| r.clone()).collect();
            let (items, missing) = self.by_reference(&refs).await?;
            let by_id: HashMap<&str, &WorkItemSnapshot> =
                items.iter().map(|s| (s.external_id.as_str(), s)).collect();
            let by_key: HashMap<&str, &WorkItemSnapshot> = items
                .iter()
                .filter_map(|s| Some((s.key.as_deref()?, s)))
                .collect();
            let mut used: HashSet<String> = HashSet::new();
            // Keys the site neither answered under their own key nor warned
            // about: asked under an old key, answered under the new (Jira
            // resolves old keys in JQL).
            let mut leftover: Vec<(usize, String)> = Vec::new();
            for (i, r) in chunk {
                let hit = match r {
                    ItemRef::Id(id) => by_id.get(id.as_str()).copied(),
                    ItemRef::Key(k) => {
                        let hit = by_key.get(k.as_str()).copied();
                        if hit.is_none() && !missing.contains(k) {
                            leftover.push((*i, k.clone()));
                        }
                        hit
                    }
                    _ => None,
                };
                if let Some(s) = hit {
                    used.insert(s.external_id.clone());
                    answers[*i] = Some(s.clone());
                }
            }
            if leftover.is_empty() {
                continue;
            }
            // The answers no asked reference matched are the moved issues.
            // One of each pairs up; more than one is ambiguous, so those
            // keys are searched for again one at a time.
            let spare: Vec<&WorkItemSnapshot> = items
                .iter()
                .filter(|s| !used.contains(&s.external_id))
                .collect();
            if spare.is_empty() {
                // Nothing came back for them: unavailable, no second ask.
                continue;
            }
            if let ([(i, k)], [s]) = (leftover.as_slice(), spare.as_slice()) {
                answers[*i] = Some(with_alias((*s).clone(), k));
                continue;
            }
            for (i, k) in leftover {
                let (one, _) = self.by_reference(&[ItemRef::Key(k.clone())]).await?;
                if let (1, Some(s)) = (one.len(), one.into_iter().next()) {
                    answers[i] = Some(with_alias(s, &k));
                }
            }
        }
        Ok(refs
            .iter()
            .zip(answers)
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
        let mut out: Vec<ItemRef> = Vec::new();
        let site = self
            .site
            .trim_start_matches("https://")
            .to_ascii_lowercase();
        for word in
            text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')'))
        {
            if let Some(rest) = word.strip_prefix("https://") {
                if let Some(path) = rest
                    .to_ascii_lowercase()
                    .strip_prefix(&site)
                    .map(|_| &rest[site.len()..])
                {
                    if let Some(k) = key_in_path(path.trim_start_matches('/')) {
                        let r = ItemRef::Key(k);
                        if !out.contains(&r) {
                            out.push(r);
                        }
                    }
                }
            }
        }
        // Keys outside URLs only: a key in another site's URL is not ours.
        for k in keys_in_prose(text, &self.config.key_prefixes) {
            let r = ItemRef::Key(k);
            if !out.contains(&r) {
                out.push(r);
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "tests_jira_dc.rs"]
mod tests;
