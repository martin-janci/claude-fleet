//! Per-harness renderers. A harness turns an IR asset into a `RenderPlan`
//! (files to write + JSON merges into config files), knows how to scan a
//! host for what is installed, and can list the assets it finds there.

pub mod claude;
pub mod codex;

use super::model::{find_placeholders, sha256_hex, Asset, Kind};
use super::sync::plan::ActionOp;
use crate::ipc_error::IpcError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Every harness id the catalog knows, in the order an explicit
/// `hosts.harnesses` list is normalised to. Read by the lint (`targets.<h>`)
/// and by `harness_set` (multi-harness F3a).
pub const HARNESS_IDS: &[&str] = &["claude", "codex"];

/// A file the harness would write on the host. `path` uses `~/` for the home dir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
/// Persisted alongside the manifest file (see `manifest::ManifestEntry`) and
/// replayed through `remove_merges` when a sync supersedes or drops the
/// asset that produced it.
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
/// Populates `ManifestMerge::value_hash`.
pub fn value_hash(v: &Value) -> String {
    sha256_hex(serde_json::to_string(v).unwrap_or_default().as_bytes())
}

/// Everything a harness would do to install one asset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

/// What a host scan found: file hashes keyed by `~/`-relative path, parsed
/// JSON config files keyed the same way, and whether the scan saw the
/// harness itself on the host.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HostSnapshot {
    pub files: BTreeMap<String, String>,
    pub configs: BTreeMap<String, serde_json::Value>,
    /// A `##PRESENT` line was in the scan output (multi-harness F3a). Only
    /// a scan that probes for its harness prints one — Codex's does (the
    /// `codex` CLI on PATH, `~/.codex/auth.json` or `~/.codex/sessions`);
    /// Claude's does not, and `harness_set::harness_gate` never asks for
    /// Claude.
    pub present: bool,
    /// Multi-harness F3c: every directory the scan found to be a symlink,
    /// `~/`-relative path → its target as `readlink` printed it (control
    /// characters dropped, at most 256 characters). Only Codex's scan
    /// reports any (`##LINK`); `sync::plan` refuses every write, adopt or
    /// removal under one (`Harness::symlink_reason`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, String>,
}

/// One installed asset with what a scan can say about it without the
/// catalog: a content hash (so identical copies on different hosts are
/// recognisable), whether it looks like it carries a secret, and whether
/// fleet itself put it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAsset {
    pub kind: Kind,
    pub name: String,
    pub hash: Option<String>,
    pub secret_like: bool,
    pub fleet_owned: bool,
}

/// JSON with every object's keys sorted, recursively: a stable input for
/// hashing whatever order the host wrote.
pub fn canonical_json(v: &serde_json::Value) -> String {
    fn sorted(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&m[k]));
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

/// sha256 over `<relative path>=<file hash>` lines for every file under
/// `prefix` (which ends in `/`), sorted by path. `None` when there is none.
pub fn dir_hash(snap: &HostSnapshot, prefix: &str) -> Option<String> {
    let mut lines: Vec<String> = snap
        .files
        .iter()
        .filter_map(|(p, h)| p.strip_prefix(prefix).map(|rel| format!("{rel}={h}")))
        .collect();
    if lines.is_empty() {
        return None;
    }
    lines.sort();
    Some(super::model::sha256_hex(lines.join("\n").as_bytes()))
}

/// Does an MCP server entry look like it carries a credential?
pub fn mcp_secret_like(v: &serde_json::Value) -> bool {
    let non_empty = |k: &str| {
        v.get(k)
            .and_then(|o| o.as_object())
            .is_some_and(|o| !o.is_empty())
    };
    let url_secret = v.get("url").and_then(|u| u.as_str()).is_some_and(|u| {
        ["token=", "key=", "secret="]
            .iter()
            .any(|s| u.to_lowercase().contains(s))
    });
    non_empty("env") || non_empty("headers") || url_secret
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
    /// `installed()` with a content hash and flags per asset. The default
    /// knows nothing beyond the identity.
    fn installed_detail(&self, snap: &HostSnapshot) -> Vec<InstalledAsset> {
        self.installed(snap)
            .into_iter()
            .map(|(kind, name)| InstalledAsset {
                kind,
                name,
                hash: None,
                secret_like: false,
                fleet_owned: false,
            })
            .collect()
    }
    /// Home-relative path (`~/...`) to the manifest file this harness uses
    /// to track which merges/files the sync engine applied.
    fn manifest_path(&self) -> &'static str;
    /// Why an action touching a path under `link` — a directory the scan
    /// reported as a symlink to `target` (`HostSnapshot::links`) — is
    /// refused (multi-harness F3c). Only a scan that reports links ever
    /// makes the planner ask. `op` is the action's own op, taken *before*
    /// `sync::plan` blocks it: a `Remove` (deleting something) reads
    /// differently from every other op (writing or adopting something).
    fn symlink_reason(&self, link: &str, target: &str, op: ActionOp) -> String {
        let verb = if op == ActionOp::Remove {
            "remove"
        } else {
            "write"
        };
        format!(
            "{link} is a symlink (to {target}); fleet won't {verb} {} files through it — replace it with a real directory",
            self.id()
        )
    }
    /// Apply `merges` then `remove` to `existing`'s parsed content and
    /// return the full new file text (with a trailing newline). An empty
    /// `existing` means an empty document.
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

/// Looks up one harness by id without building the whole registry via
/// `all()` (`drift_diff`).
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
/// Used by `remove_merges`'s `AppendUnique` case.
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
/// Used by `apply_merges`'s `Subset` case.
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
/// Used by `apply_merges`.
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
/// Called from each harness's `Harness::merge_config` implementation.
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
/// Used by `remove_merges`.
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
/// Called from each harness's `Harness::merge_config` implementation.
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

/// Is `s` a plausible content digest — `[0-9a-f]{1,128}`? Hashes cross the
/// SSH boundary as untrusted host output and are later interpolated into an
/// apply script, so nothing else is ever accepted as one.
pub fn is_hex_hash(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A `##LINK` target as a plan reason may show it: host output, so control
/// characters are dropped and it is capped at 256 characters; an empty one
/// (a `readlink` that printed nothing) reads as unknown.
fn link_target(raw: &str) -> String {
    let t: String = raw.chars().filter(|c| !c.is_control()).take(256).collect();
    if t.is_empty() {
        "an unknown target".to_string()
    } else {
        t
    }
}

/// Generalised `parse_scan`: `##HASHES` lines, then `##CONFIG <path>` blocks
/// whose base64 body is decoded by `decode(path, bytes)` (a harness plugs in
/// its own format — Claude's is plain JSON), then a required `##END`
/// sentinel (its absence means the scan was cut off, and the snapshot
/// gathered so far must not be trusted as complete: `E_SCAN`). A hash line
/// whose path is `-` (a hasher invoked with no file argument, reading
/// stdin) is skipped rather than recorded as a real file.
///
/// `##LINK <path>` (multi-harness F3c) works the same way as a `##CONFIG`
/// block: the line right after it is UNCONDITIONALLY its target, whatever
/// that line's own content is — even another `##`-prefixed marker, even
/// empty — never split out of the `##LINK` line itself, so a directory
/// whose own name happens to contain `" -> "` cannot corrupt the parsed
/// path. A target that happens to print e.g. `##PRESENT` is kept
/// (sanitised) as that literal text, not mistaken for the presence marker.
/// A path outside `~/` names nothing fleet writes: it and its target line
/// are both dropped, but the target line is still consumed — so a line
/// that would otherwise look like a `<hash>  <path>` hash line is never
/// recorded as a file. Only true end-of-input right after `##LINK <path>`,
/// with no line at all following it, cannot be swallowed this way: that
/// reads as `link_target("")`, "an unknown target" (and, same as any
/// truncated scan, `##END` never arrives either, so the whole parse still
/// fails as `E_SCAN`).
pub fn parse_scan_blocks(
    stdout: &str,
    decode: &dyn Fn(&str, &[u8]) -> Option<Value>,
) -> Result<HostSnapshot, IpcError> {
    use base64::Engine;
    let mut snap = HostSnapshot::default();
    let mut current_config: Option<String> = None;
    // Outer `Some` = "the next line is this link's target"; inner
    // `Some(path)` keeps it, `None` drops it (a non-`~/` path) — either way
    // the next line is consumed and never reinterpreted as anything else.
    let mut current_link: Option<Option<String>> = None;
    let mut saw_end = false;
    for line in stdout.lines() {
        if let Some(pending) = current_link.take() {
            if let Some(path) = pending {
                snap.links.insert(path, link_target(line));
            }
            continue;
        }
        if line == "##END" {
            saw_end = true;
            current_config = None;
            continue;
        }
        if line == "##HASHES" {
            current_config = None;
            continue;
        }
        if line == "##PRESENT" {
            snap.present = true;
            current_config = None;
            continue;
        }
        if let Some(path) = line.strip_prefix("##LINK ") {
            current_config = None;
            // A path outside `~/` names nothing fleet writes and is
            // dropped — but the next line (its target) is still consumed
            // above, just not kept.
            current_link = Some(path.starts_with("~/").then(|| path.to_string()));
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
            // Scan output is host-supplied and ends up interpolated into a
            // later apply script's compare-and-swap test, so only a real
            // hex digest is ever recorded: anything else is dropped (the
            // file then reads as absent, which plans a create, not a silent
            // overwrite).
            let hash = hash.trim();
            if !is_hex_hash(hash) {
                tracing::warn!(path, "scan reported a non-hex hash; ignoring the line");
                continue;
            }
            snap.files.insert(format!("~/{path}"), hash.to_string());
        }
    }
    // The scan was cut off right after naming a link, with no line at all
    // after it (not even `##END`): unknown. `saw_end` is false here too, so
    // the snapshot is about to be discarded as `E_SCAN` anyway — this only
    // matters for a caller that inspects `links` without checking the
    // `Result` first.
    if let Some(Some(path)) = current_link.take() {
        snap.links.insert(path, link_target(""));
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

    /// A scan line whose "hash" is not a hex digest is dropped rather than
    /// recorded: it would otherwise be interpolated verbatim into an apply
    /// script's `[ "$cur" != "<expected>" ]` test.
    #[test]
    fn parse_scan_blocks_only_accepts_hex_hashes() {
        let good = "a".repeat(64);
        let stdout = format!(
            "##HASHES\n{good}  .claude/skills/ok/SKILL.md\n\"; rm -rf /; echo \"  .claude/skills/evil/SKILL.md\nDEADBEEF  .claude/skills/upper/SKILL.md\n{}  .claude/skills/toolong/SKILL.md\n##END\n",
            "a".repeat(129)
        );
        let snap = parse_scan_blocks(&stdout, &|_, _| None).unwrap();
        assert_eq!(
            snap.files.keys().collect::<Vec<_>>(),
            vec!["~/.claude/skills/ok/SKILL.md"]
        );
        assert_eq!(snap.files["~/.claude/skills/ok/SKILL.md"], good);
        assert!(is_hex_hash("0123456789abcdef"));
        assert!(!is_hex_hash(""));
        assert!(!is_hex_hash("ABCDEF"));
        assert!(!is_hex_hash("abc def"));
        assert!(!is_hex_hash(&"a".repeat(129)));
    }

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

    /// `##PRESENT` (multi-harness F3a) says the scan saw the harness itself
    /// on the host; its absence says it did not. It is neither a hash line
    /// nor a config body, wherever it appears.
    #[test]
    fn parse_scan_blocks_reads_the_presence_line() {
        let present = parse_scan_blocks(
            "##PRESENT\n##HASHES\naaaa  .codex/skills/s/SKILL.md\n##END\n",
            &|_, _| None,
        )
        .unwrap();
        assert!(present.present);
        assert_eq!(present.files.len(), 1);
        let absent = parse_scan_blocks("##HASHES\n##END\n", &|_, _| None).unwrap();
        assert!(!absent.present);
    }

    /// `##LINK <path>`, then its target on the next line (multi-harness
    /// F3c), records a symlinked directory: a path not under `~/` is
    /// ignored (its target line is still consumed), a target loses control
    /// characters, an empty one reads as unknown, and none is a hash line.
    #[test]
    fn parse_scan_blocks_reads_symlinked_dirs() {
        let snap = parse_scan_blocks(
            "##LINK ~/.agents/skills\n/home/u/.claude/skills\n##LINK ~/.codex/skills\n\n##LINK /etc/x\n/y\n##LINK ~/.agents/skills/a b\n../x\u{7}y\n##HASHES\n##END\n",
            &|_, _| None,
        )
        .unwrap();
        assert_eq!(
            snap.links,
            BTreeMap::from([
                (
                    "~/.agents/skills".to_string(),
                    "/home/u/.claude/skills".to_string()
                ),
                ("~/.agents/skills/a b".to_string(), "../xy".to_string()),
                (
                    "~/.codex/skills".to_string(),
                    "an unknown target".to_string()
                ),
            ])
        );
        assert!(snap.files.is_empty());
    }

    /// EXTRA (F3c controller ruling): the target is always the line right
    /// after `##LINK <path>`, never split out of that line with `" -> "` —
    /// so a directory whose own name contains that exact substring parses
    /// intact instead of corrupting the path.
    #[test]
    fn parse_scan_blocks_a_link_path_may_contain_the_old_separator_literally() {
        let snap = parse_scan_blocks(
            "##LINK ~/.agents/skills/a -> b\n/x/y\n##HASHES\n##END\n",
            &|_, _| None,
        )
        .unwrap();
        assert_eq!(
            snap.links,
            BTreeMap::from([("~/.agents/skills/a -> b".to_string(), "/x/y".to_string())])
        );
    }

    /// M4 (F3c review fix round 1): the line right after `##LINK <path>` is
    /// unconditionally its target, even when that line itself looks like
    /// the `##PRESENT` marker — it must not be mistaken for one.
    #[test]
    fn parse_scan_blocks_a_links_target_that_looks_like_present_is_not_parsed_as_present() {
        let snap = parse_scan_blocks(
            "##LINK ~/.agents/skills\n##PRESENT\n##HASHES\n##END\n",
            &|_, _| None,
        )
        .unwrap();
        assert!(
            !snap.present,
            "the marker-looking line was consumed as a target, not a directive"
        );
        assert_eq!(
            snap.links,
            BTreeMap::from([("~/.agents/skills".to_string(), "##PRESENT".to_string())])
        );
    }

    /// M4: a link outside `~/` is dropped, but its target line is still
    /// consumed — so a line that would otherwise look like a valid
    /// `<hash>  <path>` hash line is never recorded as a file.
    #[test]
    fn parse_scan_blocks_a_non_tilde_links_target_line_is_not_parsed_as_a_hash_line() {
        let hash = "a".repeat(64);
        let snap = parse_scan_blocks(
            &format!("##LINK /etc/x\n{hash}  some/file\n##HASHES\n##END\n"),
            &|_, _| None,
        )
        .unwrap();
        assert!(snap.links.is_empty());
        assert!(snap.files.is_empty(), "{:?}", snap.files);
    }
}
