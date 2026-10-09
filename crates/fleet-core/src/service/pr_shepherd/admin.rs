//! The person's side of the shepherd over the control API (step 2): the
//! `pr_shepherd` tool, served only to the hub owner's own paired device
//! (`Access::PersonDevice`), never to the master token an agent holds. The
//! console's `fleet-hub shepherd` writes the same rows.
//!
//! `status` reads; `grant`, `revoke` and `pause_all` write, and the tool
//! checks the device may (a trusted full device) before calling them.

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{
    ShepherdEpisodeRow, ShepherdMergeRow, ShepherdRuleRow, Store, SHEPHERD_LEVELS,
    SHEPHERD_RECIPES_MAX_CHARS,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Most episodes and merges one `status` answers.
pub const STATUS_MAX_LIMIT: u32 = 200;
const STATUS_DEFAULT_LIMIT: u32 = 20;
/// Longest a granted rule may run before it ends on its own.
pub const MAX_RULE_HOURS: i64 = 24 * 90;

/// `pr_shepherd { action, project_id?, level?, hours?, recipes?, limit? }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "PrShepherdParams")]
pub struct ShepherdArgs {
    /// status | grant | revoke | pause_all.
    pub action: String,
    /// grant, revoke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// grant: watch | nudge | merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// grant: the rule ends after this many hours (default never).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hours: Option<i64>,
    /// grant: text added to conflict prompts, ≤ 2000 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipes: Option<String>,
    /// status: episodes and merges, ≤ 200 (default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ShepherdArgs {
    /// Whether the action only reads.
    pub fn is_read(&self) -> bool {
        self.action == "status"
    }
}

/// One rule as `status` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleView {
    #[serde(flatten)]
    pub rule: ShepherdRuleRow,
    /// owner/repo.
    pub project: String,
    /// Not expired.
    pub active: bool,
}

/// What `status` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShepherdStatus {
    pub rules: Vec<RuleView>,
    pub episodes: Vec<ShepherdEpisodeRow>,
    pub merges: Vec<ShepherdMergeRow>,
}

/// What a write answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShepherdChanged {
    /// Rules written or removed.
    pub changed: u32,
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_VALIDATE, msg.into())
}

fn project_name(s: &Store, id: i64) -> Result<Option<String>, IpcError> {
    Ok(s.list_projects()
        .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| format!("{}/{}", p.owner, p.repo)))
}

pub fn status(
    store: &Mutex<Store>,
    args: &ShepherdArgs,
    now: i64,
) -> Result<ShepherdStatus, IpcError> {
    let limit = args
        .limit
        .unwrap_or(STATUS_DEFAULT_LIMIT)
        .min(STATUS_MAX_LIMIT) as usize;
    let s = lock(store)?;
    let db = |e: rusqlite::Error| IpcError::new(codes::E_INTERNAL, e.to_string());
    let mut rules = Vec::new();
    for rule in s.list_shepherd_rules().map_err(db)? {
        rules.push(RuleView {
            project: project_name(&s, rule.project_id)?
                .unwrap_or_else(|| format!("project {}", rule.project_id)),
            active: rule.active_at(now),
            rule,
        });
    }
    Ok(ShepherdStatus {
        rules,
        episodes: s.list_shepherd_episodes(limit).map_err(db)?,
        merges: s.list_shepherd_merges(limit).map_err(db)?,
    })
}

/// Write a rule. `granted_by` names the device, for the record.
pub fn grant(
    store: &Mutex<Store>,
    args: &ShepherdArgs,
    granted_by: &str,
    now: i64,
) -> Result<ShepherdChanged, IpcError> {
    let project_id = args
        .project_id
        .ok_or_else(|| invalid("grant needs project_id"))?;
    let level = args.level.as_deref().unwrap_or("watch");
    if !SHEPHERD_LEVELS.contains(&level) {
        return Err(invalid(format!(
            "level must be one of {}",
            SHEPHERD_LEVELS.join(", ")
        )));
    }
    if let Some(h) = args.hours {
        if !(1..=MAX_RULE_HOURS).contains(&h) {
            return Err(invalid(format!("hours must be 1-{MAX_RULE_HOURS}")));
        }
    }
    if args
        .recipes
        .as_deref()
        .is_some_and(|r| r.chars().count() > SHEPHERD_RECIPES_MAX_CHARS)
    {
        return Err(invalid(format!(
            "recipes must be at most {SHEPHERD_RECIPES_MAX_CHARS} characters"
        )));
    }
    let s = lock(store)?;
    if project_name(&s, project_id)?.is_none() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no project {project_id}"),
        ));
    }
    s.grant_shepherd_rule(&ShepherdRuleRow {
        project_id,
        level: level.to_string(),
        granted_by: granted_by.to_string(),
        granted_at: now,
        expires_at: args.hours.map(|h| now + h * 3600),
        recipes: args.recipes.clone().filter(|r| !r.trim().is_empty()),
    })
    .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
    Ok(ShepherdChanged { changed: 1 })
}

pub fn revoke(store: &Mutex<Store>, args: &ShepherdArgs) -> Result<ShepherdChanged, IpcError> {
    let project_id = args
        .project_id
        .ok_or_else(|| invalid("revoke needs project_id"))?;
    let gone = lock(store)?
        .revoke_shepherd_rule(project_id)
        .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
    Ok(ShepherdChanged {
        changed: u32::from(gone),
    })
}

pub fn pause_all(store: &Mutex<Store>) -> Result<ShepherdChanged, IpcError> {
    let n = lock(store)?
        .revoke_all_shepherd_rules()
        .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
    Ok(ShepherdChanged { changed: n as u32 })
}

/// Dispatch one call. The caller has already checked the device may write.
pub fn call(
    store: &Mutex<Store>,
    args: &ShepherdArgs,
    granted_by: &str,
    now: i64,
) -> Result<serde_json::Value, IpcError> {
    let to_json = |v: serde_json::Result<serde_json::Value>| {
        v.map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))
    };
    match args.action.as_str() {
        "status" => to_json(serde_json::to_value(status(store, args, now)?)),
        "grant" => to_json(serde_json::to_value(grant(store, args, granted_by, now)?)),
        "revoke" => to_json(serde_json::to_value(revoke(store, args)?)),
        "pause_all" => to_json(serde_json::to_value(pause_all(store)?)),
        other => Err(invalid(format!(
            "unknown pr_shepherd action {other:?}: status | grant | revoke | pause_all"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        (Mutex::new(s), pid)
    }

    fn args(action: &str) -> ShepherdArgs {
        ShepherdArgs {
            action: action.into(),
            ..Default::default()
        }
    }

    #[test]
    fn grant_status_revoke_round_trip() {
        let (st, pid) = store();
        let mut a = args("grant");
        a.project_id = Some(pid);
        a.level = Some("merge".into());
        a.hours = Some(2);
        a.recipes = Some("REGEN_DOCS=1 cargo fleet-test -- reference_is_current".into());
        assert_eq!(call(&st, &a, "phone", 1000).unwrap()["changed"], 1);
        let v = call(&st, &args("status"), "phone", 1000).unwrap();
        assert_eq!(v["rules"][0]["project"], "o/r");
        assert_eq!(v["rules"][0]["level"], "merge");
        assert_eq!(v["rules"][0]["granted_by"], "phone");
        assert_eq!(v["rules"][0]["expires_at"], 1000 + 7200);
        assert_eq!(v["rules"][0]["active"], true);
        let later = call(&st, &args("status"), "phone", 1000 + 7200).unwrap();
        assert_eq!(later["rules"][0]["active"], false);
        let mut r = args("revoke");
        r.project_id = Some(pid);
        assert_eq!(call(&st, &r, "phone", 0).unwrap()["changed"], 1);
        assert_eq!(call(&st, &r, "phone", 0).unwrap()["changed"], 0);
    }

    #[test]
    fn grant_refuses_bad_input_and_unknown_projects() {
        let (st, pid) = store();
        let mut a = args("grant");
        assert!(call(&st, &a, "x", 0).is_err(), "no project");
        a.project_id = Some(pid + 99);
        assert_eq!(call(&st, &a, "x", 0).unwrap_err().code, codes::E_NOTFOUND);
        a.project_id = Some(pid);
        a.level = Some("auto".into());
        assert!(call(&st, &a, "x", 0).is_err());
        a.level = Some("nudge".into());
        a.hours = Some(0);
        assert!(call(&st, &a, "x", 0).is_err());
        a.hours = Some(MAX_RULE_HOURS + 1);
        assert!(call(&st, &a, "x", 0).is_err());
        a.hours = None;
        a.recipes = Some("x".repeat(SHEPHERD_RECIPES_MAX_CHARS + 1));
        assert!(call(&st, &a, "x", 0).is_err());
        assert!(call(&st, &args("nope"), "x", 0).is_err());
        assert!(lock(&st).unwrap().list_shepherd_rules().unwrap().is_empty());
    }

    #[test]
    fn pause_all_removes_every_rule() {
        let (st, pid) = store();
        let mut a = args("grant");
        a.project_id = Some(pid);
        call(&st, &a, "x", 0).unwrap();
        assert_eq!(call(&st, &args("pause_all"), "x", 0).unwrap()["changed"], 1);
        assert!(args("status").is_read() && !args("pause_all").is_read());
    }
}
