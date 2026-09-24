//! Asana, read-only (work graph M6.2): Company B's tracker, and the hardest
//! fit for the model — no human keys, and a task can sit in several projects.
//!
//! * **Transport.** `Direct` HTTPS to `https://app.asana.com/api/1.0` only
//!   (the host fence), or `via_host`. A personal access token as
//!   `Authorization: Bearer`.
//! * **Identity** is the task gid. The key is `asana:<gid>` — the
//!   recogniser's canonical form of an Asana URL — which is opaque: nothing
//!   types it, detection is by URL only (both URL forms). The UI shows a
//!   short display key.
//! * **Workspace.** The site is `https://app.asana.com` or
//!   `https://app.asana.com/<workspace gid>`; with several workspaces and
//!   none named, the probe says which to pick.
//! * **Views.** `mine` (the user's open tasks), and one view per project the
//!   user's tasks sit in (at most [`MAX_PROJECT_VIEWS`], found by the
//!   probe). A project view is read incrementally through the **events
//!   API** with a sync token per project; an expired token (412) turns into
//!   one whole listing. `recent` exists only where search does (Premium).
//! * **Status.** `completed` is authoritative (done / completed). Otherwise
//!   the task's section in its first project decides, through the section
//!   map a person confirmed (`settings.section_map`), else the one the probe
//!   inferred from section names (`config.section_map`), else todo.
//! * **Containers** are the task's project gids (memberships).
//! * **Rate limits**: 429 with `Retry-After`; `opt_fields` everywhere, never
//!   `opt_expand`.

use super::{
    check_http, map_transport, CallKind, Caps, Changes, Fetched, Incremental, ItemRef, Page,
    RefCtx, StatusSnapshot, TrackerError, TrackerInfo, TrackerProvider, ViewDef, WorkItemSnapshot,
    DESCRIPTION_MAX_CHARS, NOT_FOUND_OR_NO_PERMISSION,
};
use crate::net::https::{HttpTransport, Request};
use crate::store::{TrackerConfig, TrackerCredential, TrackerSettings};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const API: &str = "https://app.asana.com/api/1.0";
pub const API_HOST: &str = "app.asana.com";
/// Page size (Asana's maximum).
pub const PAGE_SIZE: usize = 100;
/// Project views the probe sets up, at most.
pub const MAX_PROJECT_VIEWS: usize = 10;
/// Tasks one events read fetches by id before it lists the view whole.
pub const MAX_CHANGED: usize = 20;
/// `/batch` actions per call (Asana's limit).
pub const BATCH_MAX: usize = 10;

/// The task fields fleet reads, and nothing else.
pub const TASK_FIELDS: &str = "gid,name,completed,modified_at,notes,permalink_url,\
     resource_subtype,assignee.gid,assignee.name,parent.gid,memberships.project.gid,\
     memberships.project.name,memberships.section.name";

pub struct Asana {
    /// The workspace the site names, if it names one.
    site_workspace: Option<String>,
    config: TrackerConfig,
    settings: TrackerSettings,
    cred: Option<TrackerCredential>,
    transport: Arc<dyn HttpTransport>,
}

/// `https://app.asana.com/<gid>` → the workspace gid.
pub fn site_workspace(site_url: &str) -> Option<String> {
    site_url
        .trim_end_matches('/')
        .strip_prefix("https://app.asana.com/")
        .filter(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
}

/// A gid: digits only, so it can go into a path unescaped.
fn gid_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.bytes().all(|b| b.is_ascii_digit())
}

/// A section name → the category its words suggest: progress / doing /
/// review → in_progress, done / shipped / complete → done, else `None`.
pub fn infer_section(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    const DONE: &[&str] = &["done", "shipped", "complete", "released", "closed"];
    const DOING: &[&str] = &[
        "progress", "doing", "review", "wip", "started", "active", "testing", "qa",
    ];
    if DONE.iter().any(|w| n.contains(w)) {
        Some("done")
    } else if DOING.iter().any(|w| n.contains(w)) {
        Some("in_progress")
    } else {
        None
    }
}

impl Asana {
    pub fn new(
        site_url: &str,
        config: TrackerConfig,
        settings: TrackerSettings,
        cred: Option<TrackerCredential>,
        transport: Arc<dyn HttpTransport>,
    ) -> Self {
        Asana {
            site_workspace: site_workspace(site_url),
            config,
            settings,
            cred,
            transport,
        }
    }

    fn workspace(&self) -> Result<&str, TrackerError> {
        self.config
            .workspace
            .as_deref()
            .or(self.site_workspace.as_deref())
            .ok_or_else(|| TrackerError::Invalid("no Asana workspace yet; test the tracker".into()))
    }

    async fn get(&self, path_and_query: &str, call: CallKind) -> Result<Value, TrackerError> {
        let cred = self.cred.as_ref().ok_or(TrackerError::Unconfigured)?;
        let req = Request::get(format!("{API}{path_and_query}"))
            .header("Authorization", cred.authorization().expose());
        let resp = self.transport.send(req).await.map_err(map_transport)?;
        check_http(&resp, call)?;
        let v: Value = resp.parse_json().map_err(TrackerError::Invalid)?;
        if !v.is_object() {
            return Err(TrackerError::Invalid("not an Asana answer".into()));
        }
        Ok(v)
    }

    /// One page of tasks at `path` (a listing endpoint with its query).
    async fn page(
        &self,
        path: &str,
        cursor: Option<String>,
        call: CallKind,
    ) -> Result<Page, TrackerError> {
        let mut q = format!("{path}&limit={PAGE_SIZE}&opt_fields={TASK_FIELDS}");
        if let Some(c) = cursor {
            if !c
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'='))
            {
                return Err(TrackerError::Invalid("an unusable page offset".into()));
            }
            q.push_str(&format!("&offset={c}"));
        }
        let v = self.get(&q, call).await?;
        let items = v["data"]
            .as_array()
            .ok_or_else(|| TrackerError::Invalid("a task listing without data".into()))?
            .iter()
            .filter_map(|t| self.snapshot(t))
            .collect();
        let next = v["next_page"]["offset"].as_str().map(str::to_string);
        Ok(Page { items, next })
    }

    /// A section's category: the person's map, the inferred one, the words.
    fn section_category(&self, section: &str) -> &str {
        let key = section.trim().to_lowercase();
        if let Some(c) = self.settings.section_map.get(&key) {
            return c;
        }
        if !self.settings.section_map_confirmed {
            if let Some(c) = self.config.section_map.get(&key) {
                return c;
            }
        }
        "todo"
    }

    /// Normalise one task.
    pub fn snapshot(&self, t: &Value) -> Option<WorkItemSnapshot> {
        let gid = t["gid"].as_str().filter(|g| gid_ok(g))?.to_string();
        let memberships = t["memberships"].as_array().cloned().unwrap_or_default();
        let section = memberships
            .first()
            .and_then(|m| m["section"]["name"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("untitled section"));
        let status = if t["completed"].as_bool().unwrap_or(false) {
            StatusSnapshot {
                name: "Completed".into(),
                category: "done".into(),
                resolution: Some("completed".into()),
            }
        } else {
            let category = section.map(|s| self.section_category(s)).unwrap_or("todo");
            // A "Done" section counts as done (plan), but only `completed`
            // says how it was resolved.
            StatusSnapshot {
                name: section.unwrap_or("Open").to_string(),
                category: category.into(),
                resolution: None,
            }
        };
        let parent = t["parent"]["gid"].as_str().filter(|g| gid_ok(g));
        let mut containers: Vec<String> = Vec::new();
        for m in &memberships {
            if let Some(p) = m["project"]["gid"].as_str().filter(|g| gid_ok(g)) {
                if !containers.iter().any(|c| c == p) {
                    containers.push(p.to_string());
                }
            }
        }
        Some(WorkItemSnapshot {
            key: Some(format!("asana:{gid}")),
            aliases: Vec::new(),
            title: t["name"].as_str().unwrap_or_default().to_string(),
            url: t["permalink_url"]
                .as_str()
                .filter(|u| u.starts_with("https://app.asana.com/"))
                .map(str::to_string),
            kind: Some(
                match t["resource_subtype"].as_str() {
                    Some("milestone") => "Milestone",
                    Some("approval") => "Approval",
                    Some("section") => "Section",
                    _ if parent.is_some() => "Subtask",
                    _ => "Task",
                }
                .into(),
            ),
            hierarchy_level: Some(if parent.is_some() { -1 } else { 0 }),
            status,
            parent_external_id: parent.map(str::to_string),
            parent_key: parent.map(|p| format!("asana:{p}")),
            containers,
            assignees: t["assignee"]["name"]
                .as_str()
                .map(|n| vec![n.to_string()])
                .unwrap_or_default(),
            assignee_id: t["assignee"]["gid"].as_str().map(str::to_string),
            iteration: None,
            iteration_active: false,
            updated: t["modified_at"].as_str().and_then(super::parse_timestamp),
            description: t["notes"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(|n| n.chars().take(DESCRIPTION_MAX_CHARS).collect()),
            external_id: gid,
        })
    }

    /// The task gid a reference names.
    fn gid_of(&self, r: &ItemRef) -> Option<String> {
        match r {
            ItemRef::Id(g) if gid_ok(g) => Some(g.clone()),
            ItemRef::Key(k) if k.starts_with("asana:") => {
                Some(k["asana:".len()..].to_string()).filter(|g| gid_ok(g))
            }
            ItemRef::Key(k) | ItemRef::Url(k) => match self.recognize(k, RefCtx::default()).first()
            {
                Some(ItemRef::Key(k)) => k.strip_prefix("asana:").map(str::to_string),
                _ => None,
            },
            _ => None,
        }
    }

    /// Up to [`BATCH_MAX`] tasks by gid in one `/batch` call; `None` for a
    /// task the token cannot see (404 / 403).
    async fn batch_get(
        &self,
        gids: &[String],
    ) -> Result<Vec<Option<WorkItemSnapshot>>, TrackerError> {
        let cred = self.cred.as_ref().ok_or(TrackerError::Unconfigured)?;
        let fields: Vec<&str> = TASK_FIELDS.split(',').collect();
        let actions: Vec<Value> = gids
            .iter()
            .map(|g| {
                json!({
                    "method": "get",
                    "relative_path": format!("/tasks/{g}"),
                    "options": { "fields": fields },
                })
            })
            .collect();
        let req = Request::post_json(
            format!("{API}/batch"),
            &json!({ "data": { "actions": actions } }),
        )
        .header("Authorization", cred.authorization().expose());
        let resp = self.transport.send(req).await.map_err(map_transport)?;
        check_http(&resp, CallKind::Other)?;
        let v: Value = resp.parse_json().map_err(TrackerError::Invalid)?;
        let results = v["data"]
            .as_array()
            .ok_or_else(|| TrackerError::Invalid("a batch answer without data".into()))?;
        let mut out = Vec::with_capacity(gids.len());
        for (i, g) in gids.iter().enumerate() {
            let r = results.get(i);
            let status = r.and_then(|r| r["status_code"].as_u64()).unwrap_or(0);
            out.push(match status {
                200 => r
                    .and_then(|r| self.snapshot(&r["body"]["data"]))
                    .filter(|s| &s.external_id == g),
                401 => return Err(TrackerError::Auth("401 Unauthorized".into())),
                429 => {
                    return Err(TrackerError::RateLimited {
                        retry_after_secs: None,
                    })
                }
                _ => None,
            });
        }
        Ok(out)
    }

    async fn fetch_gids(
        &self,
        gids: &[String],
    ) -> Result<Vec<Option<WorkItemSnapshot>>, TrackerError> {
        let mut out = Vec::with_capacity(gids.len());
        for chunk in gids.chunks(BATCH_MAX) {
            out.extend(self.batch_get(chunk).await?);
        }
        Ok(out)
    }

    /// The listing path of `view` (without paging).
    fn view_path(&self, view: &ViewDef, since: Option<i64>) -> Result<String, TrackerError> {
        let ws = self.workspace()?;
        if view.query == "mine" {
            return Ok(format!(
                "/tasks?assignee=me&workspace={ws}&completed_since=now"
            ));
        }
        if view.query == "recent" {
            let after = since.unwrap_or(crate::service::catalog::now_secs() - 14 * 86_400);
            return Ok(format!(
                "/workspaces/{ws}/tasks/search?assignee.any=me&modified_at.after={}&sort_by=modified_at",
                super::format_timestamp(after)
            ));
        }
        match view.query.strip_prefix("project:") {
            Some(p) if gid_ok(p) => Ok(format!("/projects/{p}/tasks?completed_since=now")),
            _ => Err(TrackerError::Invalid(format!(
                "unknown Asana view {:?}",
                view.query
            ))),
        }
    }
}

#[async_trait::async_trait]
impl TrackerProvider for Asana {
    fn caps(&self) -> Caps {
        Caps {
            query_lang: None,
            hierarchy: true,
            iterations: false,
            human_keys: false,
            repo_relative: false,
            multi_container: true,
            incremental: Incremental::SyncToken,
            write: false,
        }
    }

    async fn probe(&self) -> Result<TrackerInfo, TrackerError> {
        let me = self
            .get(
                "/users/me?opt_fields=gid,name,workspaces.gid,workspaces.name",
                CallKind::Identity,
            )
            .await?;
        let me = &me["data"];
        let user = me["gid"]
            .as_str()
            .ok_or_else(|| TrackerError::Invalid("no user in the answer".into()))?;
        let spaces: Vec<(String, String)> = me["workspaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| {
                Some((
                    w["gid"].as_str().filter(|g| gid_ok(g))?.to_string(),
                    w["name"].as_str().unwrap_or_default().to_string(),
                ))
            })
            .collect();
        let ws = match &self.site_workspace {
            Some(w) if spaces.iter().any(|(g, _)| g == w) => w.clone(),
            Some(w) => {
                return Err(TrackerError::Invalid(format!(
                    "this token's account is not in workspace {w}"
                )))
            }
            None if spaces.len() == 1 => spaces[0].0.clone(),
            None => {
                let names: Vec<String> = spaces.iter().map(|(g, n)| format!("{n} ({g})")).collect();
                return Err(TrackerError::Invalid(format!(
                    "this account has {} workspaces ({}); add the tracker as \
                     https://app.asana.com/<workspace gid>",
                    spaces.len(),
                    names.join(", ")
                )));
            }
        };
        // The projects the user's open tasks sit in, and their sections.
        let mine = self
            .get(
                &format!(
                    "/tasks?assignee=me&workspace={ws}&completed_since=now&limit={PAGE_SIZE}\
                     &opt_fields=memberships.project.gid,memberships.project.name"
                ),
                CallKind::Other,
            )
            .await?;
        let mut projects: Vec<(String, String)> = Vec::new();
        for t in mine["data"].as_array().into_iter().flatten() {
            for m in t["memberships"].as_array().into_iter().flatten() {
                if let Some(g) = m["project"]["gid"].as_str().filter(|g| gid_ok(g)) {
                    if !projects.iter().any(|(p, _)| p == g) && projects.len() < MAX_PROJECT_VIEWS {
                        let name = m["project"]["name"].as_str().unwrap_or("Project");
                        projects.push((g.to_string(), name.chars().take(80).collect()));
                    }
                }
            }
        }
        let mut section_map = BTreeMap::new();
        for (p, _) in &projects {
            let secs = match self
                .get(
                    &format!("/projects/{p}/sections?opt_fields=name"),
                    CallKind::Other,
                )
                .await
            {
                Ok(v) => v,
                Err(e @ (TrackerError::Auth(_) | TrackerError::RateLimited { .. })) => {
                    return Err(e)
                }
                Err(_) => continue,
            };
            for s in secs["data"].as_array().into_iter().flatten() {
                if let Some(name) = s["name"].as_str() {
                    if let Some(c) = infer_section(name) {
                        section_map.insert(name.trim().to_lowercase(), c.to_string());
                    }
                }
            }
        }
        // Search is a Premium feature: 402 / 403 means no `recent` view.
        let search = match self
            .get(
                &format!("/workspaces/{ws}/tasks/search?assignee.any=me&limit=1&opt_fields=gid"),
                CallKind::Other,
            )
            .await
        {
            Ok(_) => true,
            Err(e @ (TrackerError::Auth(_) | TrackerError::RateLimited { .. })) => return Err(e),
            Err(_) => false,
        };
        Ok(TrackerInfo {
            instance_id: Some(ws.clone()),
            config: TrackerConfig {
                account_id: Some(user.to_string()),
                display_name: me["name"].as_str().map(str::to_string),
                tz: Some("UTC".into()),
                workspace: Some(ws),
                projects,
                search,
                section_map,
                ..Default::default()
            },
        })
    }

    async fn views(&self, config: &TrackerConfig) -> Result<Vec<ViewDef>, TrackerError> {
        let mut v = vec![ViewDef {
            id: "mine".into(),
            label: "My tasks".into(),
            query: "mine".into(),
        }];
        for (gid, name) in &config.projects {
            v.push(ViewDef {
                id: format!("project:{gid}"),
                label: name.clone(),
                query: format!("project:{gid}"),
            });
        }
        if config.search {
            v.push(ViewDef {
                id: "recent".into(),
                label: "Recent".into(),
                query: "recent".into(),
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
        let path = self.view_path(view, since)?;
        self.page(&path, cursor, CallKind::View).await
    }

    async fn fetch(&self, refs: &[ItemRef]) -> Result<Vec<Fetched>, TrackerError> {
        let asked: Vec<Option<String>> = refs.iter().map(|r| self.gid_of(r)).collect();
        let gids: Vec<String> = asked.iter().flatten().cloned().collect();
        let mut got = self.fetch_gids(&gids).await?.into_iter();
        Ok(refs
            .iter()
            .zip(asked)
            .map(|(r, g)| match g.and_then(|_| got.next().flatten()) {
                Some(s) => Fetched::Found(Box::new(s)),
                None => Fetched::Unavailable {
                    reference: r.reference(),
                    reason: NOT_FOUND_OR_NO_PERMISSION.into(),
                },
            })
            .collect())
    }

    fn recognize(&self, text: &str, _ctx: RefCtx<'_>) -> Vec<ItemRef> {
        use crate::service::work::recognize::{recognize, RecognizeCtx};
        let mut out = Vec::new();
        for m in recognize(text, &RecognizeCtx::default()) {
            if m.provider.as_deref() == Some("asana") {
                let r = ItemRef::Key(m.key);
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        out
    }

    async fn changes(&self, view: &ViewDef, mark: Option<&str>) -> Result<Changes, TrackerError> {
        // Only a project has an event stream; `mine` and `recent` are
        // listed whole (they are the user's open tasks: small).
        let Some(project) = view.query.strip_prefix("project:").filter(|p| gid_ok(p)) else {
            return Ok(Changes {
                expired: true,
                ..Default::default()
            });
        };
        let mut path = format!("/events?resource={project}");
        if let Some(m) = mark {
            if !m
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':'))
            {
                return Ok(Changes {
                    expired: true,
                    ..Default::default()
                });
            }
            path.push_str(&format!("&sync={m}"));
        }
        let cred = self.cred.as_ref().ok_or(TrackerError::Unconfigured)?;
        let req = Request::get(format!("{API}{path}"))
            .header("Authorization", cred.authorization().expose());
        let resp = self.transport.send(req).await.map_err(map_transport)?;
        // 412: no token yet, or it expired (after about a day). The body
        // carries a fresh one; the caller lists the view whole.
        if resp.status == 412 {
            let v: Value = resp.parse_json().unwrap_or(Value::Null);
            return Ok(Changes {
                items: Vec::new(),
                mark: v["sync"].as_str().map(str::to_string),
                expired: true,
            });
        }
        check_http(&resp, CallKind::View)?;
        let v: Value = resp.parse_json().map_err(TrackerError::Invalid)?;
        let next = v["sync"].as_str().map(str::to_string);
        let mut gids: Vec<String> = Vec::new();
        for e in v["data"].as_array().into_iter().flatten() {
            if e["resource"]["resource_type"] != "task" || e["action"] == "deleted" {
                continue;
            }
            if let Some(g) = e["resource"]["gid"].as_str().filter(|g| gid_ok(g)) {
                if !gids.iter().any(|x| x == g) {
                    gids.push(g.to_string());
                }
            }
        }
        if v["has_more"].as_bool().unwrap_or(false) || gids.len() > MAX_CHANGED {
            // Too much changed to fetch one by one: one whole listing.
            return Ok(Changes {
                items: Vec::new(),
                mark: next,
                expired: true,
            });
        }
        let items = self
            .fetch_gids(&gids)
            .await?
            .into_iter()
            .flatten()
            .collect();
        Ok(Changes {
            items,
            mark: next,
            expired: false,
        })
    }
}

#[cfg(test)]
#[path = "tests_asana.rs"]
mod tests;
