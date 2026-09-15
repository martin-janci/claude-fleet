//! The managed manifest: a JSON file the sync engine writes on the host
//! (one per harness, at `Harness::manifest_path()`) recording exactly which
//! files and config merges fleet put there for each catalog asset, so a
//! later sync can update or remove precisely what an earlier one added
//! without disturbing anything else on the host.
//!
//! Read by `inventory::compute_states` (to tell a managed asset from one
//! that was merely found installed) and by `sync::plan`; written by
//! `sync::apply` at the end of a successful host sync.

use super::super::harness::{value_hash, HostSnapshot, ManifestMerge, RenderPlan};
use super::super::model::Kind;
use super::super::repo::Catalog;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What fleet wrote on the host for one catalog asset: which files, which
/// config merges (as `ManifestMerge`, so the value itself is never kept
/// around — only its hash), the overall `RenderPlan` hash it came from, and
/// when it was last synced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub hash: String,
    pub files: Vec<String>,
    pub merges: Vec<ManifestMerge>,
    pub synced_at: i64,
}

/// The manifest file itself: every managed asset keyed by `Manifest::key`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub updated_at: i64,
    pub assets: BTreeMap<String, ManifestEntry>,
}

impl Manifest {
    /// The manifest key for one asset, e.g. `"skill/worktree"`.
    pub fn key(kind: Kind, name: &str) -> String {
        format!("{}/{name}", kind.as_str())
    }

    /// Reverse of `key`. `None` if the prefix isn't a known kind.
    pub fn split_key(key: &str) -> Option<(Kind, String)> {
        let (k, name) = key.split_once('/')?;
        let kind = Kind::ALL.iter().copied().find(|kind| kind.as_str() == k)?;
        Some((kind, name.to_string()))
    }

    /// Read the manifest from a host snapshot's config block at `path`
    /// (the harness's `manifest_path()`). A missing or unparseable manifest
    /// is treated as "nothing synced yet" rather than an error: returns a
    /// fresh `Manifest` at `version: 1`.
    pub fn from_snapshot(snap: &HostSnapshot, path: &str) -> Manifest {
        snap.configs
            .get(path)
            .and_then(|v| serde_json::from_value::<Manifest>(v.clone()).ok())
            .unwrap_or(Manifest {
                version: 1,
                ..Default::default()
            })
    }

    /// Serialise for writing to the host: pretty-printed, trailing
    /// newline, `version` always forced to 1 (the only schema so far).
    pub fn to_json(&self) -> String {
        let mut out = self.clone();
        out.version = 1;
        let mut s = serde_json::to_string_pretty(&out).unwrap_or_default();
        s.push('\n');
        s
    }

    /// Build the entry to record for a just-applied `plan`: `hash` is the
    /// plan's overall content hash (see `RenderPlan::hash`), `files` are
    /// the paths it wrote, and `merges` hash the (already-substituted)
    /// merge values rather than keeping them.
    pub fn entry_for(hash: &str, plan: &RenderPlan, now: i64) -> ManifestEntry {
        ManifestEntry {
            hash: hash.to_string(),
            files: plan.files.iter().map(|f| f.path.clone()).collect(),
            merges: plan
                .merges
                .iter()
                .map(|m| ManifestMerge {
                    file: m.file.clone(),
                    json_path: m.json_path.clone(),
                    mode: m.mode,
                    value_hash: value_hash(&m.value),
                })
                .collect(),
            synced_at: now,
        }
    }

    /// Manifest entries whose `(kind, name)` is no longer in `catalog` (the
    /// asset was removed, or renamed) — candidates for `remove_merges` /
    /// deletion the next time this host is synced.
    pub fn orphans<'a>(&'a self, catalog: &Catalog) -> Vec<(&'a str, &'a ManifestEntry)> {
        self.assets
            .iter()
            .filter(|(key, _)| match Self::split_key(key) {
                Some((kind, name)) => catalog.find(kind, &name).is_none(),
                None => true,
            })
            .map(|(key, entry)| (key.as_str(), entry))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::{ConfigMerge, FileWrite, MergeMode};
    use crate::service::catalog::model::Asset;
    use serde_json::json;

    fn skill(name: &str) -> Asset {
        Asset::from_yaml(
            None,
            &format!("kind: skill\nname: {name}\ndescription: d\n"),
        )
        .unwrap()
    }

    #[test]
    fn key_round_trips_through_split_key() {
        let key = Manifest::key(Kind::Skill, "worktree");
        assert_eq!(key, "skill/worktree");
        assert_eq!(
            Manifest::split_key(&key),
            Some((Kind::Skill, "worktree".to_string()))
        );
        assert_eq!(Manifest::split_key("bogus/name"), None);
        assert_eq!(Manifest::split_key("no-slash"), None);
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let mut m = Manifest {
            version: 1,
            updated_at: 42,
            assets: BTreeMap::new(),
        };
        m.assets.insert(
            Manifest::key(Kind::Skill, "worktree"),
            ManifestEntry {
                hash: "abc".into(),
                files: vec!["~/.claude/skills/worktree/SKILL.md".into()],
                merges: vec![ManifestMerge {
                    file: "~/.claude/settings.json".into(),
                    json_path: vec!["hooks".into()],
                    mode: MergeMode::AppendUnique,
                    value_hash: "deadbeef".into(),
                }],
                synced_at: 100,
            },
        );
        let json = m.to_json();
        assert!(json.ends_with('\n'));
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn to_json_forces_version_1() {
        let m = Manifest {
            version: 99,
            ..Default::default()
        };
        let back: Manifest = serde_json::from_str(&m.to_json()).unwrap();
        assert_eq!(back.version, 1);
    }

    #[test]
    fn from_snapshot_tolerates_missing_and_invalid() {
        let snap = HostSnapshot::default();
        let m = Manifest::from_snapshot(&snap, "~/.claude/.fleet-assets.json");
        assert_eq!(
            m,
            Manifest {
                version: 1,
                ..Default::default()
            }
        );

        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            "~/.claude/.fleet-assets.json".into(),
            json!("not an object, not a manifest"),
        );
        let m = Manifest::from_snapshot(&snap, "~/.claude/.fleet-assets.json");
        assert_eq!(
            m,
            Manifest {
                version: 1,
                ..Default::default()
            }
        );

        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            "~/.claude/.fleet-assets.json".into(),
            json!({"version": 1, "updated_at": 5, "assets": {}}),
        );
        let m = Manifest::from_snapshot(&snap, "~/.claude/.fleet-assets.json");
        assert_eq!(m.updated_at, 5);
    }

    #[test]
    fn entry_for_records_files_and_merge_hashes() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/worktree/SKILL.md".into(),
            bytes: b"body".to_vec(),
        });
        let value = json!({"token": "resolved-secret-value"});
        plan.merges.push(ConfigMerge {
            file: "~/.claude/settings.json".into(),
            json_path: vec!["mcpServers".into(), "fleet".into()],
            mode: MergeMode::Set,
            value: value.clone(),
        });
        let entry = Manifest::entry_for("planhash", &plan, 123);
        assert_eq!(entry.hash, "planhash");
        assert_eq!(
            entry.files,
            vec!["~/.claude/skills/worktree/SKILL.md".to_string()]
        );
        assert_eq!(entry.synced_at, 123);
        assert_eq!(entry.merges.len(), 1);
        assert_eq!(entry.merges[0].value_hash, value_hash(&value));
        // The hash must reflect the substituted value, not some other one.
        assert_ne!(
            entry.merges[0].value_hash,
            value_hash(&json!({"token": "${SECRET}"}))
        );
    }

    #[test]
    fn orphans_reports_keys_missing_from_the_catalog() {
        let catalog = Catalog {
            assets: vec![skill("kept")],
            ..Default::default()
        };
        let mut m = Manifest::default();
        m.assets
            .insert(Manifest::key(Kind::Skill, "kept"), ManifestEntry::default());
        m.assets.insert(
            Manifest::key(Kind::Skill, "removed"),
            ManifestEntry::default(),
        );
        m.assets
            .insert("garbage-key".to_string(), ManifestEntry::default());
        let orphans = m.orphans(&catalog);
        let keys: Vec<&str> = orphans.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec!["garbage-key", "skill/removed"]);
    }
}
