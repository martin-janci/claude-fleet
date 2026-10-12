//! The managed manifest: a JSON file the sync engine writes on the host
//! (one per harness, at `Harness::manifest_path()`) recording exactly which
//! files and config merges fleet put there for each catalog asset, so a
//! later sync can update or remove precisely what an earlier one added
//! without disturbing anything else on the host.
//!
//! Read by `inventory::compute_states` (to tell a managed asset from one
//! that was merely found installed) and by `sync::plan`; written by
//! `sync::apply` at the end of a successful host sync.

use super::super::harness::{
    json_get, value_hash, HostSnapshot, ManifestMerge, MergeMode, RenderPlan,
};
use super::super::model::{sha256_hex, Kind};
use super::super::repo::Catalog;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What fleet wrote on the host for one catalog asset: which files, which
/// config merges (as `ManifestMerge`, so the value itself is never kept
/// around — only its hash), the overall `RenderPlan` hash it came from, and
/// when it was last synced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub hash: String,
    pub files: Vec<String>,
    pub merges: Vec<ManifestMerge>,
    pub synced_at: i64,
    /// Which catalog this asset was applied from (Assets M2). An old
    /// manifest written before this field existed has none on disk, so it
    /// reads as `"personal"` — every host synced before Assets M2 could only
    /// ever have gotten its assets from the personal catalog.
    #[serde(default = "personal")]
    pub catalog: String,
    /// Assets M5 (Rulings R1): path → sha256 of the bytes this sync wrote
    /// there — the same hash a scan reports per file — so a later plan can
    /// tell a copy that is exactly as fleet left it from one a person
    /// edited. Absent on an entry written before M5, which then cannot
    /// vouch for its copy ([`HostCopy::Unverified`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_hashes: BTreeMap<String, String>,
}

fn personal() -> String {
    "personal".into()
}

/// Manual, not derived: `catalog` defaults to `"personal"`, consistent with
/// the field's own `#[serde(default = "personal")]` rather than the
/// `String`-derived `""` a `#[derive(Default)]` would give it. A test (or
/// any other caller) building an entry with `..Default::default()` gets the
/// same catalog a deserialised pre-Assets-M2 manifest would.
impl Default for ManifestEntry {
    fn default() -> Self {
        ManifestEntry {
            hash: String::new(),
            files: Vec::new(),
            merges: Vec::new(),
            synced_at: 0,
            catalog: personal(),
            file_hashes: BTreeMap::new(),
        }
    }
}

/// Assets M5 (Rulings R1): whether a managed asset's copy on the host is
/// still what fleet wrote there — read from the manifest entry's recorded
/// hashes against a scan, so it needs no bytes from the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCopy {
    /// Every location the entry records is exactly what fleet wrote.
    Unchanged,
    /// A recorded file or merge differs from what fleet wrote, or is gone:
    /// someone edited the host copy.
    Edited,
    /// The entry cannot say: written before M5 (no file hashes), a `Subset`
    /// merge (never hashed whole), or nothing recorded at all.
    Unverified,
}

impl ManifestEntry {
    /// [`HostCopy`] of this entry's asset on `snap`, read against `plan`,
    /// the render about to be written (Assets M5, fix round 1). On top of
    /// [`Self::host_copy`], a location the render writes that the entry
    /// never recorded, and that the host already holds with something else,
    /// is content fleet never wrote — the copy is `Edited`, however
    /// untouched the recorded locations are:
    ///
    /// - a planned file at a path the entry does not list, whose host hash
    ///   is not the render's;
    /// - a planned `Set` at a `(file, json_path)` the entry does not list,
    ///   whose host value is not the planned one;
    /// - a planned `AppendUnique` there whose host value is not an array:
    ///   applying it replaces that value. Over an array it only appends, so
    ///   every element a person put there stays and nothing is foreign.
    ///
    /// A location the host does not hold yet is new, not foreign. An entry
    /// that records no location at all cannot say which locations are new
    /// either: it stays `Unverified`, as [`Self::host_copy`] reads it.
    pub fn host_copy_for(&self, snap: &HostSnapshot, plan: &RenderPlan) -> HostCopy {
        let records_any = !self.files.is_empty() || !self.merges.is_empty();
        match self.host_copy(snap) {
            HostCopy::Edited => HostCopy::Edited,
            _ if records_any && self.holds_foreign_copy(snap, plan) => HostCopy::Edited,
            recorded => recorded,
        }
    }

    /// Whether `snap` holds, at a location `plan` writes and this entry
    /// never recorded, something other than what `plan` would write there
    /// (see [`Self::host_copy_for`]).
    fn holds_foreign_copy(&self, snap: &HostSnapshot, plan: &RenderPlan) -> bool {
        let file = plan.files.iter().any(|f| {
            !self.files.contains(&f.path)
                && snap
                    .files
                    .get(&f.path)
                    .is_some_and(|h| *h != sha256_hex(&f.bytes))
        });
        file || plan.merges.iter().any(|m| {
            let recorded = self
                .merges
                .iter()
                .any(|e| e.file == m.file && e.json_path == m.json_path);
            let have = snap
                .configs
                .get(&m.file)
                .and_then(|root| json_get(root, &m.json_path));
            !recorded
                && have.is_some_and(|v| match m.mode {
                    MergeMode::Set => *v != m.value,
                    MergeMode::AppendUnique => !v.is_array(),
                    // A plugin's entry; plugins never reach rule 4.
                    MergeMode::Subset => false,
                })
        })
    }

    /// [`HostCopy`] of this entry's asset on `snap`. Any differing or
    /// missing location is `Edited`, even when another one is unverifiable.
    pub fn host_copy(&self, snap: &HostSnapshot) -> HostCopy {
        let mut unverified = self.files.is_empty() && self.merges.is_empty();
        for path in &self.files {
            let Some(want) = self.file_hashes.get(path) else {
                unverified = true;
                continue;
            };
            if snap.files.get(path) != Some(want) {
                return HostCopy::Edited;
            }
        }
        for m in &self.merges {
            let have = snap
                .configs
                .get(&m.file)
                .and_then(|root| json_get(root, &m.json_path));
            let same = match m.mode {
                MergeMode::Set => have.is_some_and(|v| value_hash(v) == m.value_hash),
                MergeMode::AppendUnique => have
                    .and_then(|v| v.as_array())
                    .is_some_and(|arr| arr.iter().any(|e| value_hash(e) == m.value_hash)),
                MergeMode::Subset => {
                    unverified = true;
                    true
                }
            };
            if !same {
                return HostCopy::Edited;
            }
        }
        if unverified {
            HostCopy::Unverified
        } else {
            HostCopy::Unchanged
        }
    }
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
    /// the paths it wrote, `merges` hash the (already-substituted) merge
    /// values rather than keeping them, and `catalog` is the name of the
    /// catalog the applied action's asset came from (`Action::catalog`).
    pub fn entry_for(hash: &str, plan: &RenderPlan, now: i64, catalog: &str) -> ManifestEntry {
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
            catalog: catalog.to_string(),
            file_hashes: plan
                .files
                .iter()
                .map(|f| (f.path.clone(), sha256_hex(&f.bytes)))
                .collect(),
        }
    }

    /// Manifest entries whose `(kind, name)` is no longer in `catalog` — the
    /// asset was removed or renamed — and whose own catalog speaks for this
    /// host (`speaks_for`; `None` = every catalog does, Rulings R8). Spec: an
    /// orphan is "a manifest entry whose catalog no longer has it". These
    /// are candidates for `remove_merges` / deletion the next time this host
    /// is synced.
    pub fn orphans<'a>(
        &'a self,
        catalog: &Catalog,
        speaks_for: Option<&BTreeSet<String>>,
    ) -> Vec<(&'a str, &'a ManifestEntry)> {
        self.assets
            .iter()
            .filter(|(key, _)| match Self::split_key(key) {
                Some((kind, name)) => catalog.find(kind, &name).is_none(),
                None => true,
            })
            .filter(|(_, entry)| speaks_for.is_none_or(|s| s.contains(&entry.catalog)))
            .map(|(key, entry)| (key.as_str(), entry))
            .collect()
    }

    /// Entries absent from `catalog` whose catalog does NOT speak for this
    /// host: nobody here can say it dropped them, so they are kept (R6).
    /// Unparseable keys are `orphans`' business, never listed here.
    pub fn held<'a>(
        &'a self,
        catalog: &Catalog,
        speaks_for: &BTreeSet<String>,
    ) -> Vec<(&'a str, &'a ManifestEntry)> {
        self.assets
            .iter()
            .filter(|(key, entry)| {
                !speaks_for.contains(&entry.catalog)
                    && Self::split_key(key)
                        .is_some_and(|(kind, name)| catalog.find(kind, &name).is_none())
            })
            .map(|(key, entry)| (key.as_str(), entry))
            .collect()
    }

    /// Remove every `assets` key `split_key` cannot parse (e.g. a key from
    /// a future/foreign schema, or plain corruption) and return the removed
    /// keys, sorted. `orphans` skips these rather than reporting them, so
    /// without this they persist in the manifest forever; called right
    /// before the manifest is rewritten to the host. Deliberately not used
    /// by `from_snapshot`'s lenient parse, which must stay tolerant of
    /// anything it cannot make sense of.
    pub fn drop_unparseable(&mut self) -> Vec<String> {
        let dropped: Vec<String> = self
            .assets
            .keys()
            .filter(|key| Self::split_key(key).is_none())
            .cloned()
            .collect();
        for key in &dropped {
            self.assets.remove(key);
        }
        dropped
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
                catalog: "personal".into(),
                // Assets M5: the recorded file hashes round-trip too.
                file_hashes: BTreeMap::from([(
                    "~/.claude/skills/worktree/SKILL.md".to_string(),
                    "f00d".to_string(),
                )]),
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
        let entry = Manifest::entry_for("planhash", &plan, 123, "personal");
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

    /// `entry_for` records whichever catalog name it is given — the applier
    /// passes `Action::catalog` (Assets M2), so a plugin/asset applied from
    /// an org catalog is recorded as such rather than silently as
    /// `"personal"`.
    #[test]
    fn entry_for_records_the_catalog_it_is_given() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"body".to_vec(),
        });
        let entry = Manifest::entry_for("h", &plan, 1, "acme");
        assert_eq!(entry.catalog, "acme");
    }

    /// An old manifest, written before Assets M2 added `catalog` to the
    /// entry, has no such field on disk. It must still deserialise —
    /// every host synced before Assets M2 could only ever have gotten its
    /// assets from the personal catalog, so that is what it reads as.
    #[test]
    fn an_entry_with_no_catalog_field_deserialises_as_personal() {
        let entry: ManifestEntry =
            serde_json::from_str(r#"{"hash":"h","files":[],"merges":[],"synced_at":1}"#).unwrap();
        assert_eq!(entry.catalog, "personal");
    }

    /// `Default` is implemented by hand, not derived: a `String`-derived
    /// default would give `catalog: ""`, inconsistent with the field's own
    /// `#[serde(default = "personal")]` — a caller building an entry with
    /// `..Default::default()` (as plenty of tests across the sync module
    /// do) must see the same `"personal"` a deserialised pre-Assets-M2
    /// manifest reads as.
    #[test]
    fn default_catalog_matches_the_serde_default() {
        assert_eq!(ManifestEntry::default().catalog, "personal");
    }

    #[test]
    fn drop_unparseable_removes_keys_split_key_cannot_parse() {
        let mut m = Manifest::default();
        m.assets
            .insert("skill/a".to_string(), ManifestEntry::default());
        m.assets
            .insert("bogus/b".to_string(), ManifestEntry::default());
        m.assets
            .insert("no-slash".to_string(), ManifestEntry::default());
        let dropped = m.drop_unparseable();
        assert_eq!(dropped, vec!["bogus/b".to_string(), "no-slash".to_string()]);
        assert_eq!(m.assets.keys().collect::<Vec<_>>(), vec!["skill/a"]);
    }

    #[test]
    fn drop_unparseable_is_a_noop_when_everything_parses() {
        let mut m = Manifest::default();
        m.assets
            .insert("skill/a".to_string(), ManifestEntry::default());
        assert!(m.drop_unparseable().is_empty());
        assert_eq!(m.assets.len(), 1);
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
        let orphans = m.orphans(&catalog, None);
        let keys: Vec<&str> = orphans.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec!["garbage-key", "skill/removed"]);
    }

    /// M2 carry 3: an orphan is an entry whose OWN catalog no longer has it;
    /// an entry of a catalog that does not speak for this host is `held`.
    #[test]
    fn orphans_and_held_split_on_the_entrys_catalog() {
        let catalog = Catalog {
            assets: vec![skill("kept")],
            ..Default::default()
        };
        let mut m = Manifest::default();
        m.assets
            .insert(Manifest::key(Kind::Skill, "kept"), ManifestEntry::default());
        m.assets
            .insert(Manifest::key(Kind::Skill, "mine"), ManifestEntry::default());
        let theirs = ManifestEntry {
            catalog: "acme".into(),
            ..Default::default()
        };
        m.assets
            .insert(Manifest::key(Kind::Skill, "theirs"), theirs.clone());
        // Still in the catalog (another catalog supplies it now): neither.
        m.assets
            .insert(Manifest::key(Kind::Skill, "kept-theirs"), theirs.clone());
        // Unparseable: never `held` (nothing to name it as).
        m.assets.insert("garbage-key".to_string(), theirs);
        let catalog = Catalog {
            assets: vec![skill("kept"), skill("kept-theirs")],
            ..catalog
        };
        let speaks = std::collections::BTreeSet::from(["personal".to_string()]);
        let keys = |v: Vec<(&str, &ManifestEntry)>| -> Vec<String> {
            v.into_iter().map(|(k, _)| k.to_string()).collect()
        };
        assert_eq!(keys(m.orphans(&catalog, Some(&speaks))), vec!["skill/mine"]);
        assert_eq!(keys(m.held(&catalog, &speaks)), vec!["skill/theirs"]);
        assert_eq!(
            keys(m.orphans(&catalog, None)),
            vec!["garbage-key", "skill/mine", "skill/theirs"],
            "None: every catalog speaks (inventory, R8)"
        );
    }

    use crate::service::catalog::model::sha256_hex;

    /// Assets M5 (R1): an entry records the hash of every file it wrote, so
    /// a later scan tells "as fleet left it" from "a person edited it".
    #[test]
    fn host_copy_tells_an_untouched_copy_from_an_edited_one() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"body".to_vec(),
        });
        let hook = json!({"type": "command", "command": "x"});
        plan.merges.push(ConfigMerge {
            file: "~/.claude/settings.json".into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value: hook.clone(),
        });
        let entry = Manifest::entry_for("h", &plan, 1, "personal");
        assert_eq!(
            entry.file_hashes.get("~/.claude/skills/s/SKILL.md"),
            Some(&sha256_hex(b"body"))
        );

        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), sha256_hex(b"body"));
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [{"type": "command", "command": "other"}, hook]}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Unchanged);

        let mut edited = snap.clone();
        edited
            .files
            .insert("~/.claude/skills/s/SKILL.md".into(), sha256_hex(b"edited"));
        assert_eq!(entry.host_copy(&edited), HostCopy::Edited);

        let mut gone = snap.clone();
        gone.files.remove("~/.claude/skills/s/SKILL.md");
        assert_eq!(
            entry.host_copy(&gone),
            HostCopy::Edited,
            "a deleted file is an edit"
        );

        let mut hook_changed = snap.clone();
        hook_changed.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [{"type": "command", "command": "y"}]}}),
        );
        assert_eq!(entry.host_copy(&hook_changed), HostCopy::Edited);
    }

    /// R1: an entry written before M5 has no file hashes — it cannot vouch
    /// for the host copy either way, and it still reads and writes as before.
    #[test]
    fn an_entry_written_before_m5_is_unverified_not_edited() {
        let entry: ManifestEntry = serde_json::from_str(
            r#"{"hash":"h","files":["~/.claude/skills/s/SKILL.md"],"merges":[],"synced_at":1}"#,
        )
        .unwrap();
        assert!(entry.file_hashes.is_empty());
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), "anything".into());
        assert_eq!(entry.host_copy(&snap), HostCopy::Unverified);
        assert_eq!(
            ManifestEntry::default().host_copy(&snap),
            HostCopy::Unverified,
            "an entry that records nothing vouches for nothing"
        );
        assert!(!serde_json::to_string(&entry)
            .unwrap()
            .contains("file_hashes"));
    }

    /// R1: a `Set` merge is checked by its value hash; a `Subset` merge
    /// (a plugin's entry) is never hashed whole, so it cannot vouch.
    #[test]
    fn host_copy_reads_set_merges_and_cannot_read_subset_ones() {
        let value = json!({"url": "https://example/mcp"});
        let entry = ManifestEntry {
            merges: vec![ManifestMerge {
                file: "~/.claude.json".into(),
                json_path: vec!["mcpServers".into(), "fleet".into()],
                mode: MergeMode::Set,
                value_hash: value_hash(&value),
            }],
            ..Default::default()
        };
        let mut snap = HostSnapshot::default();
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet": {"url": "https://example/mcp"}}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Unchanged);
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet": {"url": "https://elsewhere/mcp"}}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Edited);

        let subset = ManifestEntry {
            merges: vec![ManifestMerge {
                file: "~/.claude/plugins/installed_plugins.json".into(),
                json_path: vec!["plugins".into(), "sp@mk".into()],
                mode: MergeMode::Subset,
                value_hash: "h".into(),
            }],
            ..Default::default()
        };
        assert_eq!(subset.host_copy(&snap), HostCopy::Unverified);
    }

    /// Fix round 1 (coverage): an entry with some file hashes but not all
    /// cannot vouch for the unhashed file — the copy is unverified.
    #[test]
    fn a_partly_hashed_entry_is_unverified() {
        let entry = ManifestEntry {
            files: vec!["~/a".into(), "~/b".into()],
            file_hashes: BTreeMap::from([("~/a".to_string(), sha256_hex(b"a"))]),
            ..Default::default()
        };
        let mut snap = HostSnapshot::default();
        snap.files.insert("~/a".into(), sha256_hex(b"a"));
        snap.files.insert("~/b".into(), sha256_hex(b"anything"));
        assert_eq!(entry.host_copy(&snap), HostCopy::Unverified);
    }

    /// Fix round 1 (coverage): one location proven edited makes the copy
    /// edited, whatever another location cannot vouch for — in either order.
    #[test]
    fn edited_beats_unverified_in_one_entry() {
        let entry = ManifestEntry {
            files: vec!["~/unhashed".into(), "~/hashed".into()],
            file_hashes: BTreeMap::from([("~/hashed".to_string(), sha256_hex(b"fleet"))]),
            merges: vec![ManifestMerge {
                file: "~/.claude/plugins/installed_plugins.json".into(),
                json_path: vec!["plugins".into(), "sp@mk".into()],
                mode: MergeMode::Subset,
                value_hash: "h".into(),
            }],
            ..Default::default()
        };
        let mut snap = HostSnapshot::default();
        snap.files.insert("~/unhashed".into(), "x".into());
        snap.files.insert("~/hashed".into(), sha256_hex(b"person"));
        assert_eq!(entry.host_copy(&snap), HostCopy::Edited);
    }

    /// Fix round 1 (coverage): a recorded `AppendUnique` whose array is gone,
    /// or is no longer an array, was edited on the host.
    #[test]
    fn an_append_unique_target_missing_or_not_an_array_is_edited() {
        let hook = json!({"type": "command", "command": "x"});
        let entry = ManifestEntry {
            merges: vec![ManifestMerge {
                file: "~/.claude/settings.json".into(),
                json_path: vec!["hooks".into(), "Stop".into()],
                mode: MergeMode::AppendUnique,
                value_hash: value_hash(&hook),
            }],
            ..Default::default()
        };
        let mut snap = HostSnapshot::default();
        snap.configs
            .insert("~/.claude/settings.json".into(), json!({"hooks": {}}));
        assert_eq!(entry.host_copy(&snap), HostCopy::Edited, "missing");
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": hook.clone()}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Edited, "not an array");
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [hook]}}),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Unchanged);
    }

    /// Fix round 1 (Critical): read against the render about to be written,
    /// a planned file the entry never recorded that the host already holds
    /// with other bytes is content fleet never wrote — the copy is edited,
    /// however untouched the recorded files are.
    #[test]
    fn a_foreign_file_where_the_render_adds_one_is_an_edit() {
        let v1 = plan_with_files(&[("~/.claude/skills/s/SKILL.md", b"v1")]);
        let entry = Manifest::entry_for("h1", &v1, 1, "personal");
        let v2 = plan_with_files(&[
            ("~/.claude/skills/s/SKILL.md", b"v2"),
            ("~/.claude/skills/s/scripts/go.sh", b"fleet's\n"),
        ]);
        let mut snap = HostSnapshot::default();
        snap.files
            .insert("~/.claude/skills/s/SKILL.md".into(), sha256_hex(b"v1"));
        assert_eq!(entry.host_copy(&snap), HostCopy::Unchanged);
        assert_eq!(
            entry.host_copy_for(&snap, &v2),
            HostCopy::Unchanged,
            "the new file is not on the host yet"
        );

        snap.files.insert(
            "~/.claude/skills/s/scripts/go.sh".into(),
            sha256_hex(b"fleet's\n"),
        );
        assert_eq!(
            entry.host_copy_for(&snap, &v2),
            HostCopy::Unchanged,
            "already holds exactly what fleet would write"
        );

        snap.files.insert(
            "~/.claude/skills/s/scripts/go.sh".into(),
            sha256_hex(b"the person's\n"),
        );
        assert_eq!(entry.host_copy(&snap), HostCopy::Unchanged);
        assert_eq!(entry.host_copy_for(&snap, &v2), HostCopy::Edited);

        // A pre-M5 entry cannot vouch for its own file, but the foreign one
        // is still proof of an edit.
        let mut old = entry.clone();
        old.file_hashes.clear();
        assert_eq!(old.host_copy_for(&snap, &v2), HostCopy::Edited);

        // An entry that records nothing cannot tell new locations from its
        // own: every one is "unlisted", so it vouches for nothing.
        assert_eq!(
            ManifestEntry::default().host_copy_for(&snap, &v2),
            HostCopy::Unverified
        );
    }

    /// Fix round 1 (Critical, merge variant): a `Set` the entry never
    /// recorded, at a location the host already holds with another value,
    /// is an edit. An `AppendUnique` there replaces nothing when the host
    /// holds an array (it only appends), but replaces whatever else it
    /// finds, so a non-array there is an edit too.
    #[test]
    fn a_foreign_value_where_the_render_adds_a_merge_is_an_edit() {
        let entry = ManifestEntry {
            files: vec!["~/f".into()],
            file_hashes: BTreeMap::from([("~/f".to_string(), sha256_hex(b"f"))]),
            ..Default::default()
        };
        let mut snap = HostSnapshot::default();
        snap.files.insert("~/f".into(), sha256_hex(b"f"));
        let set = |value: serde_json::Value| RenderPlan {
            merges: vec![ConfigMerge {
                file: "~/.claude.json".into(),
                json_path: vec!["mcpServers".into(), "fleet2".into()],
                mode: MergeMode::Set,
                value,
            }],
            ..Default::default()
        };
        let mine = json!({"url": "https://example/mcp"});
        assert_eq!(
            entry.host_copy_for(&snap, &set(mine.clone())),
            HostCopy::Unchanged
        );
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet2": mine.clone()}}),
        );
        assert_eq!(
            entry.host_copy_for(&snap, &set(mine.clone())),
            HostCopy::Unchanged
        );
        snap.configs.insert(
            "~/.claude.json".into(),
            json!({"mcpServers": {"fleet2": {"url": "https://person/mcp"}}}),
        );
        assert_eq!(entry.host_copy_for(&snap, &set(mine)), HostCopy::Edited);

        let hook = json!({"type": "command", "command": "x"});
        let append = RenderPlan {
            merges: vec![ConfigMerge {
                file: "~/.claude/settings.json".into(),
                json_path: vec!["hooks".into(), "Stop".into()],
                mode: MergeMode::AppendUnique,
                value: hook,
            }],
            ..Default::default()
        };
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": [{"type": "command", "command": "theirs"}]}}),
        );
        assert_eq!(
            entry.host_copy_for(&snap, &append),
            HostCopy::Unchanged,
            "appending to a person's array keeps every element"
        );
        snap.configs.insert(
            "~/.claude/settings.json".into(),
            json!({"hooks": {"Stop": "a person's value"}}),
        );
        assert_eq!(entry.host_copy_for(&snap, &append), HostCopy::Edited);
    }

    fn plan_with_files(files: &[(&str, &[u8])]) -> RenderPlan {
        RenderPlan {
            files: files
                .iter()
                .map(|(path, bytes)| FileWrite {
                    path: (*path).into(),
                    bytes: bytes.to_vec(),
                })
                .collect(),
            ..Default::default()
        }
    }
}
