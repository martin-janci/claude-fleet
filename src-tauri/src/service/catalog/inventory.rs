//! Host inventory: run a harness scan on a host, compare against rendered
//! catalog assets, persist per-asset drift states.
#![allow(dead_code)]

use super::harness::{json_get, ConfigMerge, Harness, HostSnapshot, MergeMode};
use super::model::sha256_hex;
use super::repo::Catalog;
use crate::store::AssetInventoryRow;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetState {
    InSync,
    Drifted,
    Missing,
    Unmanaged,
    Unsupported,
}

impl AssetState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AssetState::InSync => "in_sync",
            AssetState::Drifted => "drifted",
            AssetState::Missing => "missing",
            AssetState::Unmanaged => "unmanaged",
            AssetState::Unsupported => "unsupported",
        }
    }
}

fn is_subset(want: &Value, have: &Value) -> bool {
    match (want, have) {
        (Value::Object(w), Value::Object(h)) => w
            .iter()
            .all(|(k, v)| h.get(k).is_some_and(|hv| is_subset(v, hv))),
        (Value::Array(w), Value::Array(h)) => {
            w.iter().all(|wv| h.iter().any(|hv| is_subset(wv, hv)))
        }
        _ => want == have,
    }
}

/// Does the host's config already satisfy this merge?
pub fn merge_satisfied(snap: &HostSnapshot, m: &ConfigMerge) -> bool {
    let Some(root) = snap.configs.get(&m.file) else {
        return false;
    };
    let Some(have) = json_get(root, &m.json_path) else {
        return false;
    };
    match m.mode {
        MergeMode::Set => have == &m.value,
        MergeMode::AppendUnique => have.as_array().is_some_and(|arr| arr.contains(&m.value)),
        MergeMode::Subset => is_subset(&m.value, have),
    }
}

/// Compare every catalog asset against the snapshot, then add `unmanaged`
/// rows for installed assets the catalog does not know.
pub fn compute_states(
    catalog: &Catalog,
    harness: &dyn Harness,
    host_alias: &str,
    snap: &HostSnapshot,
    scanned_at: i64,
) -> Vec<AssetInventoryRow> {
    let mut rows = Vec::new();
    for asset in &catalog.assets {
        let base = AssetInventoryRow {
            host_alias: host_alias.to_string(),
            harness: harness.id().to_string(),
            kind: asset.kind().as_str().to_string(),
            name: asset.header.name.clone(),
            scanned_at,
            ..Default::default()
        };
        let plan = match harness.render(asset) {
            Ok(p) => p,
            Err(_) => {
                rows.push(AssetInventoryRow {
                    state: AssetState::Unsupported.as_str().into(),
                    ..base
                });
                continue;
            }
        };
        let catalog_hash = plan.hash();
        let mut present = false;
        let mut all_match = true;
        let mut host_parts: Vec<String> = Vec::new();
        for f in &plan.files {
            match snap.files.get(&f.path) {
                Some(h) => {
                    present = true;
                    host_parts.push(format!("{}={h}", f.path));
                    if *h != sha256_hex(&f.bytes) {
                        all_match = false;
                    }
                }
                None => all_match = false,
            }
        }
        for m in &plan.merges {
            let have = snap
                .configs
                .get(&m.file)
                .and_then(|root| json_get(root, &m.json_path));
            let satisfied = merge_satisfied(snap, m);
            // `Set`/`Subset` merges point `json_path` at an asset-specific
            // key (e.g. `mcpServers.<name>`, `plugins.<plugin@marketplace>`),
            // so resolving that path already means *this* asset's entry is
            // present. `AppendUnique` merges (hooks) instead point at a
            // *shared* array keyed only by event (e.g. `hooks.Stop`) that
            // every hook on that event appends into, so resolving the path
            // only proves some hook exists there — not this one. Treat an
            // `AppendUnique` target as present only once its own value is
            // actually found in the array, or a catalog hook whose sibling
            // is installed but who is itself absent would read as `drifted`
            // instead of `missing`.
            let this_present = match m.mode {
                MergeMode::AppendUnique => satisfied,
                MergeMode::Set | MergeMode::Subset => have.is_some(),
            };
            if this_present {
                present = true;
                host_parts.push(format!(
                    "{}:{}={}",
                    m.file,
                    m.json_path.join("/"),
                    have.map(|v| v.to_string()).unwrap_or_default()
                ));
            }
            if !satisfied {
                all_match = false;
            }
        }
        let state = if plan.files.is_empty() && plan.merges.is_empty() {
            AssetState::InSync // disabled target: nothing to install
        } else if !present {
            AssetState::Missing
        } else if all_match {
            AssetState::InSync
        } else {
            AssetState::Drifted
        };
        let host_hash = if present {
            Some(sha256_hex(host_parts.join("\n").as_bytes()))
        } else {
            None
        };
        rows.push(AssetInventoryRow {
            state: state.as_str().into(),
            catalog_hash: Some(catalog_hash),
            host_hash,
            ..base
        });
    }
    for (kind, name) in harness.installed(snap) {
        if catalog.find(kind, &name).is_none() {
            rows.push(AssetInventoryRow {
                host_alias: host_alias.to_string(),
                harness: harness.id().to_string(),
                kind: kind.as_str().to_string(),
                name,
                state: AssetState::Unmanaged.as_str().into(),
                catalog_hash: None,
                host_hash: None,
                scanned_at,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::claude::Claude;
    use crate::service::catalog::harness::{ConfigMerge, Harness, HostSnapshot, MergeMode};
    use crate::service::catalog::model::Asset;
    use crate::service::catalog::repo::Catalog;
    use serde_json::json;

    fn snap_with(configs: Vec<(&str, serde_json::Value)>) -> HostSnapshot {
        let mut s = HostSnapshot::default();
        for (k, v) in configs {
            s.configs.insert(k.into(), v);
        }
        s
    }

    #[test]
    fn merge_satisfied_by_mode() {
        let set = ConfigMerge {
            file: "f".into(),
            json_path: vec!["a".into(), "b".into()],
            mode: MergeMode::Set,
            value: json!({"x": 1}),
        };
        assert!(merge_satisfied(
            &snap_with(vec![("f", json!({"a": {"b": {"x": 1}}}))]),
            &set
        ));
        assert!(!merge_satisfied(
            &snap_with(vec![("f", json!({"a": {"b": {"x": 2}}}))]),
            &set
        ));
        assert!(!merge_satisfied(&snap_with(vec![]), &set));

        let append = ConfigMerge {
            file: "f".into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value: json!({"hooks": [{"type": "command", "command": "x"}]}),
        };
        assert!(merge_satisfied(
            &snap_with(vec![(
                "f",
                json!({"hooks": {"Stop": [{"other": 1}, {"hooks": [{"type": "command", "command": "x"}]}]}})
            )]),
            &append
        ));
        assert!(!merge_satisfied(
            &snap_with(vec![("f", json!({"hooks": {"Stop": [{"other": 1}]}}))]),
            &append
        ));

        let subset = ConfigMerge {
            file: "f".into(),
            json_path: vec!["plugins".into(), "p@m".into()],
            mode: MergeMode::Subset,
            value: json!([{"version": "1"}]),
        };
        assert!(merge_satisfied(
            &snap_with(vec![(
                "f",
                json!({"plugins": {"p@m": [{"version": "1", "scope": "user"}]}})
            )]),
            &subset
        ));
        assert!(!merge_satisfied(
            &snap_with(vec![("f", json!({"plugins": {"p@m": [{"version": "2"}]}}))]),
            &subset
        ));
        let latest = ConfigMerge {
            value: json!([{}]),
            ..subset.clone()
        };
        assert!(merge_satisfied(
            &snap_with(vec![("f", json!({"plugins": {"p@m": [{"version": "2"}]}}))]),
            &latest
        ));
    }

    #[test]
    fn compute_states_covers_all_five_states() {
        let mut cat = Catalog::default();
        let mut skill = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        skill.body = "b\n".into();
        cat.assets.push(skill.clone());
        cat.assets.push(
            Asset::from_yaml(
                None,
                "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: u\n",
            )
            .unwrap(),
        );
        cat.assets
            .push(Asset::from_yaml(None, "kind: agent\nname: gone\ndescription: d\n").unwrap());
        cat.assets.push(Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap());

        let claude = Claude;
        let skill_plan = claude.render(&skill).unwrap();
        let skill_hash = crate::service::catalog::model::sha256_hex(&skill_plan.files[0].bytes);
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), skill_hash);
        snap.files
            .insert("~/.claude/skills/extra/SKILL.md".into(), "zzz".into());
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet": {"type": "http", "url": "OTHER"}}}),
        );
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}),
        );

        let rows = compute_states(&cat, &claude, "local", &snap, 7);
        let state = |kind: &str, name: &str| {
            rows.iter()
                .find(|r| r.kind == kind && r.name == name)
                .map(|r| r.state.clone())
                .unwrap_or_default()
        };
        assert_eq!(state("skill", "s"), "in_sync");
        assert_eq!(state("mcp_server", "fleet"), "drifted");
        assert_eq!(state("agent", "gone"), "missing");
        assert_eq!(state("hook", "h"), "in_sync");
        assert_eq!(state("skill", "extra"), "unmanaged");
        assert!(rows
            .iter()
            .all(|r| r.host_alias == "local" && r.harness == "claude" && r.scanned_at == 7));
        let s = rows.iter().find(|r| r.name == "s").unwrap();
        assert!(s.catalog_hash.is_some() && s.host_hash.is_some());

        let codex = crate::service::catalog::harness::codex::Codex;
        let rows = compute_states(&cat, &codex, "local", &HostSnapshot::default(), 7);
        assert_eq!(
            rows.iter().find(|r| r.name == "gone").unwrap().state,
            "unsupported"
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "s").unwrap().state,
            "missing"
        );
    }

    /// Regression test: an `AppendUnique` (hook) merge's `json_path` points
    /// at the *shared* per-event array (`hooks.Stop`), not an asset-specific
    /// key, so a sibling hook occupying that array must not make an absent
    /// catalog hook read as `drifted` — it must read as `missing`. Once the
    /// catalog hook's own entry is actually present alongside the sibling,
    /// it must read as `in_sync`.
    #[test]
    fn hook_presence_requires_its_own_entry_not_just_a_shared_sibling() {
        use crate::service::catalog::harness::claude::SETTINGS_PATH;

        let mut cat = Catalog::default();
        cat.assets.push(
            Asset::from_yaml(
                None,
                "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n",
            )
            .unwrap(),
        );
        let claude = Claude;

        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            SETTINGS_PATH.into(),
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other"}]}]}}),
        );
        let rows = compute_states(&cat, &claude, "local", &snap, 1);
        assert_eq!(
            rows.iter().find(|r| r.name == "h").unwrap().state,
            "missing"
        );

        snap.configs.insert(
            SETTINGS_PATH.into(),
            json!({"hooks": {"Stop": [
                {"hooks": [{"type": "command", "command": "other"}]},
                {"hooks": [{"type": "command", "command": "x"}]},
            ]}}),
        );
        let rows = compute_states(&cat, &claude, "local", &snap, 1);
        assert_eq!(
            rows.iter().find(|r| r.name == "h").unwrap().state,
            "in_sync"
        );
    }
}
