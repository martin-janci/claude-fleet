//! The intermediate representation (IR) for catalog assets.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Neutral tool vocabulary (spec: "Neutral vocabularies").
pub const TOOLS: &[&str] = &[
    "read",
    "edit",
    "write",
    "bash",
    "grep",
    "glob",
    "web_search",
    "web_fetch",
    "browser",
    "agent",
    "*",
];
/// Neutral model tiers.
pub const TIERS: &[&str] = &["fast", "default", "strong"];
/// Neutral hook events.
pub const EVENTS: &[&str] = &[
    "session_start",
    "prompt_submit",
    "before_tool",
    "after_tool",
    "stop",
    "subagent_stop",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Skill,
    Agent,
    Hook,
    McpServer,
    PluginRef,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Skill,
        Kind::Agent,
        Kind::Hook,
        Kind::McpServer,
        Kind::PluginRef,
    ];

    /// Directory inside the catalog repo that holds this kind.
    pub fn dir(&self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Agent => "agents",
            Kind::Hook => "hooks",
            Kind::McpServer => "mcp",
            Kind::PluginRef => "plugins",
        }
    }

    /// Reserved for the sync engine (resolving a scanned/installed path back
    /// to its `Kind`); not yet called from a non-test build.
    #[allow(dead_code)]
    pub fn from_dir(dir: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.dir() == dir)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Agent => "agent",
            Kind::Hook => "hook",
            Kind::McpServer => "mcp_server",
            Kind::PluginRef => "plugin_ref",
        }
    }

    /// Skills and agents are folders (asset.yaml + body); the rest are single files.
    pub fn is_folder(&self) -> bool {
        matches!(self, Kind::Skill | Kind::Agent)
    }
}

fn default_true() -> bool {
    true
}
fn default_version() -> String {
    "1".to_string()
}
fn default_tier() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}

/// Per-harness overrides (`targets.<harness>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetOverride {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Explicit model id for this harness (agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// e.g. `skill` — render an unsupported kind as another kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_as: Option<String>,
    /// Harness-native fields the IR has no slot for; written back verbatim.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for TargetOverride {
    fn default() -> Self {
        Self {
            enabled: true,
            model: None,
            render_as: None,
            extra: BTreeMap::new(),
        }
    }
}

/// Fields shared by every asset kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub kind: Kind,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    // No `skip_serializing_if` here (unlike the other optional header
    // fields): `Asset`'s JSON (the Tauri/MCP API surface) must always carry
    // a `tags` key so `AssetDetail.asset.tags` is never absent on the
    // frontend. The on-disk YAML (`AssetFile`) instead strips an empty
    // `tags` key by hand in its own `Serialize` impl, below, to keep the
    // repo clean.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// The identifier used on hosts, when it differs from `name` (e.g. an
    /// imported `foo_bar` that was slugified to the catalog name `foo-bar`).
    /// Only meaningful for kinds that derive a host path/config key from the
    /// name (skill, agent, mcp_server) — `validate` rejects it on `hook` and
    /// `plugin_ref`. See `Asset::install_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_as: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, TargetOverride>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookMatch {
    pub tool: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookAction {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub source: String,
    pub repo: String,
}

/// Kind-specific fields. Internally tagged on `kind`, flattened next to the
/// header so a YAML file is one flat mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssetSpec {
    Skill {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        allowed_tools: Vec<String>,
        #[serde(default = "default_true")]
        user_invocable: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        triggers: Vec<String>,
    },
    Agent {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<String>,
        #[serde(default = "default_tier")]
        model: String,
    },
    Hook {
        event: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        r#match: Option<HookMatch>,
        action: HookAction,
    },
    McpServer {
        transport: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
    PluginRef {
        harness: String,
        marketplace: Marketplace,
        plugin: String,
        version: String,
    },
}

/// A file that ships alongside a skill (`resources/…`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// Path relative to the skill folder, e.g. `resources/scripts/run.sh`.
    pub rel_path: String,
    #[serde(with = "bytes_b64")]
    pub bytes: Vec<u8>,
}

mod bytes_b64 {
    use base64::Engine;
    pub fn serialize<S: serde::Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
    use serde::Deserialize;
}

/// One catalog problem (a file that could not be loaded, a collision, …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

/// A fully loaded asset: YAML fields plus the body / resources read from disk.
///
/// `Header` and `AssetSpec` both flatten to the same YAML mapping and share a
/// `kind` key (`Header::kind` vs. `AssetSpec`'s internal tag). `serde_yaml`
/// cannot round-trip that through `#[serde(flatten)]`: deserializing reports
/// `missing field kind`, and serializing emits `kind` twice (which then
/// fails to re-parse as `duplicate entry with key "kind"`). Both directions
/// are therefore implemented by hand via `header_and_spec_from_value` /
/// `merge_header_and_spec` below, working through a `serde_yaml::Value` so
/// neither `Header` nor `AssetSpec` ever goes through the flatten combinator.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub header: Header,
    pub spec: AssetSpec,
    /// `body.md` (skill) or `prompt.md` (agent). Empty for other kinds.
    pub body: String,
    pub resources: Vec<Resource>,
}

impl Serialize for Asset {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map =
            merge_header_and_spec(&self.header, &self.spec).map_err(serde::ser::Error::custom)?;
        map.insert("body".into(), self.body.clone().into());
        if !self.resources.is_empty() {
            let resources =
                serde_yaml::to_value(&self.resources).map_err(serde::ser::Error::custom)?;
            map.insert("resources".into(), resources);
        }
        serde_yaml::Value::Mapping(map).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Asset {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let (header, spec) =
            header_and_spec_from_value(&value).map_err(serde::de::Error::custom)?;
        let body = value
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let resources = match value.get("resources") {
            Some(v) => serde_yaml::from_value(v.clone()).map_err(serde::de::Error::custom)?,
            None => Vec::new(),
        };
        Ok(Asset {
            header,
            spec,
            body,
            resources,
        })
    }
}

/// Serde wrapper used only for the on-disk `asset.yaml` (no body/resources).
/// See the comment on `Asset` for why this struct also needs manual
/// `Serialize`/`Deserialize`.
struct AssetFile {
    header: Header,
    spec: AssetSpec,
}

impl Serialize for AssetFile {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map =
            merge_header_and_spec(&self.header, &self.spec).map_err(serde::ser::Error::custom)?;
        // `Header::tags` no longer sets `skip_serializing_if` (see the
        // comment on that field) so `Asset`'s JSON always has the key; the
        // on-disk YAML strips it back out here when empty to keep
        // `asset.yaml` clean.
        if matches!(map.get("tags"), Some(serde_yaml::Value::Sequence(s)) if s.is_empty()) {
            map.remove("tags");
        }
        serde_yaml::Value::Mapping(map).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AssetFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let (header, spec) =
            header_and_spec_from_value(&value).map_err(serde::de::Error::custom)?;
        Ok(AssetFile { header, spec })
    }
}

/// Deserialise `Header` and `AssetSpec` from the same YAML value. Each type
/// deserialises directly from a `serde_yaml::Value` (no flatten combinator),
/// which sidesteps the `serde_yaml` + flatten + internally-tagged-enum bug;
/// unrecognised fields (the other type's fields) are ignored by default,
/// same as any non-`deny_unknown_fields` struct or enum.
fn header_and_spec_from_value(
    value: &serde_yaml::Value,
) -> Result<(Header, AssetSpec), serde_yaml::Error> {
    let header: Header = serde_yaml::from_value(value.clone())?;
    let spec: AssetSpec = serde_yaml::from_value(value.clone())?;
    Ok((header, spec))
}

/// Serialise `Header` then `AssetSpec` into one mapping, `kind` first
/// (written once by `Header`, then left untouched when `AssetSpec`'s tag
/// re-inserts the same key/value at its existing position).
fn merge_header_and_spec(
    header: &Header,
    spec: &AssetSpec,
) -> Result<serde_yaml::Mapping, serde_yaml::Error> {
    let mut map = serde_yaml::Mapping::new();
    if let serde_yaml::Value::Mapping(header_map) = serde_yaml::to_value(header)? {
        map.extend(header_map);
    }
    if let serde_yaml::Value::Mapping(spec_map) = serde_yaml::to_value(spec)? {
        map.extend(spec_map);
    }
    Ok(map)
}

impl Asset {
    /// Parse an asset YAML document. `expected` is the kind implied by the
    /// directory the file was found in; a mismatch is an error.
    pub fn from_yaml(expected: Option<Kind>, yaml: &str) -> Result<Asset, String> {
        let file: AssetFile = serde_yaml::from_str(yaml).map_err(|e| e.to_string())?;
        if let Some(k) = expected {
            if file.header.kind != k {
                return Err(format!(
                    "kind is {} but the file lives in the {} directory",
                    file.header.kind.as_str(),
                    k.dir()
                ));
            }
        }
        let spec_kind = match &file.spec {
            AssetSpec::Skill { .. } => Kind::Skill,
            AssetSpec::Agent { .. } => Kind::Agent,
            AssetSpec::Hook { .. } => Kind::Hook,
            AssetSpec::McpServer { .. } => Kind::McpServer,
            AssetSpec::PluginRef { .. } => Kind::PluginRef,
        };
        if spec_kind != file.header.kind {
            return Err("kind field does not match the kind-specific fields".into());
        }
        Ok(Asset {
            header: file.header,
            spec: file.spec,
            body: String::new(),
            resources: vec![],
        })
    }

    /// Serialise the YAML part (header + spec). Body and resources are files.
    pub fn to_yaml(&self) -> String {
        let file = AssetFile {
            header: self.header.clone(),
            spec: self.spec.clone(),
        };
        // A struct with two flattened parts always serialises; unwrap is safe.
        serde_yaml::to_string(&file).unwrap_or_default()
    }

    pub fn kind(&self) -> Kind {
        self.header.kind
    }

    /// Resolve the effective override for a harness (default when absent).
    pub fn target(&self, harness: &str) -> TargetOverride {
        self.header
            .targets
            .get(harness)
            .cloned()
            .unwrap_or_default()
    }

    /// The identifier a harness should install this asset under: `install_as`
    /// when set, else the catalog `name`.
    pub fn install_name(&self) -> &str {
        self.header
            .install_as
            .as_deref()
            .unwrap_or(&self.header.name)
    }

    /// Return every validation problem (empty means valid).
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !is_valid_name(&self.header.name) {
            out.push(format!(
                "name '{}' must match [a-z0-9][a-z0-9-]*",
                self.header.name
            ));
        }
        if self.header.description.trim().is_empty() {
            out.push("description must not be empty".into());
        }
        if let Some(install_as) = &self.header.install_as {
            if matches!(self.header.kind, Kind::Hook | Kind::PluginRef) {
                out.push(format!(
                    "install_as is not supported for kind '{}'",
                    self.header.kind.as_str()
                ));
            } else if !is_valid_install_name(install_as) {
                out.push(format!(
                    "install_as '{install_as}' must be non-empty, match [A-Za-z0-9._-]+, and not be '.' or '..'"
                ));
            }
        }
        match &self.spec {
            AssetSpec::Skill { allowed_tools, .. } => {
                for t in allowed_tools {
                    if !is_valid_tool(t) {
                        out.push(format!("allowed_tools: unknown tool '{t}'"));
                    }
                }
            }
            AssetSpec::Agent { tools, model } => {
                for t in tools {
                    if !is_valid_tool(t) {
                        out.push(format!("tools: unknown tool '{t}'"));
                    }
                }
                if !TIERS.contains(&model.as_str()) {
                    out.push(format!("model '{model}' must be one of {TIERS:?}"));
                }
            }
            AssetSpec::Hook { event, action, .. } => {
                if !EVENTS.contains(&event.as_str()) {
                    out.push(format!("event '{event}' must be one of {EVENTS:?}"));
                }
                match action.kind.as_str() {
                    "command" if is_blank(&action.command) => {
                        out.push("action.command is required for type command".into())
                    }
                    "http" if is_blank(&action.url) => {
                        out.push("action.url is required for type http".into())
                    }
                    "command" | "http" => {}
                    other => out.push(format!("action.type '{other}' must be command or http")),
                }
            }
            AssetSpec::McpServer {
                transport,
                url,
                command,
                ..
            } => match transport.as_str() {
                "http" if is_blank(url) => out.push("url is required for transport http".into()),
                "stdio" if is_blank(command) => {
                    out.push("command is required for transport stdio".into())
                }
                "http" | "stdio" => {}
                other => out.push(format!("transport '{other}' must be http or stdio")),
            },
            AssetSpec::PluginRef {
                harness, version, ..
            } => {
                if harness != "claude" {
                    out.push(format!(
                        "harness '{harness}' is not supported for plugin refs"
                    ));
                }
                if version.trim().is_empty() {
                    out.push("version must be an exact version or 'latest'".into());
                }
            }
        }
        out
    }
}

/// `None` counts as missing, and so does a `Some` value that is empty or
/// whitespace-only.
fn is_blank(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(|s| s.trim().is_empty())
}

pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `install_as`'s constraint: non-empty, every character in `[A-Za-z0-9._-]`,
/// and not `.` or `..` (both of which are meaningless or dangerous as a path
/// segment). The charset is applied to the raw value and nothing is trimmed
/// first, so surrounding whitespace is simply invalid — `install_name()`
/// hands the raw value to the harnesses, and the two must agree.
pub fn is_valid_install_name(s: &str) -> bool {
    if s.is_empty() || s == "." || s == ".." {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn is_valid_tool(t: &str) -> bool {
    TOOLS.contains(&t) || t.starts_with("mcp:")
}

/// Every `${NAME}` placeholder in `s`, in order of first appearance, deduplicated.
pub fn find_placeholders(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                    && !out.iter().any(|n| n == name)
                {
                    out.push(name.to_string());
                }
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL_YAML: &str = r#"
kind: skill
name: worktree
version: "1"
description: Create an isolated git worktree.
tags: [core]
allowed_tools: [bash, read]
triggers: ["new worktree"]
targets:
  claude:
    extra:
      disable-model-invocation: true
  codex:
    enabled: false
"#;

    #[test]
    fn skill_round_trips_through_yaml() {
        let a = Asset::from_yaml(None, SKILL_YAML).expect("parse");
        assert_eq!(a.header.kind, Kind::Skill);
        assert_eq!(a.header.name, "worktree");
        match &a.spec {
            AssetSpec::Skill {
                allowed_tools,
                user_invocable,
                triggers,
            } => {
                assert_eq!(allowed_tools, &vec!["bash".to_string(), "read".to_string()]);
                assert!(*user_invocable);
                assert_eq!(triggers, &vec!["new worktree".to_string()]);
            }
            other => panic!("wrong spec: {other:?}"),
        }
        assert!(!a.header.targets["codex"].enabled);
        assert_eq!(
            a.header.targets["claude"].extra["disable-model-invocation"],
            serde_json::Value::Bool(true)
        );
        let again = Asset::from_yaml(None, &a.to_yaml()).expect("re-parse");
        assert_eq!(again.header, a.header);
        assert_eq!(again.spec, a.spec);
    }

    #[test]
    fn hook_and_mcp_and_plugin_parse() {
        let hook = Asset::from_yaml(
            None,
            r#"
kind: hook
name: rtk-bash
description: Rewrite bash through rtk.
event: before_tool
match: { tool: bash }
action:
  type: command
  command: "if command -v rtk >/dev/null 2>&1; then exec rtk hook claude; fi"
"#,
        )
        .expect("hook");
        match &hook.spec {
            AssetSpec::Hook {
                event,
                r#match,
                action,
            } => {
                assert_eq!(event, "before_tool");
                assert_eq!(r#match.as_ref().unwrap().tool, "bash");
                assert_eq!(action.kind, "command");
            }
            other => panic!("wrong spec: {other:?}"),
        }
        let mcp = Asset::from_yaml(
            None,
            r#"
kind: mcp_server
name: claude-fleet
description: Fleet control API.
transport: http
url: http://127.0.0.1:${FLEET_MCP_PORT}/mcp
headers: { Authorization: "Bearer ${FLEET_MCP_TOKEN}" }
"#,
        )
        .expect("mcp");
        assert!(matches!(mcp.spec, AssetSpec::McpServer { .. }));
        let plugin = Asset::from_yaml(
            None,
            r#"
kind: plugin_ref
name: superpowers
description: Superpowers plugin.
harness: claude
marketplace: { name: superpowers-marketplace, source: github, repo: obra/superpowers-marketplace }
plugin: superpowers
version: "6.3.0"
"#,
        )
        .expect("plugin");
        assert!(matches!(plugin.spec, AssetSpec::PluginRef { .. }));
    }

    #[test]
    fn validation_catches_bad_name_empty_description_bad_vocab() {
        let mut a = Asset::from_yaml(None, SKILL_YAML).unwrap();
        a.header.name = "Bad Name".into();
        a.header.description = "".into();
        let problems = a.validate();
        assert!(problems.iter().any(|p| p.contains("name")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("description")),
            "{problems:?}"
        );

        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: x\ndescription: d\nevent: on_fire\naction: { type: smoke }\n",
        )
        .unwrap();
        let problems = hook.validate();
        assert!(problems.iter().any(|p| p.contains("event")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("action.type")),
            "{problems:?}"
        );

        let agent = Asset::from_yaml(
            None,
            "kind: agent\nname: a\ndescription: d\nmodel: enormous\n",
        )
        .unwrap();
        assert!(agent.validate().iter().any(|p| p.contains("model")));
    }

    #[test]
    fn kind_mismatch_with_directory_is_rejected() {
        let err = Asset::from_yaml(Some(Kind::Agent), SKILL_YAML).unwrap_err();
        assert!(err.contains("kind"), "{err}");
    }

    #[test]
    fn placeholders_and_hash() {
        assert_eq!(
            find_placeholders("Bearer ${FLEET_MCP_TOKEN} and ${X_1}"),
            vec!["FLEET_MCP_TOKEN".to_string(), "X_1".to_string()]
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn asset_json_always_includes_tags_even_when_empty() {
        // Regression test: `AssetDetail::asset` (the Tauri/MCP JSON API) must
        // always carry a `tags` key so `detail.asset.tags` is never `undefined`
        // on the frontend, even for an asset whose header has no tags.
        let a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        let v = serde_json::to_value(&a).unwrap();
        let obj = v.as_object().unwrap();
        for key in ["kind", "name", "version", "description", "tags", "body"] {
            assert!(obj.contains_key(key), "missing '{key}': {obj:?}");
        }
        assert_eq!(obj["tags"], serde_json::json!([]));
        // Fields that are genuinely absent stay absent.
        assert!(!obj.contains_key("source"));
        assert!(!obj.contains_key("resources"));
    }

    #[test]
    fn to_yaml_omits_empty_tags_but_keeps_populated_ones() {
        let a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        let yaml = a.to_yaml();
        assert!(!yaml.contains("tags"), "{yaml}");

        let tagged = Asset::from_yaml(None, SKILL_YAML).unwrap();
        assert!(tagged.to_yaml().contains("tags"));
    }

    #[test]
    fn blank_hook_action_command_counts_as_missing() {
        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: x\ndescription: d\nevent: before_tool\naction: { type: command, command: \"\" }\n",
        )
        .unwrap();
        let problems = hook.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "action.command is required for type command"),
            "{problems:?}"
        );

        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: x\ndescription: d\nevent: before_tool\naction: { type: command, command: \"   \" }\n",
        )
        .unwrap();
        let problems = hook.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "action.command is required for type command"),
            "{problems:?}"
        );
    }

    #[test]
    fn blank_hook_action_url_counts_as_missing() {
        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: x\ndescription: d\nevent: before_tool\naction: { type: http, url: \"\" }\n",
        )
        .unwrap();
        let problems = hook.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "action.url is required for type http"),
            "{problems:?}"
        );

        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: x\ndescription: d\nevent: before_tool\naction: { type: http, url: \"   \" }\n",
        )
        .unwrap();
        let problems = hook.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "action.url is required for type http"),
            "{problems:?}"
        );
    }

    #[test]
    fn blank_mcp_url_counts_as_missing() {
        let mcp = Asset::from_yaml(
            None,
            "kind: mcp_server\nname: x\ndescription: d\ntransport: http\nurl: \"\"\n",
        )
        .unwrap();
        let problems = mcp.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "url is required for transport http"),
            "{problems:?}"
        );

        let mcp = Asset::from_yaml(
            None,
            "kind: mcp_server\nname: x\ndescription: d\ntransport: http\nurl: \"   \"\n",
        )
        .unwrap();
        let problems = mcp.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "url is required for transport http"),
            "{problems:?}"
        );
    }

    #[test]
    fn blank_mcp_command_counts_as_missing() {
        let mcp = Asset::from_yaml(
            None,
            "kind: mcp_server\nname: x\ndescription: d\ntransport: stdio\ncommand: \"\"\n",
        )
        .unwrap();
        let problems = mcp.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "command is required for transport stdio"),
            "{problems:?}"
        );

        let mcp = Asset::from_yaml(
            None,
            "kind: mcp_server\nname: x\ndescription: d\ntransport: stdio\ncommand: \"   \"\n",
        )
        .unwrap();
        let problems = mcp.validate();
        assert!(
            problems
                .iter()
                .any(|p| p == "command is required for transport stdio"),
            "{problems:?}"
        );
    }

    #[test]
    fn install_as_round_trips_through_yaml_and_json() {
        let mut a = Asset::from_yaml(None, SKILL_YAML).unwrap();
        a.header.install_as = Some("foo_bar".into());
        assert_eq!(a.install_name(), "foo_bar");

        let yaml = a.to_yaml();
        assert!(yaml.contains("install_as: foo_bar"), "{yaml}");
        let again = Asset::from_yaml(None, &yaml).unwrap();
        assert_eq!(again.header.install_as.as_deref(), Some("foo_bar"));
        assert_eq!(again.install_name(), "foo_bar");

        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["install_as"], serde_json::json!("foo_bar"));

        // Without `install_as`: the key is absent in both forms, and
        // `install_name()` falls back to `name`.
        let plain = Asset::from_yaml(None, SKILL_YAML).unwrap();
        assert!(plain.header.install_as.is_none());
        assert!(
            !plain.to_yaml().contains("install_as"),
            "{}",
            plain.to_yaml()
        );
        let plain_json = serde_json::to_value(&plain).unwrap();
        assert!(!plain_json.as_object().unwrap().contains_key("install_as"));
        assert_eq!(plain.install_name(), plain.header.name);
    }

    #[test]
    fn validate_rejects_bad_install_as() {
        let hook_yaml = "kind: hook\nname: x\ndescription: A reasonably long description here.\nevent: before_tool\naction: { type: command, command: echo }\n";
        let plugin_yaml = "kind: plugin_ref\nname: x\ndescription: A reasonably long description here.\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: p\nversion: \"1.0.0\"\n";

        for kind_yaml in [hook_yaml, plugin_yaml] {
            for value in ["", "a b", "..", "a/b", "ok"] {
                let mut a = Asset::from_yaml(None, kind_yaml).unwrap();
                a.header.install_as = Some(value.to_string());
                let problems = a.validate();
                assert!(
                    problems.iter().any(|p| p.starts_with("install_as")),
                    "kind {:?} value {value:?}: {problems:?}",
                    a.header.kind
                );
            }
        }

        let skill_yaml = "kind: skill\nname: x\ndescription: A reasonably long description here.\n";
        let agent_yaml = "kind: agent\nname: x\ndescription: A reasonably long description here.\n";
        let mcp_yaml = "kind: mcp_server\nname: x\ndescription: A reasonably long description here.\ntransport: http\nurl: http://127.0.0.1/mcp\n";
        for kind_yaml in [skill_yaml, agent_yaml, mcp_yaml] {
            let mut a = Asset::from_yaml(None, kind_yaml).unwrap();
            a.header.install_as = Some("foo_bar".to_string());
            let problems = a.validate();
            assert!(problems.is_empty(), "{:?}: {problems:?}", a.header.kind);
        }
    }

    #[test]
    fn is_valid_install_name_checks_the_raw_value() {
        assert!(is_valid_install_name("foo_bar"));
        assert!(is_valid_install_name("a.b"));
        assert!(is_valid_install_name("-"));
        // Nothing is trimmed away first: surrounding whitespace is simply
        // invalid, because the charset is applied to the raw value.
        for bad in [
            "", " ", " foo", "foo ", " foo ", ".", "..", " . ", " .. ", "a/b",
        ] {
            assert!(!is_valid_install_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn kind_dirs_and_names() {
        assert_eq!(Kind::Skill.dir(), "skills");
        assert_eq!(Kind::McpServer.dir(), "mcp");
        assert_eq!(Kind::PluginRef.dir(), "plugins");
        assert_eq!(Kind::from_dir("agents"), Some(Kind::Agent));
        assert_eq!(Kind::Hook.as_str(), "hook");
    }
}
