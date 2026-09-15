//! Per-harness renderers. A harness turns an IR asset into a `RenderPlan`
//! (files to write + JSON merges into config files), knows how to scan a
//! host for what is installed, and can list the assets it finds there.

pub mod claude;
pub mod codex;

use super::model::{find_placeholders, sha256_hex, Asset, Kind};
use crate::ipc_error::IpcError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Reserved for the sync engine (enumerating supported harness ids without
/// going through `all()`); not yet called from a non-test build.
#[allow(dead_code)]
pub const HARNESS_IDS: &[&str] = &["claude", "codex"];

/// A file the harness would write on the host. `path` uses `~/` for the home dir.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileWrite {
    pub path: String,
    #[serde(serialize_with = "ser_utf8_or_b64")]
    pub bytes: Vec<u8>,
}

fn ser_utf8_or_b64<S: serde::Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
    use base64::Engine;
    match std::str::from_utf8(b) {
        Ok(t) => s.serialize_str(t),
        Err(_) => s.serialize_str(&format!(
            "base64:{}",
            base64::engine::general_purpose::STANDARD.encode(b)
        )),
    }
}

/// How a `ConfigMerge` value relates to what is on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// The value at `json_path` must equal `value`.
    Set,
    /// `json_path` is an array that must contain an element equal to `value`.
    AppendUnique,
    /// The object at `json_path` must contain every key of `value` with an equal value.
    Subset,
}

/// A structured edit of a JSON config file on the host.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigMerge {
    pub file: String,
    pub json_path: Vec<String>,
    pub mode: MergeMode,
    pub value: serde_json::Value,
}

/// A record of one merge fleet already applied to a harness's manifest, kept
/// so it can be undone later (an asset removed from the catalog, or a target
/// disabled) without disturbing edits the merge never made. `value_hash`
/// identifies the exact value that was merged in, without keeping the value
/// itself around (`AppendUnique` uses it to find the element to remove
/// again; `Set`/`Subset` just drop the whole key at `json_path`).
///
/// Reserved for the sync engine (Task 3+ persists these alongside the
/// manifest file and replays them through `remove_merges`); not yet
/// constructed from a non-test build.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestMerge {
    pub file: String,
    pub json_path: Vec<String>,
    pub mode: MergeMode,
    pub value_hash: String,
}

/// Sha256 of `v`'s canonical (key-sorted, since `serde_json::Value` is
/// BTreeMap-backed) `to_string()`.
///
/// Reserved for the sync engine (populating `ManifestMerge::value_hash`);
/// not yet called from a non-test build.
#[allow(dead_code)]
pub fn value_hash(v: &Value) -> String {
    sha256_hex(serde_json::to_string(v).unwrap_or_default().as_bytes())
}

/// Everything a harness would do to install one asset.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RenderPlan {
    pub files: Vec<FileWrite>,
    pub merges: Vec<ConfigMerge>,
    pub placeholders: Vec<String>,
    pub warnings: Vec<String>,
}

impl RenderPlan {
    /// Stable content hash: sorted files (path + bytes) then sorted merges
    /// (file + path + canonical JSON). Key order inside JSON does not matter
    /// because `serde_json::Value` objects are BTreeMap-backed when the
    /// `preserve_order` feature is off (it is off in this crate).
    pub fn hash(&self) -> String {
        let mut files: Vec<&FileWrite> = self.files.iter().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut merges: Vec<String> = self
            .merges
            .iter()
            .map(|m| {
                format!(
                    "{}\u{0}{}\u{0}{:?}\u{0}{}",
                    m.file,
                    m.json_path.join("/"),
                    m.mode,
                    serde_json::to_string(&m.value).unwrap_or_default()
                )
            })
            .collect();
        merges.sort();
        let mut buf: Vec<u8> = Vec::new();
        for f in files {
            buf.extend_from_slice(b"F");
            buf.extend_from_slice(f.path.as_bytes());
            buf.push(0);
            buf.extend_from_slice(&f.bytes);
            buf.push(0);
        }
        for m in merges {
            buf.extend_from_slice(b"M");
            buf.extend_from_slice(m.as_bytes());
            buf.push(0);
        }
        sha256_hex(&buf)
    }

    /// Record any `${NAME}` placeholders found in `text`.
    pub fn note_placeholders(&mut self, text: &str) {
        for p in find_placeholders(text) {
            if !self.placeholders.contains(&p) {
                self.placeholders.push(p);
            }
        }
    }
}

/// What a host scan found: file hashes keyed by `~/`-relative path, and parsed
/// JSON config files keyed the same way.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HostSnapshot {
    pub files: BTreeMap<String, String>,
    pub configs: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unsupported {
    pub harness: &'static str,
    pub kind: Kind,
}

impl Unsupported {
    pub fn into_ipc(self) -> IpcError {
        IpcError::new(
            super::E_ASSET_UNSUPPORTED,
            format!(
                "{} cannot render {} assets",
                self.harness,
                self.kind.as_str()
            ),
        )
    }
}

pub trait Harness: Send + Sync {
    fn id(&self) -> &'static str;
    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported>;
    /// Bash script that prints the host snapshot; `None` = no scanning.
    fn scan_script(&self) -> Option<String>;
    fn parse_scan(&self, stdout: &str) -> Result<HostSnapshot, IpcError>;
    /// Every asset (kind, name) the snapshot shows as installed.
    fn installed(&self, snap: &HostSnapshot) -> Vec<(Kind, String)>;
    /// Home-relative path (`~/...`) to the manifest file this harness uses
    /// to track which merges/files the sync engine applied.
    ///
    /// Reserved for the sync engine; not yet called from a non-test build.
    #[allow(dead_code)]
    fn manifest_path(&self) -> &'static str;
    /// Apply `merges` then `remove` to `existing`'s parsed content and
    /// return the full new file text (with a trailing newline). An empty
    /// `existing` means an empty document.
    ///
    /// Reserved for the sync engine; not yet called from a non-test build.
    #[allow(dead_code)]
    fn merge_config(
        &self,
        file: &str,
        existing: &str,
        merges: &[ConfigMerge],
        remove: &[ManifestMerge],
    ) -> Result<String, IpcError>;
}

pub fn all() -> Vec<Box<dyn Harness>> {
    vec![Box::new(claude::Claude), Box::new(codex::Codex)]
}

/// Reserved for the sync engine (looking up one harness by id without
/// building the whole registry via `all()`); not yet called from a
/// non-test build.
#[allow(dead_code)]
pub fn by_id(id: &str) -> Option<Box<dyn Harness>> {
    match id {
        "claude" => Some(Box::new(claude::Claude)),
        "codex" => Some(Box::new(codex::Codex)),
        _ => None,
    }
}

/// Walk `path` through nested JSON objects.
pub fn json_get<'a>(root: &'a serde_json::Value, path: &[String]) -> Option<&'a serde_json::Value> {
    let mut cur = root;
    for key in path {
        cur = cur.as_object()?.get(key)?;
    }
    Some(cur)
}

/// Mutable counterpart of `json_get`: walks `path` through nested JSON
/// objects without creating anything, `None` if any segment is missing or
/// not an object.
///
/// Reserved for the sync engine (`remove_merges`'s `AppendUnique` case); not
/// yet called from a non-test build.
#[allow(dead_code)]
fn json_get_mut<'a>(root: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut cur = root;
    for key in path {
        cur = cur.as_object_mut()?.get_mut(key)?;
    }
    Some(cur)
}

/// Does `have` satisfy `want`? Mirrors `inventory::is_subset`'s semantics
/// exactly (an object is a subset when every key of `want` is present in
/// `have` with a subset value; an array is a subset when every element of
/// `want` has some element of `have` that is a superset of it). Duplicated
/// here rather than shared, because `inventory` depends on `harness` and
/// sharing the other way would be a cycle.
///
/// Reserved for the sync engine (`apply_merges`'s `Subset` case); not yet
/// called from a non-test build.
#[allow(dead_code)]
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

/// Navigate `path` through nested JSON objects from `root`, creating
/// missing intermediate objects (and replacing anything in the way that
/// isn't an object), and return a mutable reference to the value at `path`
/// — inserting the result of `default` there first if it is not already
/// present.
///
/// Reserved for the sync engine (`apply_merges`); not yet called from a
/// non-test build.
#[allow(dead_code)]
fn ensure_at<'a>(
    root: &'a mut Value,
    path: &[String],
    default: impl FnOnce() -> Value,
) -> &'a mut Value {
    if path.is_empty() {
        return root;
    }
    let mut default = Some(default);
    let mut cur = root;
    let last = path.len() - 1;
    for (i, key) in path.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let map = cur.as_object_mut().expect("just ensured object");
        cur = if i == last {
            let d = default
                .take()
                .expect("consumed exactly once, on the last segment");
            map.entry(key.clone()).or_insert_with(d)
        } else {
            map.entry(key.clone())
                .or_insert_with(|| Value::Object(Map::new()))
        };
    }
    cur
}

/// Apply structured config edits in place. See `MergeMode` for what each
/// mode means; `AppendUnique` and `Subset` are idempotent (re-applying an
/// already-satisfied merge is a no-op).
///
/// Reserved for the sync engine (`Harness::merge_config`); not yet called
/// from a non-test build.
#[allow(dead_code)]
pub fn apply_merges(root: &mut Value, merges: &[ConfigMerge]) {
    for m in merges {
        match m.mode {
            MergeMode::Set => {
                let slot = ensure_at(root, &m.json_path, || Value::Null);
                *slot = m.value.clone();
            }
            MergeMode::AppendUnique => {
                let slot = ensure_at(root, &m.json_path, || Value::Array(Vec::new()));
                if !slot.is_array() {
                    *slot = Value::Array(Vec::new());
                }
                let arr = slot.as_array_mut().expect("just ensured array");
                if !arr.contains(&m.value) {
                    arr.push(m.value.clone());
                }
            }
            MergeMode::Subset => match &m.value {
                Value::Array(items) => {
                    let Some(first) = items.first() else {
                        continue;
                    };
                    let slot = ensure_at(root, &m.json_path, || Value::Array(Vec::new()));
                    if !slot.is_array() {
                        *slot = Value::Array(Vec::new());
                    }
                    let arr = slot.as_array_mut().expect("just ensured array");
                    let covered = arr.iter().any(|hv| is_subset(first, hv));
                    if !covered {
                        arr.push(first.clone());
                    }
                }
                Value::Object(obj) => {
                    let slot = ensure_at(root, &m.json_path, || Value::Object(Map::new()));
                    if !slot.is_object() {
                        *slot = Value::Object(Map::new());
                    }
                    let map = slot.as_object_mut().expect("just ensured object");
                    for (k, v) in obj {
                        map.insert(k.clone(), v.clone());
                    }
                }
                _ => {}
            },
        }
    }
}

/// Delete the value at `path` from `root`, then prune any ancestor object
/// that becomes empty as a result — stopping before the root-level (first)
/// path segment, which is always kept even once it becomes `{}`.
///
/// Reserved for the sync engine (`remove_merges`); not yet called from a
/// non-test build.
#[allow(dead_code)]
fn remove_key_path(root: &mut Value, path: &[String]) {
    // Returns whether `cur` is now empty (so the caller may prune it too).
    fn prune(cur: &mut Value, path: &[String]) -> bool {
        let Some(obj) = cur.as_object_mut() else {
            return false;
        };
        if path.len() == 1 {
            obj.remove(&path[0]);
        } else if let Some(child) = obj.get_mut(&path[0]) {
            if prune(child, &path[1..]) {
                obj.remove(&path[0]);
            }
        }
        obj.is_empty()
    }
    match path.len() {
        0 => {}
        1 => {
            if let Some(obj) = root.as_object_mut() {
                obj.remove(&path[0]);
            }
        }
        _ => {
            if let Some(obj) = root.as_object_mut() {
                if let Some(child) = obj.get_mut(&path[0]) {
                    // Discard the "did it become empty" result: the
                    // root-level segment is never pruned, however empty.
                    prune(child, &path[1..]);
                }
            }
        }
    }
}

/// Undo previously applied merges. See `ManifestMerge` for the mapping from
/// `MergeMode` to how removal works.
///
/// Reserved for the sync engine (`Harness::merge_config`); not yet called
/// from a non-test build.
#[allow(dead_code)]
pub fn remove_merges(root: &mut Value, merges: &[ManifestMerge]) {
    for m in merges {
        if m.json_path.is_empty() {
            continue;
        }
        match m.mode {
            MergeMode::Set | MergeMode::Subset => remove_key_path(root, &m.json_path),
            MergeMode::AppendUnique => {
                if let Some(arr) = json_get_mut(root, &m.json_path).and_then(Value::as_array_mut) {
                    arr.retain(|v| value_hash(v) != m.value_hash);
                    if arr.is_empty() {
                        remove_key_path(root, &m.json_path);
                    }
                }
            }
        }
    }
}

/// Generalised `parse_scan`: `##HASHES` lines, then `##CONFIG <path>` blocks
/// whose base64 body is decoded by `decode(path, bytes)` (a harness plugs in
/// its own format — Claude's is plain JSON), then a required `##END`
/// sentinel (its absence means the scan was cut off, and the snapshot
/// gathered so far must not be trusted as complete: `E_SCAN`). A hash line
/// whose path is `-` (a hasher invoked with no file argument, reading
/// stdin) is skipped rather than recorded as a real file.
pub fn parse_scan_blocks(
    stdout: &str,
    decode: &dyn Fn(&str, &[u8]) -> Option<Value>,
) -> Result<HostSnapshot, IpcError> {
    use base64::Engine;
    let mut snap = HostSnapshot::default();
    let mut current_config: Option<String> = None;
    let mut saw_end = false;
    for line in stdout.lines() {
        if line == "##END" {
            saw_end = true;
            current_config = None;
            continue;
        }
        if line == "##HASHES" {
            current_config = None;
            continue;
        }
        if let Some(path) = line.strip_prefix("##CONFIG ") {
            current_config = Some(path.trim().to_string());
            continue;
        }
        if let Some(path) = current_config.take() {
            let b64 = line.trim();
            if b64.is_empty() {
                continue;
            }
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
                continue;
            };
            if let Some(v) = decode(&path, &bytes) {
                snap.configs.insert(path, v);
            }
            continue;
        }
        // `<hash>  <path>` (two spaces from sha256sum / shasum).
        if let Some((hash, path)) = line.split_once("  ") {
            let path = path.trim_start_matches("./");
            if path == "-" {
                continue;
            }
            snap.files
                .insert(format!("~/{path}"), hash.trim().to_string());
        }
    }
    if !saw_end {
        return Err(IpcError::new(
            crate::ipc_error::codes::E_SCAN,
            "scan output truncated (no ##END)",
        ));
    }
    Ok(snap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan_with(
        files: Vec<(&str, &str)>,
        merges: Vec<(&str, Vec<&str>, serde_json::Value)>,
    ) -> RenderPlan {
        let mut p = RenderPlan::default();
        for (path, body) in files {
            p.files.push(FileWrite {
                path: path.into(),
                bytes: body.as_bytes().to_vec(),
            });
        }
        for (file, path, value) in merges {
            p.merges.push(ConfigMerge {
                file: file.into(),
                json_path: path.into_iter().map(String::from).collect(),
                mode: MergeMode::Set,
                value,
            });
        }
        p
    }

    #[test]
    fn hash_is_order_independent_and_content_sensitive() {
        let a = plan_with(vec![("~/x/a", "1"), ("~/x/b", "2")], vec![]);
        let b = plan_with(vec![("~/x/b", "2"), ("~/x/a", "1")], vec![]);
        let c = plan_with(vec![("~/x/a", "1"), ("~/x/b", "3")], vec![]);
        assert_eq!(a.hash(), b.hash());
        assert_ne!(a.hash(), c.hash());
        let m1 = plan_with(
            vec![],
            vec![(
                "~/.claude.json",
                vec!["mcpServers", "x"],
                serde_json::json!({"a":1,"b":2}),
            )],
        );
        let m2 = plan_with(
            vec![],
            vec![(
                "~/.claude.json",
                vec!["mcpServers", "x"],
                serde_json::json!({"b":2,"a":1}),
            )],
        );
        assert_eq!(
            m1.hash(),
            m2.hash(),
            "canonical JSON: key order must not matter"
        );
    }

    #[test]
    fn note_placeholders_collects_unique_names() {
        let mut p = RenderPlan::default();
        p.note_placeholders("${A} ${B}");
        p.note_placeholders("${B} ${C}");
        assert_eq!(p.placeholders, vec!["A", "B", "C"]);
    }

    #[test]
    fn json_get_walks_objects() {
        let v = serde_json::json!({"a": {"b": [1, 2]}});
        assert_eq!(
            json_get(&v, &["a".into(), "b".into()]),
            Some(&serde_json::json!([1, 2]))
        );
        assert_eq!(json_get(&v, &["a".into(), "zz".into()]), None);
        assert_eq!(json_get(&v, &[]), Some(&v));
    }

    #[test]
    fn apply_and_remove_merges_cover_every_mode() {
        let mut root = json!({});
        let set = ConfigMerge {
            file: "f".into(),
            json_path: vec!["mcpServers".into(), "x".into()],
            mode: MergeMode::Set,
            value: json!({"type":"http"}),
        };
        let app = ConfigMerge {
            file: "f".into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value: json!({"hooks":[{"type":"command","command":"x"}]}),
        };
        let sub = ConfigMerge {
            file: "f".into(),
            json_path: vec!["plugins".into(), "p@m".into()],
            mode: MergeMode::Subset,
            value: json!([{"version":"1"}]),
        };
        apply_merges(&mut root, &[set.clone(), app.clone(), sub.clone()]);
        apply_merges(&mut root, &[app.clone(), sub.clone()]); // idempotent
        assert_eq!(root["mcpServers"]["x"]["type"], "http");
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(root["plugins"]["p@m"].as_array().unwrap().len(), 1);
        let rm = |m: &ConfigMerge| ManifestMerge {
            file: m.file.clone(),
            json_path: m.json_path.clone(),
            mode: m.mode,
            value_hash: value_hash(&m.value),
        };
        remove_merges(&mut root, &[rm(&set), rm(&app), rm(&sub)]);
        assert!(root["mcpServers"].get("x").is_none());
        assert!(root["hooks"].get("Stop").is_none(), "empty array removed");
        assert!(root.get("hooks").is_some(), "top-level key kept");
        assert!(root["plugins"].get("p@m").is_none());
    }

    #[test]
    fn remove_append_unique_keeps_other_elements() {
        let mut root = json!({"hooks":{"Stop":[{"a":1},{"b":2}]}});
        let m = ManifestMerge {
            file: "f".into(),
            json_path: vec!["hooks".into(), "Stop".into()],
            mode: MergeMode::AppendUnique,
            value_hash: value_hash(&json!({"a":1})),
        };
        remove_merges(&mut root, &[m]);
        assert_eq!(root["hooks"]["Stop"], json!([{"b":2}]));
    }

    #[test]
    fn registry_knows_claude_and_codex() {
        assert_eq!(HARNESS_IDS, &["claude", "codex"]);
        assert!(by_id("claude").is_some());
        assert!(by_id("codex").is_some());
        assert!(by_id("nope").is_none());
        assert_eq!(all().len(), 2);
    }
}
