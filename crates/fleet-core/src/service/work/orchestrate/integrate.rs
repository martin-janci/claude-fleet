//! The integration check (orchestration O7, design §8): before a mission
//! closes, Fleet asks git on the host whether its finished branches merge
//! with each other — `git merge-tree --write-tree`, which touches no
//! worktree — and turns a conflict into a proposed task "Resolve
//! integration conflict A × B" that waits for both (a `create` card a
//! person applies).
//!
//! Without git ≥ 2.38 on the host the state is `unknown` and nothing is
//! carded. A pair is looked at again only when one of its heads moved.

use super::Deps;
use crate::shell::quote as shq;
use crate::store::{MissionRow, NewCard, Store, TreeEntry, TreeRef};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// Members one check compares, at most (pairs grow as n²).
pub const INTEGRATE_MEMBERS_MAX: usize = 12;
/// Conflicted paths a card names, at most.
pub const CONFLICT_FILES_MAX: usize = 20;

/// One finished member's branch, as its last implementation left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub item_id: i64,
    pub label: String,
    pub head: String,
    pub host: String,
    pub path: String,
    pub project_id: i64,
}

/// What git said about one pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairState {
    pub a: i64,
    pub b: i64,
    /// `clean | conflict | unknown`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// The members whose implementation finished with a commit, in a checkout
/// Fleet knows.
pub fn branches(s: &Store, m: &MissionRow) -> Vec<Branch> {
    let mut out = Vec::new();
    let Ok(items) = s.mission_items(m.id) else {
        return out;
    };
    for i in items.iter().filter(|i| Some(i.id) != m.root_item_id) {
        let Ok(Some(t)) = s.latest_item_task(i.id, "implement") else {
            continue;
        };
        if t.state != "done" || super::steps::attempt_failed(&t) {
            continue;
        }
        let Some(head) = t.evidence.as_ref().and_then(|e| e.head.clone()) else {
            continue;
        };
        let Some(w) = t
            .worker_session_id
            .and_then(|w| s.get_session_by_id(w).ok().flatten())
        else {
            continue;
        };
        let Some(wt) = w
            .worktree_id
            .and_then(|id| s.get_worktree_row(id).ok().flatten())
        else {
            continue;
        };
        out.push(Branch {
            item_id: i.id,
            label: i.key.clone().unwrap_or_else(|| format!("#{}", i.id)),
            head,
            host: w.host_alias.clone(),
            path: wt.path,
            project_id: wt.project_id,
        });
        if out.len() >= INTEGRATE_MEMBERS_MAX {
            break;
        }
    }
    out
}

/// The script that merges every pair of `heads` in the repo, run in the
/// first of `cwds` (the branches' worktrees) still on disk: one removed
/// worktree must not leave the others unchecked while its heads stay cached.
pub fn merge_tree_script(cwds: &[&str], heads: &[String]) -> String {
    let dirs: Vec<String> = cwds.iter().map(|d| shq(d)).collect();
    let mut s = format!(
        "ok=; for d in {}; do cd -- \"$d\" 2>/dev/null && {{ ok=1; break; }}; done\n\
         [ -n \"$ok\" ] || {{ echo __FLEET_MT_NOREPO; exit 0; }}\n\
         git merge-tree -h 2>&1 | grep -q -- --write-tree || {{ echo __FLEET_MT_OLD; exit 0; }}\n",
        dirs.join(" ")
    );
    for i in 0..heads.len() {
        for j in i + 1..heads.len() {
            s.push_str(&format!(
                "out=$(git merge-tree --write-tree --name-only --no-messages {} {} 2>/dev/null); rc=$?\n\
                 echo \"__FLEET_MT {i} {j} $rc\"\n\
                 [ \"$rc\" = 1 ] && printf '%s\\n' \"$out\" | sed -n '2,{}p' | sed \"s/^/__FLEET_MTF {i} {j} /\"\n",
                shq(&heads[i]),
                shq(&heads[j]),
                CONFLICT_FILES_MAX + 1,
            ));
        }
    }
    s
}

/// PURE: the pairs' states from the script's output, by index.
pub fn parse_merge_tree(stdout: &str, n: usize) -> Vec<(usize, usize, String, Vec<String>)> {
    if stdout
        .lines()
        .any(|l| l == "__FLEET_MT_OLD" || l == "__FLEET_MT_NOREPO")
    {
        return Vec::new();
    }
    let mut states: BTreeMap<(usize, usize), (String, Vec<String>)> = BTreeMap::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("__FLEET_MTF ") {
            let mut p = rest.splitn(3, ' ');
            let (Some(i), Some(j), Some(f)) = (p.next(), p.next(), p.next()) else {
                continue;
            };
            if let (Ok(i), Ok(j)) = (i.parse(), j.parse()) {
                if let Some(e) = states.get_mut(&(i, j)) {
                    if !f.trim().is_empty() && e.1.len() < CONFLICT_FILES_MAX {
                        e.1.push(f.trim().to_string());
                    }
                }
            }
        } else if let Some(rest) = line.strip_prefix("__FLEET_MT ") {
            let p: Vec<&str> = rest.split(' ').collect();
            if p.len() != 3 {
                continue;
            }
            let (Ok(i), Ok(j)) = (p[0].parse::<usize>(), p[1].parse::<usize>()) else {
                continue;
            };
            if i >= n || j >= n || i >= j {
                continue;
            }
            let state = match p[2] {
                "0" => "clean",
                "1" => "conflict",
                _ => "unknown",
            };
            states.insert((i, j), (state.to_string(), Vec::new()));
        }
    }
    states
        .into_iter()
        .map(|((i, j), (st, f))| (i, j, st, f))
        .collect()
}

/// The heads each mission was last checked at, so an unchanged set costs
/// no ssh call.
fn checked() -> &'static Mutex<HashMap<i64, String>> {
    static C: std::sync::OnceLock<Mutex<HashMap<i64, String>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Check `m`'s finished branches against each other; card each conflict.
/// Never fails the tick: what git cannot answer stays `unknown`.
pub async fn check(deps: &Deps, m: &MissionRow) -> Vec<PairState> {
    let found = match deps.store.lock() {
        Ok(s) => branches(&s, m),
        Err(_) => return Vec::new(),
    };
    if found.len() < 2 {
        return Vec::new();
    }
    let key: String = found
        .iter()
        .map(|b| format!("{}={}", b.item_id, b.head))
        .collect::<Vec<_>>()
        .join(",");
    if let Ok(mut c) = checked().lock() {
        if c.get(&m.id) == Some(&key) {
            return Vec::new();
        }
        c.insert(m.id, key);
    }
    // One repo on one host is one script.
    let mut groups: BTreeMap<(String, i64), Vec<&Branch>> = BTreeMap::new();
    for b in &found {
        groups
            .entry((b.host.clone(), b.project_id))
            .or_default()
            .push(b);
    }
    let mut out = Vec::new();
    for ((host, _), group) in groups.into_iter().filter(|(_, g)| g.len() >= 2) {
        let heads: Vec<String> = group.iter().map(|b| b.head.clone()).collect();
        let cwds: Vec<&str> = group.iter().map(|b| b.path.as_str()).collect();
        let script = merge_tree_script(&cwds, &heads);
        let stdout = match crate::service::catalog::inventory::run_host_script(
            &deps.ssh, &host, &script,
        )
        .await
        {
            Ok(o) => o,
            Err(e) => {
                tracing::debug!(mission = m.id, %host, error = %e.message, "[integrate] not checked");
                // Not checked: forget the heads so the next tick asks again
                // (a finished mission's heads never move).
                if let Ok(mut c) = checked().lock() {
                    c.remove(&m.id);
                }
                continue;
            }
        };
        for (i, j, state, files) in parse_merge_tree(&stdout, group.len()) {
            let (a, b) = (group[i], group[j]);
            if state == "conflict" {
                if let Ok(s) = deps.store.lock() {
                    conflict_card(&s, m, a, b, &files);
                }
            }
            out.push(PairState {
                a: a.item_id,
                b: b.item_id,
                state,
                files,
            });
        }
    }
    out
}

/// The proposed task a conflict raises: once per pair of heads.
fn conflict_card(s: &Store, m: &MissionRow, a: &Branch, b: &Branch, files: &[String]) {
    let decision = format!(
        "integrate:{}:{}:{}:{}",
        a.item_id,
        b.item_id,
        &a.head[..a.head.len().min(12)],
        &b.head[..b.head.len().min(12)]
    );
    let entry = TreeEntry {
        title: format!("Resolve integration conflict {} × {}", a.label, b.label),
        notes: (!files.is_empty()).then(|| format!("Conflicting paths: {}", files.join(", "))),
        why: Some("their branches do not merge cleanly (git merge-tree)".into()),
        depends_on: vec![TreeRef::Item(a.item_id), TreeRef::Item(b.item_id)],
    };
    let card = s.add_card(
        m.id,
        &NewCard {
            decision_id: &decision,
            source: "loop",
            kind: "create",
            work_item_id: None,
            payload: Some(serde_json::json!({
                "tree": [entry],
                "done_when": [["review"]],
                "role": "integrate",
            })),
        },
    );
    if let Ok(Some(_)) = card {
        let _ = s.record_mission_event(
            m.id,
            &crate::store::NewMissionEvent {
                kind: "integration",
                actor: "loop",
                work_item_id: None,
                payload: Some(serde_json::json!({
                    "state": "conflict", "a": a.item_id, "b": b.item_id, "files": files,
                })),
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests;
