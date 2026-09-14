//! Per-harness renderers. A harness turns an IR asset into a `RenderPlan`
//! (files to write + JSON merges into config files), knows how to scan a
//! host for what is installed, and can list the assets it finds there.

// Task 2 lands the trait, render plan and registry; the renderers themselves
// (Tasks 3 and 4) are the only callers today, so most of this reads as dead
// in a non-test build. See model.rs for the same pattern.
#![allow(dead_code)]

pub mod claude;
pub mod codex;

use super::model::{find_placeholders, sha256_hex, Asset, Kind};
use crate::ipc_error::IpcError;
use serde::Serialize;
use std::collections::BTreeMap;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
}

pub fn all() -> Vec<Box<dyn Harness>> {
    vec![Box::new(claude::Claude), Box::new(codex::Codex)]
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn registry_knows_claude_and_codex() {
        assert_eq!(HARNESS_IDS, &["claude", "codex"]);
        assert!(by_id("claude").is_some());
        assert!(by_id("codex").is_some());
        assert!(by_id("nope").is_none());
        assert_eq!(all().len(), 2);
    }
}
