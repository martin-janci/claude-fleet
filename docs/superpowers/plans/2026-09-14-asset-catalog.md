# Asset Catalog (sub-project 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a harness-neutral asset catalog to claude-fleet: a git repo of skills, agents, hooks, MCP servers and plugin refs in a fleet-defined IR, rendered per harness (Claude Code fully, Codex CLI for skills and MCP), imported from the controller's `~/.claude`, and compared against every host's installed state in a new Assets tab.

**Architecture:** A new `service/catalog/` module owns the IR (`model.rs`), a `Harness` trait with per-harness renderers (`harness/claude.rs`, `harness/codex.rs`), catalog loading from a local git clone (`repo.rs`), an importer from a Claude config directory (`import.rs`), and a host scanner that computes per-asset drift states (`inventory.rs`). The loaded catalog lives in a process-wide `RwLock`; inventory rows persist in SQLite (migration 018) and flow to the UI as row events. Thin Tauri commands (`commands/assets.rs`) and three MCP tools wrap the service. The frontend gets an `assets.ts` store and an `AssetsPanel.svelte` view tab.

**Tech Stack:** Rust (tauri 2, serde, serde_yaml 0.9, sha2, hex, base64, rusqlite, rmcp), Svelte 5 + TypeScript, Vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-09-14-asset-catalog-design.md`

## Global Constraints

- Service functions take `&Mutex<Store>` / `&Arc<SshClient>`, never `tauri::State`. Never hold the `Store` mutex across an `.await`.
- Every value interpolated into a shell script is quoted with `crate::shell::quote`. Paths starting with `~/` in remote scripts must use the `"$HOME"/'rest'` form (see `remote_path` in `service/provision.rs`).
- Wire field names are snake_case on both sides (no serde rename). Every Rust `Option<T>` is `T | null` in TS.
- New `IpcError` codes used in this plan: `E_CATALOG_NOT_CONFIGURED`, `E_CATALOG_GIT`, `E_CATALOG_PARSE`, `E_ASSET_UNSUPPORTED`, `E_ASSET_EXISTS`, `E_ASSET_NOT_FOUND`.
- Migration is `018_asset_catalog.sql`; schema version assertions move from `17` to `18` in `store.rs` (two places) and `service/health.rs` (one place).
- MCP tools added: exactly `list_assets`, `scan_assets`, `import_assets`, in one commit; then regenerate `docs/control-api-reference.md` with `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`.
- Read-only towards hosts. Nothing in this plan writes to any host's config. The importer writes only into the local catalog repo working tree and never commits.
- Rust verification: `cargo` on this headless box fails in the gtk build script. Run every `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` step on a machine with the Tauri system libraries (the developer's desktop, or a fleet host that has them). Do not skip Rust verification steps; if they cannot run where you are, say so explicitly in the task report.
- Frontend verification runs anywhere: `pnpm run check && pnpm run test && pnpm run build`. Pre-existing failures in `session_ui.test.ts` / `App.test.ts` (`localStorage is undefined`) are not yours; compare against `main` if unsure.
- Commit after every task with a Conventional Commit message, ending with the line `Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS`. Never bump versions or edit `CHANGELOG.md` by hand.

## File Structure

New backend files (one responsibility each):

| File | Responsibility |
|---|---|
| `src-tauri/src/service/catalog/mod.rs` | Module wiring, error-code constants, process-wide `CatalogState`, service entry points used by commands and MCP (`configure`, `load`, `list_assets`, `get_asset`, `scan_hosts`, `import_host`, `inventory`) |
| `src-tauri/src/service/catalog/model.rs` | IR types (`Asset`, `Header`, `AssetSpec`, …), YAML (de)serialisation, validation, neutral vocabularies, placeholder detection, sha256 helper |
| `src-tauri/src/service/catalog/harness/mod.rs` | `RenderPlan`, `FileWrite`, `ConfigMerge`, `MergeMode`, `HostSnapshot`, `Unsupported`, the `Harness` trait, and the registry (`all()`, `by_id()`) |
| `src-tauri/src/service/catalog/harness/claude.rs` | Claude Code renderer for all five kinds, scan script, scan parser, installed-asset enumeration |
| `src-tauri/src/service/catalog/harness/codex.rs` | Codex renderer for skill and mcp_server; no scanning |
| `src-tauri/src/service/catalog/repo.rs` | Local git clone/pull/head via `git` CLI, walking the repo into a `Catalog` with per-file problems, writing an asset back to disk |
| `src-tauri/src/service/catalog/inventory.rs` | Running a harness scan script on a host, computing per-asset states from a `HostSnapshot`, persisting rows |
| `src-tauri/src/service/catalog/import.rs` | Converting a Claude config directory into IR assets in the repo working tree, with dry-run and secret flagging |
| `src-tauri/src/commands/assets.rs` | Tauri IPC wrappers |
| `src-tauri/migrations/018_asset_catalog.sql` | `catalog_config` and `asset_inventory` tables |

Modified backend files: `src-tauri/Cargo.toml`, `src-tauri/src/service/mod.rs`, `src-tauri/src/store.rs`, `src-tauri/src/events.rs`, `src-tauri/src/service/health.rs` (test assertion), `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/mcp/tools.rs`, `docs/control-api-reference.md`.

New frontend files: `src/lib/assets.ts` (+ `assets.test.ts`), `src/lib/AssetsPanel.svelte` (+ `AssetsPanel.test.ts`), `src/lib/AssetList.svelte`, `src/lib/AssetDetail.svelte`, `src/lib/ImportDialog.svelte`. Modified: `src/lib/events.ts`, `src/lib/events.test.ts`, `src/App.svelte`.

Docs: `docs/concepts.md`, `docs/control-api.md`.

---

### Task 1: Dependencies, module skeleton, and the IR model

**Files:**
- Modify: `src-tauri/Cargo.toml` (dependencies block)
- Modify: `src-tauri/src/service/mod.rs`
- Create: `src-tauri/src/service/catalog/mod.rs`
- Create: `src-tauri/src/service/catalog/model.rs`
- Create: `src-tauri/src/service/catalog/harness/mod.rs` (stub only in this task; filled in Task 2)

**Interfaces:**
- Produces: `model::{Kind, Asset, Header, AssetSpec, HookMatch, HookAction, Marketplace, TargetOverride, Source, Resource, Problem}`, `Asset::from_yaml(kind_dir_hint: Option<Kind>, yaml: &str) -> Result<Asset, String>`, `Asset::to_yaml(&self) -> String`, `Asset::validate(&self) -> Vec<String>`, `model::sha256_hex(&[u8]) -> String`, `model::find_placeholders(&str) -> Vec<String>`, constants `TOOLS`, `TIERS`, `EVENTS`.

- [ ] **Step 1: Add dependencies**

In `src-tauri/Cargo.toml`, inside `[dependencies]` after `dirs = "5"`:

```toml
# Asset catalog (docs/superpowers/specs/2026-09-14-asset-catalog-design.md)
serde_yaml = "0.9"
sha2 = "0.10"
hex = "0.4"
base64 = "0.22"
```

- [ ] **Step 2: Register the module**

In `src-tauri/src/service/mod.rs` add `pub mod catalog;` between `pub mod bg_sessions;` and `pub mod clipboard;`.

Create `src-tauri/src/service/catalog/mod.rs`:

```rust
//! Asset catalog: harness-neutral skills / agents / hooks / MCP servers /
//! plugin refs kept in a git repo, rendered per harness, and compared with
//! what each host actually has installed.
//! Spec: docs/superpowers/specs/2026-09-14-asset-catalog-design.md

pub mod harness;
pub mod model;

pub const E_CATALOG_NOT_CONFIGURED: &str = "E_CATALOG_NOT_CONFIGURED";
pub const E_CATALOG_GIT: &str = "E_CATALOG_GIT";
pub const E_CATALOG_PARSE: &str = "E_CATALOG_PARSE";
pub const E_ASSET_UNSUPPORTED: &str = "E_ASSET_UNSUPPORTED";
pub const E_ASSET_EXISTS: &str = "E_ASSET_EXISTS";
pub const E_ASSET_NOT_FOUND: &str = "E_ASSET_NOT_FOUND";
```

Create `src-tauri/src/service/catalog/harness/mod.rs` as an empty file with only a doc comment `//! Per-harness renderers. Filled in Task 2.` so the crate compiles.

- [ ] **Step 3: Write the failing model tests**

Create `src-tauri/src/service/catalog/model.rs` with only the test module first:

```rust
//! The intermediate representation (IR) for catalog assets.

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
            AssetSpec::Skill { allowed_tools, user_invocable, triggers } => {
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
            AssetSpec::Hook { event, r#match, action } => {
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
        assert!(problems.iter().any(|p| p.contains("description")), "{problems:?}");

        let hook = Asset::from_yaml(
            None,
            "kind: hook\nname: x\ndescription: d\nevent: on_fire\naction: { type: smoke }\n",
        )
        .unwrap();
        let problems = hook.validate();
        assert!(problems.iter().any(|p| p.contains("event")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("action.type")), "{problems:?}");

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
    fn kind_dirs_and_names() {
        assert_eq!(Kind::Skill.dir(), "skills");
        assert_eq!(Kind::McpServer.dir(), "mcp");
        assert_eq!(Kind::PluginRef.dir(), "plugins");
        assert_eq!(Kind::from_dir("agents"), Some(Kind::Agent));
        assert_eq!(Kind::Hook.as_str(), "hook");
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cd src-tauri && cargo test service::catalog::model -- --nocapture`
Expected: compile errors (`Asset`, `Kind` … not found).

- [ ] **Step 5: Implement the model**

Insert above the `#[cfg(test)]` block in `model.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Neutral tool vocabulary (spec: "Neutral vocabularies").
pub const TOOLS: &[&str] = &[
    "read", "edit", "write", "bash", "grep", "glob", "web_search", "web_fetch", "browser",
    "agent", "*",
];
/// Neutral model tiers.
pub const TIERS: &[&str] = &["fast", "default", "strong"];
/// Neutral hook events.
pub const EVENTS: &[&str] = &[
    "session_start", "prompt_submit", "before_tool", "after_tool", "stop", "subagent_stop",
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
    pub const ALL: [Kind; 5] = [Kind::Skill, Kind::Agent, Kind::Hook, Kind::McpServer, Kind::PluginRef];

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
        Self { enabled: true, model: None, render_as: None, extra: BTreeMap::new() }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    #[serde(flatten)]
    pub header: Header,
    #[serde(flatten)]
    pub spec: AssetSpec,
    /// `body.md` (skill) or `prompt.md` (agent). Empty for other kinds.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub resources: Vec<Resource>,
}

/// Serde wrapper used only for the on-disk `asset.yaml` (no body/resources).
#[derive(Serialize, Deserialize)]
struct AssetFile {
    #[serde(flatten)]
    header: Header,
    #[serde(flatten)]
    spec: AssetSpec,
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
        Ok(Asset { header: file.header, spec: file.spec, body: String::new(), resources: vec![] })
    }

    /// Serialise the YAML part (header + spec). Body and resources are files.
    pub fn to_yaml(&self) -> String {
        let file = AssetFile { header: self.header.clone(), spec: self.spec.clone() };
        // A struct with two flattened parts always serialises; unwrap is safe.
        serde_yaml::to_string(&file).unwrap_or_default()
    }

    pub fn kind(&self) -> Kind {
        self.header.kind
    }

    /// Resolve the effective override for a harness (default when absent).
    pub fn target(&self, harness: &str) -> TargetOverride {
        self.header.targets.get(harness).cloned().unwrap_or_default()
    }

    /// Return every validation problem (empty means valid).
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !is_valid_name(&self.header.name) {
            out.push(format!("name '{}' must match [a-z0-9][a-z0-9-]*", self.header.name));
        }
        if self.header.description.trim().is_empty() {
            out.push("description must not be empty".into());
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
                    "command" if action.command.is_none() => {
                        out.push("action.command is required for type command".into())
                    }
                    "http" if action.url.is_none() => {
                        out.push("action.url is required for type http".into())
                    }
                    "command" | "http" => {}
                    other => out.push(format!("action.type '{other}' must be command or http")),
                }
            }
            AssetSpec::McpServer { transport, url, command, .. } => match transport.as_str() {
                "http" if url.is_none() => out.push("url is required for transport http".into()),
                "stdio" if command.is_none() => {
                    out.push("command is required for transport stdio".into())
                }
                "http" | "stdio" => {}
                other => out.push(format!("transport '{other}' must be http or stdio")),
            },
            AssetSpec::PluginRef { harness, version, .. } => {
                if harness != "claude" {
                    out.push(format!("harness '{harness}' is not supported for plugin refs"));
                }
                if version.trim().is_empty() {
                    out.push("version must be an exact version or 'latest'".into());
                }
            }
        }
        out
    }
}

pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
                    && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
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
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd src-tauri && cargo test service::catalog::model`
Expected: 6 tests pass. If `serde(flatten)` with the internally tagged enum fails to deserialise (`missing field kind`), replace the two `#[serde(flatten)]` lines in `AssetFile`/`Asset` by a manual `Deserialize` that first reads the document into `serde_yaml::Value`, reads `kind`, then deserialises `Header` and `AssetSpec` from the same value; keep the tests unchanged.

- [ ] **Step 7: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add Cargo.toml Cargo.lock src/service/mod.rs src/service/catalog
git commit -m "feat(catalog): IR model for skills, agents, hooks, MCP servers and plugin refs

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 2: Render plan, host snapshot, and the Harness trait

**Files:**
- Modify: `src-tauri/src/service/catalog/harness/mod.rs` (replace the stub)

**Interfaces:**
- Consumes: `model::{Asset, Kind, sha256_hex, find_placeholders}`.
- Produces:
  - `FileWrite { path: String, bytes: Vec<u8> }`
  - `MergeMode { Set, AppendUnique, Subset }`
  - `ConfigMerge { file: String, json_path: Vec<String>, mode: MergeMode, value: serde_json::Value }`
  - `RenderPlan { files, merges, placeholders, warnings }` with `RenderPlan::hash(&self) -> String` and `RenderPlan::note_placeholders(&mut self, text: &str)`
  - `HostSnapshot { files: BTreeMap<String, String>, configs: BTreeMap<String, serde_json::Value> }`
  - `Unsupported { harness: &'static str, kind: Kind }`
  - `trait Harness: Send + Sync { fn id(&self) -> &'static str; fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported>; fn scan_script(&self) -> Option<String>; fn parse_scan(&self, stdout: &str) -> Result<HostSnapshot, IpcError>; fn installed(&self, snap: &HostSnapshot) -> Vec<(Kind, String)>; }`
  - `all() -> Vec<Box<dyn Harness>>`, `by_id(&str) -> Option<Box<dyn Harness>>`, `HARNESS_IDS: &[&str]`
  - `json_get<'a>(root: &'a serde_json::Value, path: &[String]) -> Option<&'a serde_json::Value>`

- [ ] **Step 1: Write the failing tests**

Replace `harness/mod.rs` with:

```rust
//! Per-harness renderers. A harness turns an IR asset into a `RenderPlan`
//! (files to write + JSON merges into config files), knows how to scan a
//! host for what is installed, and can list the assets it finds there.

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_with(files: Vec<(&str, &str)>, merges: Vec<(&str, Vec<&str>, serde_json::Value)>) -> RenderPlan {
        let mut p = RenderPlan::default();
        for (path, body) in files {
            p.files.push(FileWrite { path: path.into(), bytes: body.as_bytes().to_vec() });
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
        let m1 = plan_with(vec![], vec![("~/.claude.json", vec!["mcpServers", "x"], serde_json::json!({"a":1,"b":2}))]);
        let m2 = plan_with(vec![], vec![("~/.claude.json", vec!["mcpServers", "x"], serde_json::json!({"b":2,"a":1}))]);
        assert_eq!(m1.hash(), m2.hash(), "canonical JSON: key order must not matter");
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
        assert_eq!(json_get(&v, &["a".into(), "b".into()]), Some(&serde_json::json!([1, 2])));
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test service::catalog::harness::tests`
Expected: compile errors (types not defined).

- [ ] **Step 3: Implement**

Insert above the test module:

```rust
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
            format!("{} cannot render {} assets", self.harness, self.kind.as_str()),
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
```

Create placeholder files so the registry compiles (filled in Tasks 3 and 4):

`harness/claude.rs`:
```rust
//! Claude Code renderer. Filled in Task 3.
use super::{Harness, HostSnapshot, RenderPlan, Unsupported};
use crate::ipc_error::IpcError;
use crate::service::catalog::model::{Asset, Kind};

pub struct Claude;

impl Harness for Claude {
    fn id(&self) -> &'static str { "claude" }
    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported> {
        Err(Unsupported { harness: "claude", kind: asset.kind() })
    }
    fn scan_script(&self) -> Option<String> { None }
    fn parse_scan(&self, _stdout: &str) -> Result<HostSnapshot, IpcError> { Ok(HostSnapshot::default()) }
    fn installed(&self, _snap: &HostSnapshot) -> Vec<(Kind, String)> { vec![] }
}
```

`harness/codex.rs`: identical with `Codex` / `"codex"`.

- [ ] **Step 4: Run to verify pass**

Run: `cd src-tauri && cargo test service::catalog::harness::tests`
Expected: 4 tests pass.

- [ ] **Step 5: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/service/catalog/harness
git commit -m "feat(catalog): render plan, host snapshot and Harness trait

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 3: Claude Code renderer

**Files:**
- Modify: `src-tauri/src/service/catalog/harness/claude.rs` (replace the stub's `render`)

**Interfaces:**
- Consumes: Task 1 model, Task 2 plan types.
- Produces: `Claude::render` for all five kinds; helpers `pub fn map_tool(neutral: &str) -> String`, `pub fn map_tier(tier: &str) -> &'static str`, `pub fn map_event(event: &str) -> Option<&'static str>`, `pub fn unmap_tool(claude: &str) -> Option<&'static str>`, `pub fn unmap_tier(model: &str) -> Option<&'static str>`, `pub fn unmap_event(claude: &str) -> Option<&'static str>`, `pub fn frontmatter(fields: &[(&str, serde_yaml::Value)]) -> String`, `pub const SETTINGS_PATH: &str = "~/.claude/settings.json"`, `pub const CLAUDE_JSON_PATH: &str = "~/.claude.json"`, `pub const PLUGINS_PATH: &str = "~/.claude/plugins/installed_plugins.json"`, `pub const SKILLS_DIR: &str = "~/.claude/skills"`, `pub const AGENTS_DIR: &str = "~/.claude/agents"`.

- [ ] **Step 1: Write the failing golden tests**

Append to `claude.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::{MergeMode, RenderPlan};
    use crate::service::catalog::model::{Asset, Resource};

    fn render(yaml: &str, body: &str) -> RenderPlan {
        let mut a = Asset::from_yaml(None, yaml).expect("parse");
        a.body = body.to_string();
        Claude.render(&a).expect("render")
    }

    #[test]
    fn skill_renders_skill_md_and_resources() {
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: worktree\ndescription: Make a worktree.\nallowed_tools: [bash, read]\ntargets:\n  claude:\n    extra:\n      disable-model-invocation: true\n",
        )
        .unwrap();
        a.body = "# Worktree\n\nDo it.\n".into();
        a.resources.push(Resource { rel_path: "resources/scripts/go.sh".into(), bytes: b"echo hi\n".to_vec() });
        let plan = Claude.render(&a).unwrap();
        assert_eq!(plan.files.len(), 2);
        let skill = plan.files.iter().find(|f| f.path == "~/.claude/skills/worktree/SKILL.md").unwrap();
        let text = String::from_utf8(skill.bytes.clone()).unwrap();
        assert_eq!(
            text,
            "---\nname: worktree\ndescription: Make a worktree.\nallowed-tools: Bash, Read\ndisable-model-invocation: true\n---\n# Worktree\n\nDo it.\n"
        );
        let res = plan.files.iter().find(|f| f.path == "~/.claude/skills/worktree/scripts/go.sh").unwrap();
        assert_eq!(res.bytes, b"echo hi\n");
        assert!(plan.merges.is_empty());
    }

    #[test]
    fn skill_triggers_fold_into_description() {
        let plan = render(
            "kind: skill\nname: s\ndescription: Base.\ntriggers: [\"foo\", \"bar\"]\n",
            "b\n",
        );
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        assert!(text.contains("description: Base. Triggers: foo, bar"), "{text}");
    }

    #[test]
    fn agent_renders_markdown_with_mapped_tools_and_model() {
        let plan = render(
            "kind: agent\nname: pm-qa\ndescription: QA lens.\ntools: [read, grep, bash]\nmodel: strong\n",
            "You are QA.\n",
        );
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "~/.claude/agents/pm-qa.md");
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        assert_eq!(
            text,
            "---\nname: pm-qa\ndescription: QA lens.\ntools: Read, Grep, Bash\nmodel: opus\n---\nYou are QA.\n"
        );
    }

    #[test]
    fn agent_explicit_model_override_wins() {
        let plan = render(
            "kind: agent\nname: a\ndescription: d\nmodel: fast\ntargets:\n  claude:\n    model: claude-haiku-4-5-20251001\n",
            "p\n",
        );
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        assert!(text.contains("model: claude-haiku-4-5-20251001"), "{text}");
    }

    #[test]
    fn hook_renders_append_unique_merge_into_settings() {
        let plan = render(
            "kind: hook\nname: rtk\ndescription: d\nevent: before_tool\nmatch: { tool: bash }\naction:\n  type: command\n  command: \"exec rtk hook claude\"\n  timeout_s: 5\n",
            "",
        );
        assert!(plan.files.is_empty());
        assert_eq!(plan.merges.len(), 1);
        let m = &plan.merges[0];
        assert_eq!(m.file, SETTINGS_PATH);
        assert_eq!(m.json_path, vec!["hooks", "PreToolUse"]);
        assert_eq!(m.mode, MergeMode::AppendUnique);
        assert_eq!(
            m.value,
            serde_json::json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "exec rtk hook claude", "timeout": 5}]})
        );
    }

    #[test]
    fn http_hook_keeps_placeholders_and_reports_them() {
        let plan = render(
            "kind: hook\nname: fleet-stop\ndescription: d\nevent: stop\naction:\n  type: http\n  url: \"http://127.0.0.1:${FLEET_MCP_PORT}/hook\"\n  headers: { Authorization: \"Bearer ${FLEET_MCP_TOKEN}\" }\n",
            "",
        );
        let m = &plan.merges[0];
        assert_eq!(m.json_path, vec!["hooks", "Stop"]);
        assert_eq!(
            m.value,
            serde_json::json!({"hooks": [{"type": "http", "url": "http://127.0.0.1:${FLEET_MCP_PORT}/hook", "headers": {"Authorization": "Bearer ${FLEET_MCP_TOKEN}"}}]})
        );
        assert_eq!(plan.placeholders, vec!["FLEET_MCP_PORT", "FLEET_MCP_TOKEN"]);
    }

    #[test]
    fn mcp_http_and_stdio_render_set_merges() {
        let http = render(
            "kind: mcp_server\nname: claude-fleet\ndescription: d\ntransport: http\nurl: http://127.0.0.1:4180/mcp\nheaders: { Authorization: \"Bearer ${T}\" }\n",
            "",
        );
        let m = &http.merges[0];
        assert_eq!(m.file, CLAUDE_JSON_PATH);
        assert_eq!(m.json_path, vec!["mcpServers", "claude-fleet"]);
        assert_eq!(m.mode, MergeMode::Set);
        assert_eq!(m.value, serde_json::json!({"type": "http", "url": "http://127.0.0.1:4180/mcp", "headers": {"Authorization": "Bearer ${T}"}}));
        let stdio = render(
            "kind: mcp_server\nname: jira\ndescription: d\ntransport: stdio\ncommand: npx\nargs: [\"-y\", \"jira-mcp\"]\nenv: { JIRA_TOKEN: \"${JIRA_TOKEN}\" }\n",
            "",
        );
        assert_eq!(stdio.merges[0].value, serde_json::json!({"type": "stdio", "command": "npx", "args": ["-y", "jira-mcp"], "env": {"JIRA_TOKEN": "${JIRA_TOKEN}"}}));
    }

    #[test]
    fn plugin_ref_renders_subset_merge_on_installed_plugins() {
        let pinned = render(
            "kind: plugin_ref\nname: superpowers\ndescription: d\nharness: claude\nmarketplace: { name: superpowers-marketplace, source: github, repo: obra/superpowers-marketplace }\nplugin: superpowers\nversion: \"6.3.0\"\n",
            "",
        );
        let m = &pinned.merges[0];
        assert_eq!(m.file, PLUGINS_PATH);
        assert_eq!(m.json_path, vec!["plugins", "superpowers@superpowers-marketplace"]);
        assert_eq!(m.mode, MergeMode::Subset);
        assert_eq!(m.value, serde_json::json!([{"version": "6.3.0"}]));
        let latest = render(
            "kind: plugin_ref\nname: superpowers\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: superpowers\nversion: latest\n",
            "",
        );
        assert_eq!(latest.merges[0].value, serde_json::json!([{}]));
    }

    #[test]
    fn disabled_target_yields_empty_plan_with_warning() {
        let plan = render("kind: skill\nname: s\ndescription: d\ntargets:\n  claude:\n    enabled: false\n", "b");
        assert!(plan.files.is_empty());
        assert_eq!(plan.warnings, vec!["disabled for claude by targets.claude.enabled"]);
    }

    #[test]
    fn vocab_maps_round_trip() {
        assert_eq!(map_tool("read"), "Read");
        assert_eq!(map_tool("mcp:claude-fleet"), "mcp__claude-fleet__*");
        assert_eq!(map_tool("*"), "*");
        assert_eq!(unmap_tool("WebFetch"), Some("web_fetch"));
        assert_eq!(unmap_tool("Weird"), None);
        assert_eq!(map_tier("fast"), "haiku");
        assert_eq!(unmap_tier("opus"), Some("strong"));
        assert_eq!(map_event("after_tool"), Some("PostToolUse"));
        assert_eq!(unmap_event("UserPromptSubmit"), Some("prompt_submit"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test service::catalog::harness::claude`
Expected: tests fail (`render` returns `Unsupported`; helper fns missing).

- [ ] **Step 3: Implement**

Replace the stub body of `claude.rs` (keep the test module) with:

```rust
//! Claude Code renderer, scanner and installed-asset enumeration.

use super::{ConfigMerge, FileWrite, Harness, HostSnapshot, MergeMode, RenderPlan, Unsupported};
use crate::ipc_error::IpcError;
use crate::service::catalog::model::{Asset, AssetSpec, Kind};
use serde_json::{json, Value};

pub const SETTINGS_PATH: &str = "~/.claude/settings.json";
pub const CLAUDE_JSON_PATH: &str = "~/.claude.json";
pub const PLUGINS_PATH: &str = "~/.claude/plugins/installed_plugins.json";
pub const SKILLS_DIR: &str = "~/.claude/skills";
pub const AGENTS_DIR: &str = "~/.claude/agents";

const TOOL_MAP: &[(&str, &str)] = &[
    ("read", "Read"),
    ("edit", "Edit"),
    ("write", "Write"),
    ("bash", "Bash"),
    ("grep", "Grep"),
    ("glob", "Glob"),
    ("web_search", "WebSearch"),
    ("web_fetch", "WebFetch"),
    ("browser", "mcp__plugin_superpowers-chrome_chrome__use_browser"),
    ("agent", "Agent"),
    ("*", "*"),
];
const TIER_MAP: &[(&str, &str)] = &[("fast", "haiku"), ("default", "sonnet"), ("strong", "opus")];
const EVENT_MAP: &[(&str, &str)] = &[
    ("session_start", "SessionStart"),
    ("prompt_submit", "UserPromptSubmit"),
    ("before_tool", "PreToolUse"),
    ("after_tool", "PostToolUse"),
    ("stop", "Stop"),
    ("subagent_stop", "SubagentStop"),
];

pub fn map_tool(neutral: &str) -> String {
    if let Some(server) = neutral.strip_prefix("mcp:") {
        return format!("mcp__{server}__*");
    }
    TOOL_MAP
        .iter()
        .find(|(n, _)| *n == neutral)
        .map(|(_, c)| c.to_string())
        .unwrap_or_else(|| neutral.to_string())
}
pub fn unmap_tool(claude: &str) -> Option<&'static str> {
    TOOL_MAP.iter().find(|(_, c)| *c == claude).map(|(n, _)| *n)
}
pub fn map_tier(tier: &str) -> &'static str {
    TIER_MAP.iter().find(|(t, _)| *t == tier).map(|(_, m)| *m).unwrap_or("sonnet")
}
pub fn unmap_tier(model: &str) -> Option<&'static str> {
    TIER_MAP.iter().find(|(_, m)| *m == model).map(|(t, _)| *t)
}
pub fn map_event(event: &str) -> Option<&'static str> {
    EVENT_MAP.iter().find(|(n, _)| *n == event).map(|(_, c)| *c)
}
pub fn unmap_event(claude: &str) -> Option<&'static str> {
    EVENT_MAP.iter().find(|(_, c)| *c == claude).map(|(n, _)| *n)
}

/// Render a YAML frontmatter block. Values are emitted with serde_yaml so
/// strings with `:` are quoted correctly; scalars stay on one line.
pub fn frontmatter(fields: &[(&str, serde_yaml::Value)]) -> String {
    let mut map = serde_yaml::Mapping::new();
    for (k, v) in fields {
        map.insert(serde_yaml::Value::String(k.to_string()), v.clone());
    }
    let body = serde_yaml::to_string(&serde_yaml::Value::Mapping(map)).unwrap_or_default();
    format!("---\n{body}---\n")
}

fn yaml_str(s: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(s.to_string())
}

fn json_to_yaml(v: &Value) -> serde_yaml::Value {
    serde_yaml::to_value(v).unwrap_or(serde_yaml::Value::Null)
}

pub struct Claude;

impl Claude {
    fn render_skill(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::Skill { allowed_tools, triggers, .. } = &a.spec else { return };
        let t = a.target("claude");
        let mut description = a.header.description.clone();
        if !triggers.is_empty() {
            description = format!("{} Triggers: {}", description.trim_end(), triggers.join(", "));
        }
        let mut fields: Vec<(&str, serde_yaml::Value)> = vec![
            ("name", yaml_str(&a.header.name)),
            ("description", yaml_str(&description)),
        ];
        if !allowed_tools.is_empty() {
            let mapped: Vec<String> = allowed_tools.iter().map(|x| map_tool(x)).collect();
            fields.push(("allowed-tools", yaml_str(&mapped.join(", "))));
        }
        for (k, v) in &t.extra {
            fields.push((k.as_str(), json_to_yaml(v)));
        }
        let text = format!("{}{}", frontmatter(&fields), a.body);
        plan.note_placeholders(&text);
        let dir = format!("{SKILLS_DIR}/{}", a.header.name);
        plan.files.push(FileWrite { path: format!("{dir}/SKILL.md"), bytes: text.into_bytes() });
        for r in &a.resources {
            let rel = r.rel_path.strip_prefix("resources/").unwrap_or(&r.rel_path);
            plan.files.push(FileWrite { path: format!("{dir}/{rel}"), bytes: r.bytes.clone() });
        }
    }

    fn render_agent(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::Agent { tools, model } = &a.spec else { return };
        let t = a.target("claude");
        let mut fields: Vec<(&str, serde_yaml::Value)> = vec![
            ("name", yaml_str(&a.header.name)),
            ("description", yaml_str(&a.header.description)),
        ];
        if !tools.is_empty() {
            let mapped: Vec<String> = tools.iter().map(|x| map_tool(x)).collect();
            fields.push(("tools", yaml_str(&mapped.join(", "))));
        }
        let model_id = t.model.clone().unwrap_or_else(|| map_tier(model).to_string());
        fields.push(("model", yaml_str(&model_id)));
        for (k, v) in &t.extra {
            fields.push((k.as_str(), json_to_yaml(v)));
        }
        let text = format!("{}{}", frontmatter(&fields), a.body);
        plan.note_placeholders(&text);
        plan.files.push(FileWrite { path: format!("{AGENTS_DIR}/{}.md", a.header.name), bytes: text.into_bytes() });
    }

    fn render_hook(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::Hook { event, r#match, action } = &a.spec else { return };
        let Some(claude_event) = map_event(event) else {
            plan.warnings.push(format!("unknown hook event '{event}'"));
            return;
        };
        let mut inner = serde_json::Map::new();
        inner.insert("type".into(), json!(action.kind));
        if let Some(c) = &action.command {
            inner.insert("command".into(), json!(c));
        }
        if let Some(u) = &action.url {
            inner.insert("url".into(), json!(u));
        }
        if !action.headers.is_empty() {
            inner.insert("headers".into(), json!(action.headers));
        }
        if let Some(t) = action.timeout_s {
            inner.insert("timeout".into(), json!(t));
        }
        for (k, v) in &a.target("claude").extra {
            inner.insert(k.clone(), v.clone());
        }
        let mut entry = serde_json::Map::new();
        if let Some(m) = r#match {
            entry.insert("matcher".into(), json!(map_tool(&m.tool)));
        }
        entry.insert("hooks".into(), json!([Value::Object(inner)]));
        let value = Value::Object(entry);
        plan.note_placeholders(&value.to_string());
        plan.merges.push(ConfigMerge {
            file: SETTINGS_PATH.into(),
            json_path: vec!["hooks".into(), claude_event.into()],
            mode: MergeMode::AppendUnique,
            value,
        });
    }

    fn render_mcp(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::McpServer { transport, url, headers, command, args, env } = &a.spec else { return };
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), json!(transport));
        if transport == "http" {
            obj.insert("url".into(), json!(url.clone().unwrap_or_default()));
            if !headers.is_empty() {
                obj.insert("headers".into(), json!(headers));
            }
        } else {
            obj.insert("command".into(), json!(command.clone().unwrap_or_default()));
            if !args.is_empty() {
                obj.insert("args".into(), json!(args));
            }
            if !env.is_empty() {
                obj.insert("env".into(), json!(env));
            }
        }
        for (k, v) in &a.target("claude").extra {
            obj.insert(k.clone(), v.clone());
        }
        let value = Value::Object(obj);
        plan.note_placeholders(&value.to_string());
        plan.merges.push(ConfigMerge {
            file: CLAUDE_JSON_PATH.into(),
            json_path: vec!["mcpServers".into(), a.header.name.clone()],
            mode: MergeMode::Set,
            value,
        });
    }

    fn render_plugin(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::PluginRef { marketplace, plugin, version, .. } = &a.spec else { return };
        let entry = if version == "latest" { json!({}) } else { json!({ "version": version }) };
        plan.merges.push(ConfigMerge {
            file: PLUGINS_PATH.into(),
            json_path: vec!["plugins".into(), format!("{plugin}@{}", marketplace.name)],
            mode: MergeMode::Subset,
            value: json!([entry]),
        });
    }
}

impl Harness for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported> {
        let mut plan = RenderPlan::default();
        if !asset.target("claude").enabled {
            plan.warnings.push("disabled for claude by targets.claude.enabled".into());
            return Ok(plan);
        }
        match asset.kind() {
            Kind::Skill => self.render_skill(asset, &mut plan),
            Kind::Agent => self.render_agent(asset, &mut plan),
            Kind::Hook => self.render_hook(asset, &mut plan),
            Kind::McpServer => self.render_mcp(asset, &mut plan),
            Kind::PluginRef => self.render_plugin(asset, &mut plan),
        }
        Ok(plan)
    }

    fn scan_script(&self) -> Option<String> {
        None // Task 6
    }

    fn parse_scan(&self, _stdout: &str) -> Result<HostSnapshot, IpcError> {
        Ok(HostSnapshot::default()) // Task 6
    }

    fn installed(&self, _snap: &HostSnapshot) -> Vec<(Kind, String)> {
        vec![] // Task 6
    }
}
```

Note on `Subset` for plugins: `installed_plugins.json` v2 stores each plugin key as an *array* of install records, so the merge value is `[{"version": …}]` and the inventory check (Task 6) treats an array value as "some element of the host array is a superset of some element of the value".

- [ ] **Step 4: Run to verify pass**

Run: `cd src-tauri && cargo test service::catalog::harness::claude`
Expected: 10 tests pass. If `serde_yaml` quotes `Make a worktree.` differently than the golden expects (it should not; plain strings without special characters are unquoted), adjust the implementation, not the golden.

- [ ] **Step 5: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/service/catalog/harness/claude.rs
git commit -m "feat(catalog): Claude Code renderer for all asset kinds

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 4: Codex renderer (skills + MCP, experimental)

**Files:**
- Modify: `src-tauri/src/service/catalog/harness/codex.rs` (replace the stub)

**Interfaces:**
- Consumes: Task 2 plan types, `claude::frontmatter`.
- Produces: `Codex::render` for `Kind::Skill` and `Kind::McpServer`; `Unsupported` for the rest unless `targets.codex.render_as == "skill"` on an agent. `pub const CODEX_SKILLS_DIR: &str = "~/.codex/skills"`, `pub const CODEX_CONFIG_PATH: &str = "~/.codex/config.toml"`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::MergeMode;
    use crate::service::catalog::model::Asset;

    #[test]
    fn skill_renders_to_codex_skills_dir() {
        let mut a = Asset::from_yaml(None, "kind: skill\nname: worktree\ndescription: Make one.\nallowed_tools: [bash]\n").unwrap();
        a.body = "body\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "~/.codex/skills/worktree/SKILL.md");
        assert_eq!(
            String::from_utf8(plan.files[0].bytes.clone()).unwrap(),
            "---\nname: worktree\ndescription: Make one.\n---\nbody\n"
        );
    }

    #[test]
    fn mcp_renders_toml_table_merge() {
        let a = Asset::from_yaml(None, "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: http://127.0.0.1:4180/mcp\n").unwrap();
        let plan = Codex.render(&a).unwrap();
        let m = &plan.merges[0];
        assert_eq!(m.file, CODEX_CONFIG_PATH);
        assert_eq!(m.json_path, vec!["mcp_servers", "fleet"]);
        assert_eq!(m.mode, MergeMode::Set);
        assert_eq!(m.value, serde_json::json!({"url": "http://127.0.0.1:4180/mcp"}));
        let s = Asset::from_yaml(None, "kind: mcp_server\nname: j\ndescription: d\ntransport: stdio\ncommand: npx\nargs: [x]\nenv: { A: b }\n").unwrap();
        assert_eq!(Codex.render(&s).unwrap().merges[0].value, serde_json::json!({"command": "npx", "args": ["x"], "env": {"A": "b"}}));
    }

    #[test]
    fn hooks_and_plugins_are_unsupported_agents_unless_render_as_skill() {
        let hook = Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        assert!(Codex.render(&hook).is_err());
        let plugin = Asset::from_yaml(None, "kind: plugin_ref\nname: p\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: p\nversion: latest\n").unwrap();
        assert!(Codex.render(&plugin).is_err());
        let agent = Asset::from_yaml(None, "kind: agent\nname: pm\ndescription: d\n").unwrap();
        assert!(Codex.render(&agent).is_err());
        let mut as_skill = Asset::from_yaml(None, "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    render_as: skill\n").unwrap();
        as_skill.body = "prompt\n".into();
        let plan = Codex.render(&as_skill).unwrap();
        assert_eq!(plan.files[0].path, "~/.codex/skills/pm/SKILL.md");
        assert_eq!(plan.warnings, vec!["agent rendered as a codex skill (targets.codex.render_as)"]);
    }

    #[test]
    fn no_scanning() {
        assert!(Codex.scan_script().is_none());
        assert!(Codex.installed(&Default::default()).is_empty());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test service::catalog::harness::codex`
Expected: failures (stub returns `Unsupported` for everything).

- [ ] **Step 3: Implement**

```rust
//! Codex CLI renderer (experimental): skills and MCP servers only.

use super::claude::frontmatter;
use super::{ConfigMerge, FileWrite, Harness, HostSnapshot, MergeMode, RenderPlan, Unsupported};
use crate::ipc_error::IpcError;
use crate::service::catalog::model::{Asset, AssetSpec, Kind};
use serde_json::json;

pub const CODEX_SKILLS_DIR: &str = "~/.codex/skills";
pub const CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";

pub struct Codex;

fn skill_file(name: &str, description: &str, body: &str, plan: &mut RenderPlan) {
    let fm = frontmatter(&[
        ("name", serde_yaml::Value::String(name.to_string())),
        ("description", serde_yaml::Value::String(description.to_string())),
    ]);
    let text = format!("{fm}{body}");
    plan.note_placeholders(&text);
    plan.files.push(FileWrite { path: format!("{CODEX_SKILLS_DIR}/{name}/SKILL.md"), bytes: text.into_bytes() });
}

impl Harness for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported> {
        let mut plan = RenderPlan::default();
        let t = asset.target("codex");
        if !t.enabled {
            plan.warnings.push("disabled for codex by targets.codex.enabled".into());
            return Ok(plan);
        }
        let unsupported = || Unsupported { harness: "codex", kind: asset.kind() };
        match &asset.spec {
            AssetSpec::Skill { .. } => {
                skill_file(&asset.header.name, &asset.header.description, &asset.body, &mut plan);
                let dir = format!("{CODEX_SKILLS_DIR}/{}", asset.header.name);
                for r in &asset.resources {
                    let rel = r.rel_path.strip_prefix("resources/").unwrap_or(&r.rel_path);
                    plan.files.push(FileWrite { path: format!("{dir}/{rel}"), bytes: r.bytes.clone() });
                }
            }
            AssetSpec::Agent { .. } if t.render_as.as_deref() == Some("skill") => {
                skill_file(&asset.header.name, &asset.header.description, &asset.body, &mut plan);
                plan.warnings.push("agent rendered as a codex skill (targets.codex.render_as)".into());
            }
            AssetSpec::McpServer { transport, url, headers, command, args, env } => {
                let mut obj = serde_json::Map::new();
                if transport == "http" {
                    obj.insert("url".into(), json!(url.clone().unwrap_or_default()));
                    if !headers.is_empty() {
                        obj.insert("http_headers".into(), json!(headers));
                    }
                } else {
                    obj.insert("command".into(), json!(command.clone().unwrap_or_default()));
                    if !args.is_empty() {
                        obj.insert("args".into(), json!(args));
                    }
                    if !env.is_empty() {
                        obj.insert("env".into(), json!(env));
                    }
                }
                for (k, v) in &t.extra {
                    obj.insert(k.clone(), v.clone());
                }
                let value = serde_json::Value::Object(obj);
                plan.note_placeholders(&value.to_string());
                plan.merges.push(ConfigMerge {
                    file: CODEX_CONFIG_PATH.into(),
                    json_path: vec!["mcp_servers".into(), asset.header.name.clone()],
                    mode: MergeMode::Set,
                    value,
                });
            }
            AssetSpec::Agent { .. } | AssetSpec::Hook { .. } | AssetSpec::PluginRef { .. } => {
                return Err(unsupported());
            }
        }
        Ok(plan)
    }

    fn scan_script(&self) -> Option<String> {
        None
    }

    fn parse_scan(&self, _stdout: &str) -> Result<HostSnapshot, IpcError> {
        Err(IpcError::new(super::super::E_ASSET_UNSUPPORTED, "codex hosts are not scanned in this version"))
    }

    fn installed(&self, _snap: &HostSnapshot) -> Vec<(Kind, String)> {
        vec![]
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd src-tauri && cargo test service::catalog::harness`
Expected: all harness tests pass (Tasks 2, 3, 4).

- [ ] **Step 5: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/service/catalog/harness/codex.rs
git commit -m "feat(catalog): experimental Codex renderer for skills and MCP servers

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 5: Catalog repo: git ops, loading, writing assets, process-wide state

**Files:**
- Create: `src-tauri/src/service/catalog/repo.rs`
- Modify: `src-tauri/src/service/catalog/mod.rs` (add `pub mod repo;` and `CatalogState`)

**Interfaces:**
- Consumes: Task 1 model.
- Produces:
  - `Catalog { assets: Vec<Asset>, problems: Vec<Problem>, head: String, loaded_at: i64 }` with `Catalog::find(&self, kind: Kind, name: &str) -> Option<&Asset>`
  - `load_dir(root: &Path) -> Result<Catalog, IpcError>` (parse only; `head` empty)
  - `ensure_repo(path: &Path, remote: Option<&str>) -> Result<(), IpcError>`
  - `pull(path: &Path) -> Result<(), IpcError>`
  - `head(path: &Path) -> Result<String, IpcError>`
  - `write_asset(root: &Path, asset: &Asset, overwrite: bool) -> Result<(), IpcError>`
  - `asset_path(root: &Path, kind: Kind, name: &str) -> PathBuf`
  - In `mod.rs`: `pub static CATALOG: once_cell::sync::Lazy<std::sync::RwLock<Option<Catalog>>>` and `pub fn now_secs() -> i64`.

- [ ] **Step 1: Write the failing tests**

Create `repo.rs` with tests only:

```rust
//! The catalog repo on the controller: git operations via the `git` CLI,
//! and loading / writing IR assets on disk.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::{Asset, Kind, Resource};
    use std::fs;

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fleet-catalog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(p: &std::path::Path, rel: &str, content: &str) {
        let f = p.join(rel);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, content).unwrap();
    }

    #[test]
    fn load_dir_reads_every_kind_and_collects_problems() {
        let root = tmp("load");
        write(&root, "catalog.yaml", "schema_version: 1\nname: test\n");
        write(&root, "skills/worktree/asset.yaml", "kind: skill\nname: worktree\ndescription: d\n");
        write(&root, "skills/worktree/body.md", "# body\n");
        write(&root, "skills/worktree/resources/scripts/go.sh", "echo\n");
        write(&root, "agents/pm/asset.yaml", "kind: agent\nname: pm\ndescription: d\n");
        write(&root, "agents/pm/prompt.md", "prompt\n");
        write(&root, "hooks/stop.yaml", "kind: hook\nname: stop\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        write(&root, "mcp/fleet.yaml", "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: u\n");
        write(&root, "plugins/sp.yaml", "kind: plugin_ref\nname: sp\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: sp\nversion: latest\n");
        write(&root, "hooks/bad.yaml", "kind: hook\nname: WRONG NAME\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        write(&root, "mcp/garbage.yaml", ": : not yaml [\n");
        write(&root, "skills/mismatch/asset.yaml", "kind: skill\nname: other\ndescription: d\n");

        let cat = load_dir(&root).unwrap();
        let names: Vec<(Kind, String)> = cat.assets.iter().map(|a| (a.kind(), a.header.name.clone())).collect();
        assert_eq!(
            names,
            vec![
                (Kind::Skill, "worktree".into()),
                (Kind::Agent, "pm".into()),
                (Kind::Hook, "stop".into()),
                (Kind::McpServer, "fleet".into()),
                (Kind::PluginRef, "sp".into()),
            ]
        );
        let skill = cat.find(Kind::Skill, "worktree").unwrap();
        assert_eq!(skill.body, "# body\n");
        assert_eq!(skill.resources.len(), 1);
        assert_eq!(skill.resources[0].rel_path, "resources/scripts/go.sh");
        assert_eq!(cat.find(Kind::Agent, "pm").unwrap().body, "prompt\n");
        assert_eq!(cat.problems.len(), 3, "{:?}", cat.problems);
        assert!(cat.problems.iter().any(|p| p.path.ends_with("hooks/bad.yaml") && p.message.contains("name")));
        assert!(cat.problems.iter().any(|p| p.path.ends_with("mcp/garbage.yaml")));
        assert!(cat.problems.iter().any(|p| p.path.ends_with("skills/mismatch/asset.yaml") && p.message.contains("stem")));
    }

    #[test]
    fn load_dir_rejects_missing_or_wrong_schema() {
        let root = tmp("schema");
        let err = load_dir(&root).unwrap_err();
        assert_eq!(err.code, "E_CATALOG_PARSE");
        write(&root, "catalog.yaml", "schema_version: 99\n");
        assert_eq!(load_dir(&root).unwrap_err().code, "E_CATALOG_PARSE");
    }

    #[test]
    fn write_asset_round_trips_and_refuses_overwrite() {
        let root = tmp("write");
        write(&root, "catalog.yaml", "schema_version: 1\n");
        let mut a = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        a.body = "b\n".into();
        a.resources.push(Resource { rel_path: "resources/x.txt".into(), bytes: b"x".to_vec() });
        write_asset(&root, &a, false).unwrap();
        assert!(root.join("skills/s/asset.yaml").exists());
        assert_eq!(fs::read_to_string(root.join("skills/s/body.md")).unwrap(), "b\n");
        assert_eq!(fs::read(root.join("skills/s/resources/x.txt")).unwrap(), b"x");
        let err = write_asset(&root, &a, false).unwrap_err();
        assert_eq!(err.code, "E_ASSET_EXISTS");
        write_asset(&root, &a, true).unwrap();
        let hook = Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        write_asset(&root, &hook, false).unwrap();
        assert!(root.join("hooks/h.yaml").exists());
        let cat = load_dir(&root).unwrap();
        assert_eq!(cat.assets.len(), 2);
        assert_eq!(cat.find(Kind::Skill, "s").unwrap().resources[0].bytes, b"x");
    }

    #[test]
    fn git_ensure_head_and_pull_on_a_local_repo() {
        let origin = tmp("origin");
        let run = |dir: &std::path::Path, args: &[&str]| {
            let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&origin, &["init", "-q", "-b", "main"]);
        run(&origin, &["config", "user.email", "t@t"]);
        run(&origin, &["config", "user.name", "t"]);
        write(&origin, "catalog.yaml", "schema_version: 1\n");
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-q", "-m", "init"]);

        let clone = std::env::temp_dir().join(format!("fleet-catalog-clone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&clone);
        ensure_repo(&clone, Some(origin.to_str().unwrap())).unwrap();
        assert!(clone.join(".git").exists());
        let h1 = head(&clone).unwrap();
        assert_eq!(h1.len(), 40);
        ensure_repo(&clone, Some(origin.to_str().unwrap())).unwrap(); // idempotent

        write(&origin, "hooks/x.yaml", "kind: hook\nname: x\ndescription: d\nevent: stop\naction: { type: command, command: x }\n");
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-q", "-m", "two"]);
        pull(&clone).unwrap();
        assert_ne!(head(&clone).unwrap(), h1);

        let missing = std::env::temp_dir().join("fleet-catalog-does-not-exist");
        let _ = fs::remove_dir_all(&missing);
        let err = ensure_repo(&missing, None).unwrap_err();
        assert_eq!(err.code, "E_CATALOG_GIT");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test service::catalog::repo`
Expected: compile errors.

- [ ] **Step 3: Implement**

Insert above the tests:

```rust
use super::model::{Asset, Kind, Problem, Resource};
use super::{E_ASSET_EXISTS, E_CATALOG_GIT, E_CATALOG_PARSE};
use crate::ipc_error::IpcError;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Catalog {
    pub assets: Vec<Asset>,
    pub problems: Vec<Problem>,
    pub head: String,
    pub loaded_at: i64,
}

impl Catalog {
    pub fn find(&self, kind: Kind, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.kind() == kind && a.header.name == name)
    }
}

#[derive(serde::Deserialize)]
struct CatalogFile {
    schema_version: u64,
}

fn git(dir: &Path, args: &[&str]) -> Result<String, IpcError> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| IpcError::new(E_CATALOG_GIT, format!("spawn git: {e}")))?;
    if !out.status.success() {
        return Err(IpcError::new(E_CATALOG_GIT, format!("git {}: failed", args.join(" ")))
            .with_details(serde_json::json!({ "stderr": String::from_utf8_lossy(&out.stderr).trim() })));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Clone `remote` into `path` when `path` has no `.git`. With no remote, the
/// directory must already be a git repo.
pub fn ensure_repo(path: &Path, remote: Option<&str>) -> Result<(), IpcError> {
    if path.join(".git").exists() {
        return Ok(());
    }
    match remote {
        Some(url) => {
            let parent = path.parent().unwrap_or(Path::new("."));
            std::fs::create_dir_all(parent)?;
            let target = path.to_string_lossy().to_string();
            git(parent, &["clone", "-q", url, &target])?;
            Ok(())
        }
        None => Err(IpcError::new(
            E_CATALOG_GIT,
            format!("{} is not a git repository and no remote URL is configured", path.display()),
        )),
    }
}

pub fn pull(path: &Path) -> Result<(), IpcError> {
    git(path, &["pull", "-q", "--ff-only"]).map(|_| ())
}

pub fn head(path: &Path) -> Result<String, IpcError> {
    git(path, &["rev-parse", "HEAD"])
}

pub fn asset_path(root: &Path, kind: Kind, name: &str) -> PathBuf {
    if kind.is_folder() {
        root.join(kind.dir()).join(name).join("asset.yaml")
    } else {
        root.join(kind.dir()).join(format!("{name}.yaml"))
    }
}

fn body_file(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "body.md",
        Kind::Agent => "prompt.md",
        _ => "",
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string()
}

fn read_resources(dir: &Path) -> std::io::Result<Vec<Resource>> {
    let mut out = Vec::new();
    let res = dir.join("resources");
    if !res.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![res];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(Resource { rel_path: rel(dir, &p), bytes: std::fs::read(&p)? });
            }
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

fn load_one(root: &Path, kind: Kind, yaml_path: &Path, stem: &str) -> Result<Asset, String> {
    let text = std::fs::read_to_string(yaml_path).map_err(|e| e.to_string())?;
    let mut asset = Asset::from_yaml(Some(kind), &text)?;
    if asset.header.name != stem {
        return Err(format!("name '{}' does not match the file/folder stem '{stem}'", asset.header.name));
    }
    let problems = asset.validate();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    if kind.is_folder() {
        let dir = yaml_path.parent().unwrap_or(root);
        let body = dir.join(body_file(kind));
        asset.body = std::fs::read_to_string(&body)
            .map_err(|_| format!("missing {}", body_file(kind)))?;
        asset.resources = read_resources(dir).map_err(|e| e.to_string())?;
    }
    Ok(asset)
}

/// Parse a catalog working tree. Never fails on a bad asset: those become
/// `problems`. Fails only when `catalog.yaml` is missing or has the wrong
/// schema version.
pub fn load_dir(root: &Path) -> Result<Catalog, IpcError> {
    let cat_file = root.join("catalog.yaml");
    let text = std::fs::read_to_string(&cat_file)
        .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("{}: {e}", cat_file.display())))?;
    let cf: CatalogFile = serde_yaml::from_str(&text)
        .map_err(|e| IpcError::new(E_CATALOG_PARSE, format!("catalog.yaml: {e}")))?;
    if cf.schema_version != SCHEMA_VERSION {
        return Err(IpcError::new(
            E_CATALOG_PARSE,
            format!("catalog.yaml schema_version {} is not supported (want {SCHEMA_VERSION})", cf.schema_version),
        ));
    }
    let mut cat = Catalog { loaded_at: super::now_secs(), ..Default::default() };
    for kind in Kind::ALL {
        let dir = root.join(kind.dir());
        if !dir.is_dir() {
            continue;
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let (yaml_path, stem) = if kind.is_folder() {
                if !p.is_dir() {
                    continue;
                }
                (p.join("asset.yaml"), p.file_name().unwrap_or_default().to_string_lossy().to_string())
            } else {
                if p.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                (p.clone(), p.file_stem().unwrap_or_default().to_string_lossy().to_string())
            };
            match load_one(root, kind, &yaml_path, &stem) {
                Ok(a) => cat.assets.push(a),
                Err(message) => cat.problems.push(Problem { path: rel(root, &yaml_path), message }),
            }
        }
    }
    Ok(cat)
}

/// Write an asset into the working tree (asset.yaml + body + resources).
pub fn write_asset(root: &Path, asset: &Asset, overwrite: bool) -> Result<(), IpcError> {
    let kind = asset.kind();
    let yaml_path = asset_path(root, kind, &asset.header.name);
    if yaml_path.exists() && !overwrite {
        return Err(IpcError::new(
            E_ASSET_EXISTS,
            format!("{} {} already exists in the catalog", kind.as_str(), asset.header.name),
        ));
    }
    let dir = yaml_path.parent().unwrap_or(root);
    std::fs::create_dir_all(dir)?;
    std::fs::write(&yaml_path, asset.to_yaml())?;
    if kind.is_folder() {
        std::fs::write(dir.join(body_file(kind)), &asset.body)?;
        for r in &asset.resources {
            let p = dir.join(&r.rel_path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(p, &r.bytes)?;
        }
    }
    Ok(())
}
```

In `catalog/mod.rs` add:

```rust
pub mod repo;

/// The loaded catalog, process-wide. `None` until `load` succeeds. Both the
/// Tauri commands and the MCP tools read it; only `load` writes it.
pub static CATALOG: once_cell::sync::Lazy<std::sync::RwLock<Option<repo::Catalog>>> =
    once_cell::sync::Lazy::new(|| std::sync::RwLock::new(None));

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `CATALOG` is process-global, so tests that write it must serialise.
#[cfg(test)]
pub static CATALOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

- [ ] **Step 4: Run to verify pass**

Run: `cd src-tauri && cargo test service::catalog::repo`
Expected: 4 tests pass (`git` must be on PATH).

- [ ] **Step 5: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/service/catalog
git commit -m "feat(catalog): load and write the catalog repo, git clone/pull/head

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 6: Store migration 018, inventory rows, and events

**Files:**
- Create: `src-tauri/migrations/018_asset_catalog.sql`
- Modify: `src-tauri/src/store.rs` (migrate arm, row structs, helpers, tests)
- Modify: `src-tauri/src/events.rs` (two new events on the trait and all three impls)
- Modify: `src-tauri/src/service/health.rs:233` (`17` → `18`)

**Interfaces:**
- Produces in `store.rs`:
  - `pub struct CatalogConfigRow { pub repo_path: String, pub remote_url: Option<String>, pub head_commit: Option<String>, pub last_loaded_at: Option<i64> }`
  - `pub struct AssetInventoryRow { pub host_alias: String, pub harness: String, pub kind: String, pub name: String, pub state: String, pub catalog_hash: Option<String>, pub host_hash: Option<String>, pub scanned_at: i64 }`
  - `Store::get_catalog_config(&self) -> Result<Option<CatalogConfigRow>>`
  - `Store::set_catalog_config(&self, repo_path: &str, remote_url: Option<&str>) -> Result<CatalogConfigRow>`
  - `Store::set_catalog_head(&self, head: &str, loaded_at: i64) -> Result<()>`
  - `Store::replace_host_inventory(&self, host_alias: &str, harness: &str, rows: &[AssetInventoryRow]) -> Result<()>` (transaction: delete + insert, then emit one `asset_inventory_updated` per row and `asset_inventory_cleared(host, harness)` first)
  - `Store::list_inventory(&self) -> Result<Vec<AssetInventoryRow>>`
- Produces in `events.rs`: `CatalogSummary { head: String, loaded_at: i64, asset_count: usize, problem_count: usize }`, `AssetInventoryClearedPayload { host_alias, harness }`, trait methods `asset_inventory_updated(&AssetInventoryRow)`, `asset_inventory_cleared(host_alias: &str, harness: &str)`, `catalog_loaded(&CatalogSummary)`; event names `asset_inventory:updated`, `asset_inventory:cleared`, `catalog:loaded`.

- [ ] **Step 1: Write the migration**

`src-tauri/migrations/018_asset_catalog.sql`:

```sql
-- Asset catalog (sub-project 1): where the catalog repo lives, and the
-- per-host / per-harness drift state of every catalog asset as of the last
-- scan. See docs/superpowers/specs/2026-09-14-asset-catalog-design.md.
CREATE TABLE IF NOT EXISTS catalog_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  repo_path TEXT NOT NULL,
  remote_url TEXT,
  head_commit TEXT,
  last_loaded_at INTEGER
);

CREATE TABLE IF NOT EXISTS asset_inventory (
  host_alias TEXT NOT NULL,
  harness TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  state TEXT NOT NULL,
  catalog_hash TEXT,
  host_hash TEXT,
  scanned_at INTEGER NOT NULL,
  PRIMARY KEY (host_alias, harness, kind, name)
);

INSERT OR IGNORE INTO schema_version (version) VALUES (18);
```

- [ ] **Step 2: Write the failing store tests**

In `store.rs` tests module, change both `assert_eq!(... schema_version ..., 17)` lines (around 2379 and 2771) to `18`, add `"catalog_config", "asset_inventory"` to `EXPECTED_TABLES`, and add:

```rust
    #[test]
    fn catalog_config_set_get_and_head() {
        let s = Store::open_in_memory().expect("open");
        assert!(s.get_catalog_config().unwrap().is_none());
        let row = s.set_catalog_config("/tmp/assets", Some("git@x:y.git")).unwrap();
        assert_eq!(row.repo_path, "/tmp/assets");
        assert_eq!(row.remote_url.as_deref(), Some("git@x:y.git"));
        assert!(row.head_commit.is_none());
        s.set_catalog_head("abc123", 42).unwrap();
        let row = s.get_catalog_config().unwrap().unwrap();
        assert_eq!(row.head_commit.as_deref(), Some("abc123"));
        assert_eq!(row.last_loaded_at, Some(42));
        // Re-configure replaces path and remote but keeps a single row.
        let row = s.set_catalog_config("/tmp/other", None).unwrap();
        assert_eq!(row.repo_path, "/tmp/other");
        assert!(row.remote_url.is_none());
    }

    #[test]
    fn replace_host_inventory_prunes_and_emits() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let dyn_bus: std::sync::Arc<dyn crate::events::EventBus> = bus.clone();
        let s = Store::open_with_bus_in_memory(dyn_bus).expect("open");
        let row = |name: &str, state: &str| AssetInventoryRow {
            host_alias: "local".into(),
            harness: "claude".into(),
            kind: "skill".into(),
            name: name.into(),
            state: state.into(),
            catalog_hash: Some("c".into()),
            host_hash: Some("h".into()),
            scanned_at: 1,
        };
        s.replace_host_inventory("local", "claude", &[row("a", "in_sync"), row("b", "missing")]).unwrap();
        assert_eq!(s.list_inventory().unwrap().len(), 2);
        let ev = bus.take();
        assert_eq!(ev[0], "asset_inventory:cleared:local:claude");
        assert!(ev.contains(&"asset_inventory:updated:local:claude:skill:a".to_string()));
        // A second scan that no longer sees `b` prunes it; another host is untouched.
        s.replace_host_inventory("mefistos", "claude", &[AssetInventoryRow { host_alias: "mefistos".into(), ..row("z", "drifted") }]).unwrap();
        s.replace_host_inventory("local", "claude", &[row("a", "drifted")]).unwrap();
        let all = s.list_inventory().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|r| r.host_alias == "local" && r.name == "a" && r.state == "drifted"));
        assert!(all.iter().any(|r| r.host_alias == "mefistos" && r.name == "z"));
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `cd src-tauri && cargo test store::tests::catalog_config_set_get_and_head store::tests::replace_host_inventory_prunes_and_emits store::tests::migrate_is_idempotent`
Expected: compile errors / `18 != 17`.

- [ ] **Step 4: Implement store and events**

In `store.rs` `migrate()` after the `if v < 17 { … }` block:

```rust
        if v < 18 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/018_asset_catalog.sql"))?;
            tx.commit()?;
        }
```

Row structs next to `HostRow`:

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogConfigRow {
    pub repo_path: String,
    pub remote_url: Option<String>,
    pub head_commit: Option<String>,
    pub last_loaded_at: Option<i64>,
}

/// Drift state of one catalog asset on one host for one harness
/// (migration 018). `state` is one of in_sync | drifted | missing |
/// unmanaged | unsupported.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AssetInventoryRow {
    pub host_alias: String,
    pub harness: String,
    pub kind: String,
    pub name: String,
    pub state: String,
    pub catalog_hash: Option<String>,
    pub host_hash: Option<String>,
    pub scanned_at: i64,
}
```

Helpers inside `impl Store` (next to the host helpers):

```rust
    pub fn get_catalog_config(&self) -> Result<Option<CatalogConfigRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT repo_path, remote_url, head_commit, last_loaded_at FROM catalog_config WHERE id = 1",
        )?;
        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(row) => Ok(Some(CatalogConfigRow {
                repo_path: row.get(0)?,
                remote_url: row.get(1)?,
                head_commit: row.get(2)?,
                last_loaded_at: row.get(3)?,
            })),
            None => Ok(None),
        }
    }

    pub fn set_catalog_config(
        &self,
        repo_path: &str,
        remote_url: Option<&str>,
    ) -> Result<CatalogConfigRow, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO catalog_config (id, repo_path, remote_url) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET repo_path=excluded.repo_path, remote_url=excluded.remote_url,
                                           head_commit=NULL, last_loaded_at=NULL",
            rusqlite::params![repo_path, remote_url],
        )?;
        Ok(self.get_catalog_config()?.expect("row just written"))
    }

    pub fn set_catalog_head(&self, head: &str, loaded_at: i64) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE catalog_config SET head_commit=?1, last_loaded_at=?2 WHERE id = 1",
            rusqlite::params![head, loaded_at],
        )?;
        Ok(())
    }

    /// Replace every inventory row for (host, harness) in one transaction,
    /// then emit `asset_inventory:cleared` followed by one `:updated` per row.
    pub fn replace_host_inventory(
        &self,
        host_alias: &str,
        harness: &str,
        rows: &[AssetInventoryRow],
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM asset_inventory WHERE host_alias=?1 AND harness=?2",
            rusqlite::params![host_alias, harness],
        )?;
        for r in rows {
            tx.execute(
                "INSERT INTO asset_inventory (host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    host_alias, harness, r.kind, r.name, r.state, r.catalog_hash, r.host_hash, r.scanned_at
                ],
            )?;
        }
        tx.commit()?;
        self.bus.asset_inventory_cleared(host_alias, harness);
        for r in rows {
            self.bus.asset_inventory_updated(r);
        }
        Ok(())
    }

    pub fn list_inventory(&self) -> Result<Vec<AssetInventoryRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT host_alias, harness, kind, name, state, catalog_hash, host_hash, scanned_at
             FROM asset_inventory ORDER BY host_alias, harness, kind, name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AssetInventoryRow {
                host_alias: row.get(0)?,
                harness: row.get(1)?,
                kind: row.get(2)?,
                name: row.get(3)?,
                state: row.get(4)?,
                catalog_hash: row.get(5)?,
                host_hash: row.get(6)?,
                scanned_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }
```

In `events.rs`: extend the `use crate::store::{…}` import with `AssetInventoryRow`, add payload structs and trait methods:

```rust
#[derive(Serialize, Clone)]
pub struct AssetInventoryClearedPayload {
    pub host_alias: String,
    pub harness: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CatalogSummary {
    pub head: String,
    pub loaded_at: i64,
    pub asset_count: usize,
    pub problem_count: usize,
}
```

Trait additions (in `EventBus`):

```rust
    fn asset_inventory_updated(&self, row: &AssetInventoryRow);
    fn asset_inventory_cleared(&self, host_alias: &str, harness: &str);
    fn catalog_loaded(&self, summary: &CatalogSummary);
```

`NoopEventBus`: three empty bodies. `AppHandleEventBus`:

```rust
    fn asset_inventory_updated(&self, row: &AssetInventoryRow) {
        self.queue("asset_inventory:updated", row);
    }
    fn asset_inventory_cleared(&self, host_alias: &str, harness: &str) {
        self.queue(
            "asset_inventory:cleared",
            &AssetInventoryClearedPayload { host_alias: host_alias.to_string(), harness: harness.to_string() },
        );
    }
    fn catalog_loaded(&self, summary: &CatalogSummary) {
        self.queue("catalog:loaded", summary);
    }
```

`RecordingEventBus`:

```rust
    fn asset_inventory_updated(&self, r: &AssetInventoryRow) {
        self.events.lock().unwrap().push(format!(
            "asset_inventory:updated:{}:{}:{}:{}",
            r.host_alias, r.harness, r.kind, r.name
        ));
    }
    fn asset_inventory_cleared(&self, host_alias: &str, harness: &str) {
        self.events.lock().unwrap().push(format!("asset_inventory:cleared:{host_alias}:{harness}"));
    }
    fn catalog_loaded(&self, s: &CatalogSummary) {
        self.events.lock().unwrap().push(format!("catalog:loaded:{}", s.head));
    }
```

Change `src-tauri/src/service/health.rs:233` to `assert_eq!(h.schema_version, 18);`.

- [ ] **Step 5: Run to verify pass**

Run: `cd src-tauri && cargo test store:: && cargo test service::health`
Expected: all pass, including `migrate_is_idempotent` at 18 and `open_in_memory_creates_all_tables`.

- [ ] **Step 6: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add migrations/018_asset_catalog.sql src/store.rs src/events.rs src/service/health.rs
git commit -m "feat(store): catalog_config and asset_inventory tables (migration 018) with row events

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 7: Claude host scan script, parser, and state computation

**Files:**
- Modify: `src-tauri/src/service/catalog/harness/claude.rs` (`scan_script`, `parse_scan`, `installed`, plus `pub fn hook_asset_name(event: &str, matcher: Option<&str>) -> String`)
- Create: `src-tauri/src/service/catalog/inventory.rs` (pure part)
- Modify: `src-tauri/src/service/catalog/mod.rs` (`pub mod inventory;`)

**Interfaces:**
- Consumes: Task 2 `HostSnapshot`, `RenderPlan`, `MergeMode`, `json_get`; Task 5 `Catalog`; Task 6 `AssetInventoryRow`.
- Produces:
  - `claude::scan_script()` returns `Some(script)`; `claude::parse_scan(stdout)` fills `HostSnapshot.files` (keys like `~/.claude/skills/x/SKILL.md`) and `HostSnapshot.configs` (keys `SETTINGS_PATH`, `CLAUDE_JSON_PATH`, `PLUGINS_PATH`).
  - `claude::installed(snap)` lists `(Kind, name)` for skills (folder names under `SKILLS_DIR`), agents (file stems under `AGENTS_DIR`), hooks (`hook_asset_name` per settings entry), MCP servers (`mcpServers` keys), plugins (`plugins` keys with `@marketplace` stripped).
  - `inventory::AssetState` enum with `as_str()`: `in_sync | drifted | missing | unmanaged | unsupported`.
  - `inventory::merge_satisfied(snap: &HostSnapshot, m: &ConfigMerge) -> bool`
  - `inventory::compute_states(catalog: &Catalog, harness: &dyn Harness, host_alias: &str, snap: &HostSnapshot, scanned_at: i64) -> Vec<AssetInventoryRow>`

- [ ] **Step 1: Write the failing tests (claude scan)**

Add to the `tests` module in `claude.rs`:

```rust
    const SCAN_OUT: &str = "##HASHES\n\
aaaa  .claude/skills/worktree/SKILL.md\n\
bbbb  .claude/skills/worktree/scripts/go.sh\n\
cccc  .claude/agents/pm-qa.md\n\
##CONFIG ~/.claude/settings.json\n\
eyJob29rcyI6eyJTdG9wIjpbeyJob29rcyI6W3sidHlwZSI6ImNvbW1hbmQiLCJjb21tYW5kIjoieCJ9XX1dLCJQcmVUb29sVXNlIjpbeyJtYXRjaGVyIjoiQmFzaCIsImhvb2tzIjpbXX1dfX0=\n\
##CONFIG ~/.claude.json\n\
eyJtY3BTZXJ2ZXJzIjp7ImNsYXVkZS1mbGVldCI6eyJ0eXBlIjoiaHR0cCJ9fX0=\n\
##CONFIG ~/.claude/plugins/installed_plugins.json\n\
eyJwbHVnaW5zIjp7InN1cGVycG93ZXJzQHN1cGVycG93ZXJzLW1hcmtldHBsYWNlIjpbeyJ2ZXJzaW9uIjoiNi4zLjAifV19fQ==\n\
##END\n";

    #[test]
    fn parse_scan_reads_hashes_and_configs() {
        let snap = Claude.parse_scan(SCAN_OUT).unwrap();
        assert_eq!(snap.files["~/.claude/skills/worktree/SKILL.md"], "aaaa");
        assert_eq!(snap.files["~/.claude/agents/pm-qa.md"], "cccc");
        assert_eq!(snap.configs[SETTINGS_PATH]["hooks"]["Stop"][0]["hooks"][0]["command"], "x");
        assert_eq!(snap.configs[CLAUDE_JSON_PATH]["mcpServers"]["claude-fleet"]["type"], "http");
        assert_eq!(snap.configs[PLUGINS_PATH]["plugins"]["superpowers@superpowers-marketplace"][0]["version"], "6.3.0");
    }

    #[test]
    fn parse_scan_tolerates_empty_or_invalid_config_blocks() {
        let snap = Claude.parse_scan("##HASHES\n##CONFIG ~/.claude/settings.json\n\n##CONFIG ~/.claude.json\nbm90IGpzb24=\n##END\n").unwrap();
        assert!(!snap.configs.contains_key(SETTINGS_PATH));
        assert!(!snap.configs.contains_key(CLAUDE_JSON_PATH));
    }

    #[test]
    fn installed_enumerates_every_kind() {
        let snap = Claude.parse_scan(SCAN_OUT).unwrap();
        let mut got = Claude.installed(&snap);
        got.sort();
        assert_eq!(
            got,
            vec![
                (Kind::Skill, "worktree".to_string()),
                (Kind::Agent, "pm-qa".to_string()),
                (Kind::Hook, "before-tool-bash".to_string()),
                (Kind::Hook, "stop".to_string()),
                (Kind::McpServer, "claude-fleet".to_string()),
                (Kind::PluginRef, "superpowers".to_string()),
            ]
        );
    }

    #[test]
    fn scan_script_is_home_relative_and_quoted() {
        let s = Claude.scan_script().unwrap();
        assert!(s.contains("##HASHES"));
        assert!(s.contains("##CONFIG ~/.claude/settings.json"));
        assert!(s.contains("base64"));
        assert!(!s.contains('\''), "no single quotes: the whole script is passed through shell::quote");
    }

    #[test]
    fn hook_names_are_stable() {
        assert_eq!(hook_asset_name("PreToolUse", Some("Bash")), "before-tool-bash");
        assert_eq!(hook_asset_name("Stop", None), "stop");
        assert_eq!(hook_asset_name("Stop", Some("")), "stop");
        assert_eq!(hook_asset_name("PostToolUse", Some("EnterWorktree|ExitWorktree")), "after-tool-enterworktree-exitworktree");
    }
```

The three base64 blocks decode to `{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}],"PreToolUse":[{"matcher":"Bash","hooks":[]}]}}`, `{"mcpServers":{"claude-fleet":{"type":"http"}}}` and `{"plugins":{"superpowers@superpowers-marketplace":[{"version":"6.3.0"}]}}`.

- [ ] **Step 2: Write the failing tests (inventory)**

Create `inventory.rs`:

```rust
//! Host inventory: run a harness scan on a host, compare against rendered
//! catalog assets, persist per-asset drift states.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::claude::Claude;
    use crate::service::catalog::harness::{ConfigMerge, Harness, HostSnapshot, MergeMode};
    use crate::service::catalog::model::{Asset, Kind};
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
        let set = ConfigMerge { file: "f".into(), json_path: vec!["a".into(), "b".into()], mode: MergeMode::Set, value: json!({"x": 1}) };
        assert!(merge_satisfied(&snap_with(vec![("f", json!({"a": {"b": {"x": 1}}}))]), &set));
        assert!(!merge_satisfied(&snap_with(vec![("f", json!({"a": {"b": {"x": 2}}}))]), &set));
        assert!(!merge_satisfied(&snap_with(vec![]), &set));

        let append = ConfigMerge { file: "f".into(), json_path: vec!["hooks".into(), "Stop".into()], mode: MergeMode::AppendUnique, value: json!({"hooks": [{"type": "command", "command": "x"}]}) };
        assert!(merge_satisfied(&snap_with(vec![("f", json!({"hooks": {"Stop": [{"other": 1}, {"hooks": [{"type": "command", "command": "x"}]}]}}))]), &append));
        assert!(!merge_satisfied(&snap_with(vec![("f", json!({"hooks": {"Stop": [{"other": 1}]}}))]), &append));

        let subset = ConfigMerge { file: "f".into(), json_path: vec!["plugins".into(), "p@m".into()], mode: MergeMode::Subset, value: json!([{"version": "1"}]) };
        assert!(merge_satisfied(&snap_with(vec![("f", json!({"plugins": {"p@m": [{"version": "1", "scope": "user"}]}}))]), &subset));
        assert!(!merge_satisfied(&snap_with(vec![("f", json!({"plugins": {"p@m": [{"version": "2"}]}}))]), &subset));
        let latest = ConfigMerge { value: json!([{}]), ..subset.clone() };
        assert!(merge_satisfied(&snap_with(vec![("f", json!({"plugins": {"p@m": [{"version": "2"}]}}))]), &latest));
    }

    #[test]
    fn compute_states_covers_all_five_states() {
        let mut cat = Catalog::default();
        let mut skill = Asset::from_yaml(None, "kind: skill\nname: s\ndescription: d\n").unwrap();
        skill.body = "b\n".into();
        cat.assets.push(skill.clone());
        cat.assets.push(Asset::from_yaml(None, "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: u\n").unwrap());
        cat.assets.push(Asset::from_yaml(None, "kind: agent\nname: gone\ndescription: d\n").unwrap());
        cat.assets.push(Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap());

        let claude = Claude;
        let skill_plan = claude.render(&skill).unwrap();
        let skill_hash = crate::service::catalog::model::sha256_hex(&skill_plan.files[0].bytes);
        let mut snap = HostSnapshot::default();
        snap.files.insert("~/.claude/skills/s/SKILL.md".into(), skill_hash);
        snap.files.insert("~/.claude/skills/extra/SKILL.md".into(), "zzz".into());
        snap.configs.insert("~/.claude.json".into(), json!({"mcpServers": {"fleet": {"type": "http", "url": "OTHER"}}}));
        snap.configs.insert("~/.claude/settings.json".into(), json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "x"}]}]}}));

        let rows = compute_states(&cat, &claude, "local", &snap, 7);
        let state = |kind: &str, name: &str| rows.iter().find(|r| r.kind == kind && r.name == name).map(|r| r.state.clone()).unwrap_or_default();
        assert_eq!(state("skill", "s"), "in_sync");
        assert_eq!(state("mcp_server", "fleet"), "drifted");
        assert_eq!(state("agent", "gone"), "missing");
        assert_eq!(state("hook", "h"), "in_sync");
        assert_eq!(state("skill", "extra"), "unmanaged");
        assert!(rows.iter().all(|r| r.host_alias == "local" && r.harness == "claude" && r.scanned_at == 7));
        let s = rows.iter().find(|r| r.name == "s").unwrap();
        assert!(s.catalog_hash.is_some() && s.host_hash.is_some());

        let codex = crate::service::catalog::harness::codex::Codex;
        let rows = compute_states(&cat, &codex, "local", &HostSnapshot::default(), 7);
        assert_eq!(rows.iter().find(|r| r.name == "gone").unwrap().state, "unsupported");
        assert_eq!(rows.iter().find(|r| r.name == "s").unwrap().state, "missing");
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cd src-tauri && cargo test service::catalog`
Expected: compile errors for `inventory` items and failing claude scan tests.

- [ ] **Step 4: Implement claude scan**

In `claude.rs` replace the three stub methods and add `hook_asset_name`:

```rust
/// Name of a hook asset derived from its Claude event and matcher, e.g.
/// `before-tool-bash`. Used by both the importer and `installed()` so the
/// two agree on identity.
pub fn hook_asset_name(claude_event: &str, matcher: Option<&str>) -> String {
    let event = unmap_event(claude_event).unwrap_or(claude_event).replace('_', "-").to_lowercase();
    let m: String = matcher
        .unwrap_or("")
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if m.is_empty() {
        event
    } else {
        format!("{event}-{m}")
    }
}

const CONFIG_FILES: &[&str] = &[SETTINGS_PATH, CLAUDE_JSON_PATH, PLUGINS_PATH];

impl Harness for Claude {
    // … id / render unchanged …

    /// Prints `##HASHES` + `<sha256>  <home-relative path>` lines for every
    /// file under skills/ and agents/ (symlinks followed), then one
    /// `##CONFIG <path>` block per config file with its base64 content on
    /// one line, then `##END`. No single quotes: the caller wraps the whole
    /// script in `shell::quote`.
    fn scan_script(&self) -> Option<String> {
        let mut s = String::new();
        s.push_str("cd \"$HOME\" || exit 0; ");
        s.push_str("if command -v sha256sum >/dev/null 2>&1; then H=sha256sum; else H=\"shasum -a 256\"; fi; ");
        s.push_str("echo \"##HASHES\"; ");
        s.push_str("for d in .claude/skills .claude/agents; do if [ -d \"$d\" ]; then find -L \"$d\" -type f -print0 2>/dev/null | xargs -0 $H 2>/dev/null; fi; done; ");
        for f in CONFIG_FILES {
            let rel = f.trim_start_matches("~/");
            s.push_str(&format!("echo \"##CONFIG {f}\"; if [ -f \"{rel}\" ]; then base64 < \"{rel}\" | tr -d \"\\n\"; fi; echo; "));
        }
        s.push_str("echo \"##END\"");
        Some(s)
    }

    fn parse_scan(&self, stdout: &str) -> Result<HostSnapshot, IpcError> {
        use base64::Engine;
        let mut snap = HostSnapshot::default();
        let mut current_config: Option<String> = None;
        for line in stdout.lines() {
            if line == "##HASHES" || line == "##END" {
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
                let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else { continue };
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    snap.configs.insert(path, v);
                }
                continue;
            }
            // `<hash>  <path>` (two spaces from sha256sum / shasum).
            if let Some((hash, path)) = line.split_once("  ") {
                let path = path.trim_start_matches("./");
                snap.files.insert(format!("~/{path}"), hash.trim().to_string());
            }
        }
        Ok(snap)
    }

    fn installed(&self, snap: &HostSnapshot) -> Vec<(Kind, String)> {
        let mut out: Vec<(Kind, String)> = Vec::new();
        let mut push = |k: Kind, n: String| {
            if !out.iter().any(|(kk, nn)| *kk == k && *nn == n) {
                out.push((k, n));
            }
        };
        for path in snap.files.keys() {
            if let Some(rest) = path.strip_prefix(&format!("{SKILLS_DIR}/")) {
                if let Some((name, _)) = rest.split_once('/') {
                    push(Kind::Skill, name.to_string());
                }
            } else if let Some(rest) = path.strip_prefix(&format!("{AGENTS_DIR}/")) {
                if let Some(stem) = rest.strip_suffix(".md") {
                    if !stem.contains('/') {
                        push(Kind::Agent, stem.to_string());
                    }
                }
            }
        }
        if let Some(hooks) = snap.configs.get(SETTINGS_PATH).and_then(|v| v.get("hooks")).and_then(Value::as_object) {
            for (event, entries) in hooks {
                for e in entries.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                    let matcher = e.get("matcher").and_then(Value::as_str);
                    push(Kind::Hook, hook_asset_name(event, matcher));
                }
            }
        }
        if let Some(servers) = snap.configs.get(CLAUDE_JSON_PATH).and_then(|v| v.get("mcpServers")).and_then(Value::as_object) {
            for name in servers.keys() {
                push(Kind::McpServer, name.clone());
            }
        }
        if let Some(plugins) = snap.configs.get(PLUGINS_PATH).and_then(|v| v.get("plugins")).and_then(Value::as_object) {
            for key in plugins.keys() {
                let name = key.split('@').next().unwrap_or(key).to_string();
                push(Kind::PluginRef, name);
            }
        }
        out
    }
}
```

- [ ] **Step 5: Implement inventory (pure part)**

Insert above the tests in `inventory.rs`:

```rust
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
        (Value::Object(w), Value::Object(h)) => w.iter().all(|(k, v)| h.get(k).is_some_and(|hv| is_subset(v, hv))),
        (Value::Array(w), Value::Array(h)) => w.iter().all(|wv| h.iter().any(|hv| is_subset(wv, hv))),
        _ => want == have,
    }
}

/// Does the host's config already satisfy this merge?
pub fn merge_satisfied(snap: &HostSnapshot, m: &ConfigMerge) -> bool {
    let Some(root) = snap.configs.get(&m.file) else { return false };
    let Some(have) = json_get(root, &m.json_path) else { return false };
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
                rows.push(AssetInventoryRow { state: AssetState::Unsupported.as_str().into(), ..base });
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
            if have.is_some() {
                present = true;
                host_parts.push(format!("{}:{}={}", m.file, m.json_path.join("/"), have.map(|v| v.to_string()).unwrap_or_default()));
            }
            if !merge_satisfied(snap, m) {
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
        let host_hash = if present { Some(sha256_hex(host_parts.join("\n").as_bytes())) } else { None };
        rows.push(AssetInventoryRow { state: state.as_str().into(), catalog_hash: Some(catalog_hash), host_hash, ..base });
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
```

Add `pub mod inventory;` to `catalog/mod.rs`.

- [ ] **Step 6: Run to verify pass**

Run: `cd src-tauri && cargo test service::catalog`
Expected: all catalog tests pass.

- [ ] **Step 7: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/service/catalog
git commit -m "feat(catalog): Claude host scan and per-asset drift state computation

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 8: Service entry points: configure, load, list, get, scan

**Files:**
- Modify: `src-tauri/src/service/catalog/mod.rs` (service functions)
- Modify: `src-tauri/src/service/catalog/inventory.rs` (host scan runner)

**Interfaces:**
- Consumes: Tasks 5, 6, 7.
- Produces (all in `catalog/mod.rs` unless noted):
  - `pub struct ConfigureArgs { pub repo_path: String, pub remote_url: Option<String> }` (Deserialize)
  - `pub fn configure(args: ConfigureArgs, store: &Mutex<Store>) -> Result<CatalogConfigRow, IpcError>`
  - `pub fn load(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError>`
  - `pub struct HostState { pub host_alias: String, pub harness: String, pub state: String }`
  - `pub struct AssetSummary { pub kind: String, pub name: String, pub version: String, pub description: String, pub tags: Vec<String>, pub hosts: Vec<HostState> }`
  - `pub struct AssetListing { pub head: String, pub loaded_at: i64, pub assets: Vec<AssetSummary>, pub unmanaged: Vec<AssetInventoryRow>, pub problems: Vec<Problem> }`
  - `pub fn list_assets(store: &Mutex<Store>) -> Result<AssetListing, IpcError>`
  - `pub struct Preview { pub harness: String, pub plan: Option<RenderPlan>, pub unsupported: Option<String> }`
  - `pub struct AssetDetail { pub asset: Asset, pub previews: Vec<Preview>, pub hosts: Vec<HostState> }`
  - `pub fn get_asset(kind: Kind, name: &str, store: &Mutex<Store>) -> Result<AssetDetail, IpcError>`
  - `pub fn inventory(store: &Mutex<Store>) -> Result<Vec<AssetInventoryRow>, IpcError>`
  - `pub fn config(store: &Mutex<Store>) -> Result<Option<CatalogConfigRow>, IpcError>`
  - In `inventory.rs`: `pub struct HostScanResult { pub host: String, pub status: String /* scanned | skipped | failed */, pub detail: Option<String>, pub rows: usize }`, `pub async fn scan_hosts(store: &Mutex<Store>, ssh: &Arc<SshClient>, only_host: Option<&str>) -> Result<Vec<HostScanResult>, IpcError>`, `pub async fn run_host_script(ssh: &Arc<SshClient>, host: &str, script: &str) -> Result<String, IpcError>`.

- [ ] **Step 1: Write the failing tests**

In `catalog/mod.rs` add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::Mutex;

    fn repo_with_one_skill(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("fleet-catalog-svc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(root.join("skills/s/asset.yaml"), "kind: skill\nname: s\ndescription: d\n").unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").args(args).current_dir(&root).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        root
    }

    #[test]
    fn load_requires_configuration() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let err = load(false, &store).unwrap_err();
        assert_eq!(err.code, E_CATALOG_NOT_CONFIGURED);
        assert_eq!(list_assets(&store).unwrap_err().code, E_CATALOG_NOT_CONFIGURED);
    }

    #[test]
    fn configure_load_list_get() {
        let _g = CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = repo_with_one_skill("cll");
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let cfg = configure(ConfigureArgs { repo_path: root.to_string_lossy().into(), remote_url: None }, &store).unwrap();
        assert_eq!(cfg.repo_path, root.to_string_lossy());
        let summary = load(false, &store).unwrap();
        assert_eq!(summary.asset_count, 1);
        assert_eq!(summary.head.len(), 40);
        assert_eq!(store.lock().unwrap().get_catalog_config().unwrap().unwrap().head_commit, Some(summary.head.clone()));

        // Seed inventory rows as a scan would.
        store.lock().unwrap().replace_host_inventory("local", "claude", &[
            crate::store::AssetInventoryRow { host_alias: "local".into(), harness: "claude".into(), kind: "skill".into(), name: "s".into(), state: "drifted".into(), catalog_hash: None, host_hash: None, scanned_at: 1 },
            crate::store::AssetInventoryRow { host_alias: "local".into(), harness: "claude".into(), kind: "skill".into(), name: "extra".into(), state: "unmanaged".into(), catalog_hash: None, host_hash: None, scanned_at: 1 },
        ]).unwrap();

        let listing = list_assets(&store).unwrap();
        assert_eq!(listing.assets.len(), 1);
        assert_eq!(listing.assets[0].name, "s");
        assert_eq!(listing.assets[0].hosts, vec![HostState { host_alias: "local".into(), harness: "claude".into(), state: "drifted".into() }]);
        assert_eq!(listing.unmanaged.len(), 1);
        assert_eq!(listing.unmanaged[0].name, "extra");

        let detail = get_asset(model::Kind::Skill, "s", &store).unwrap();
        assert_eq!(detail.asset.body, "b\n");
        assert_eq!(detail.previews.len(), 2);
        let claude = detail.previews.iter().find(|p| p.harness == "claude").unwrap();
        assert_eq!(claude.plan.as_ref().unwrap().files[0].path, "~/.claude/skills/s/SKILL.md");
        assert_eq!(get_asset(model::Kind::Agent, "nope", &store).unwrap_err().code, E_ASSET_NOT_FOUND);
        let hook = model::Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        repo::write_asset(&root, &hook, false).unwrap();
        load(false, &store).unwrap();
        let detail = get_asset(model::Kind::Hook, "h", &store).unwrap();
        let codex = detail.previews.iter().find(|p| p.harness == "codex").unwrap();
        assert!(codex.plan.is_none());
        assert!(codex.unsupported.as_deref().unwrap().contains("codex"));
    }
}
```

In `inventory.rs` tests add:

```rust
    #[tokio::test]
    async fn run_host_script_local_executes_bash() {
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let out = run_host_script(&ssh, "local", "echo \"hi $((1+1))\"").await.unwrap();
        assert_eq!(out.trim(), "hi 2");
    }

    #[tokio::test]
    async fn scan_hosts_scans_local_and_persists_rows() {
        let _g = crate::service::catalog::CATALOG_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("fleet-catalog-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("skills/s")).unwrap();
        std::fs::write(root.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        std::fs::write(root.join("skills/s/asset.yaml"), "kind: skill\nname: s\ndescription: d\n").unwrap();
        std::fs::write(root.join("skills/s/body.md"), "b\n").unwrap();
        let cat = crate::service::catalog::repo::load_dir(&root).unwrap();
        *crate::service::catalog::CATALOG.write().unwrap() = Some(cat);

        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        store.lock().unwrap().insert_host("local", None).unwrap();
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let results = scan_hosts(&store, &ssh, Some("local")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "scanned", "{:?}", results[0].detail);
        let rows = store.lock().unwrap().list_inventory().unwrap();
        assert!(rows.iter().any(|r| r.name == "s" && r.harness == "claude"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test service::catalog`
Expected: compile errors.

- [ ] **Step 3: Implement service functions in `mod.rs`**

```rust
use crate::events::CatalogSummary;
use crate::ipc_error::IpcError;
use crate::store::{AssetInventoryRow, CatalogConfigRow, Store};
use harness::{Harness, RenderPlan};
use model::{Asset, Kind, Problem};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigureArgs {
    pub repo_path: String,
    pub remote_url: Option<String>,
}

fn lock(store: &Mutex<Store>) -> Result<std::sync::MutexGuard<'_, Store>, IpcError> {
    store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))
}

fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => p.to_string(),
    }
}

pub fn config(store: &Mutex<Store>) -> Result<Option<CatalogConfigRow>, IpcError> {
    Ok(lock(store)?.get_catalog_config()?)
}

fn require_config(store: &Mutex<Store>) -> Result<CatalogConfigRow, IpcError> {
    config(store)?.ok_or_else(|| IpcError::new(E_CATALOG_NOT_CONFIGURED, "configure the catalog repo first"))
}

/// Persist the repo location and clone it if needed. Does not load.
pub fn configure(args: ConfigureArgs, store: &Mutex<Store>) -> Result<CatalogConfigRow, IpcError> {
    let path = expand_home(args.repo_path.trim());
    if path.is_empty() {
        return Err(IpcError::new("E_VALIDATION", "repo_path must not be empty"));
    }
    let remote = args.remote_url.as_deref().map(str::trim).filter(|s| !s.is_empty());
    repo::ensure_repo(std::path::Path::new(&path), remote)?;
    Ok(lock(store)?.set_catalog_config(&path, remote)?)
}

/// (Optionally pull, then) parse the repo into `CATALOG` and record HEAD.
pub fn load(pull: bool, store: &Mutex<Store>) -> Result<CatalogSummary, IpcError> {
    let cfg = require_config(store)?;
    let root = std::path::PathBuf::from(&cfg.repo_path);
    repo::ensure_repo(&root, cfg.remote_url.as_deref())?;
    if pull {
        repo::pull(&root)?;
    }
    let mut cat = repo::load_dir(&root)?;
    cat.head = repo::head(&root)?;
    let summary = CatalogSummary {
        head: cat.head.clone(),
        loaded_at: cat.loaded_at,
        asset_count: cat.assets.len(),
        problem_count: cat.problems.len(),
    };
    {
        let s = lock(store)?;
        s.set_catalog_head(&summary.head, summary.loaded_at)?;
        s.bus_catalog_loaded(&summary);
    }
    *CATALOG.write().map_err(|_| IpcError::new("E_LOCK", "catalog lock poisoned"))? = Some(cat);
    Ok(summary)
}

fn with_catalog<T>(f: impl FnOnce(&repo::Catalog) -> Result<T, IpcError>) -> Result<T, IpcError> {
    let guard = CATALOG.read().map_err(|_| IpcError::new("E_LOCK", "catalog lock poisoned"))?;
    match guard.as_ref() {
        Some(c) => f(c),
        None => Err(IpcError::new(E_CATALOG_NOT_CONFIGURED, "catalog not loaded; call catalog_load")),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostState {
    pub host_alias: String,
    pub harness: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetSummary {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub tags: Vec<String>,
    pub hosts: Vec<HostState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetListing {
    pub head: String,
    pub loaded_at: i64,
    pub assets: Vec<AssetSummary>,
    pub unmanaged: Vec<AssetInventoryRow>,
    pub problems: Vec<Problem>,
}

fn host_states(rows: &[AssetInventoryRow], kind: Kind, name: &str) -> Vec<HostState> {
    rows.iter()
        .filter(|r| r.kind == kind.as_str() && r.name == name && r.state != "unmanaged")
        .map(|r| HostState { host_alias: r.host_alias.clone(), harness: r.harness.clone(), state: r.state.clone() })
        .collect()
}

pub fn inventory(store: &Mutex<Store>) -> Result<Vec<AssetInventoryRow>, IpcError> {
    Ok(lock(store)?.list_inventory()?)
}

pub fn list_assets(store: &Mutex<Store>) -> Result<AssetListing, IpcError> {
    require_config(store)?;
    let rows = inventory(store)?;
    with_catalog(|cat| {
        Ok(AssetListing {
            head: cat.head.clone(),
            loaded_at: cat.loaded_at,
            assets: cat
                .assets
                .iter()
                .map(|a| AssetSummary {
                    kind: a.kind().as_str().to_string(),
                    name: a.header.name.clone(),
                    version: a.header.version.clone(),
                    description: a.header.description.clone(),
                    tags: a.header.tags.clone(),
                    hosts: host_states(&rows, a.kind(), &a.header.name),
                })
                .collect(),
            unmanaged: rows.iter().filter(|r| r.state == "unmanaged").cloned().collect(),
            problems: cat.problems.clone(),
        })
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub harness: String,
    pub plan: Option<RenderPlan>,
    pub unsupported: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetDetail {
    pub asset: Asset,
    pub previews: Vec<Preview>,
    pub hosts: Vec<HostState>,
}

pub fn get_asset(kind: Kind, name: &str, store: &Mutex<Store>) -> Result<AssetDetail, IpcError> {
    let rows = inventory(store)?;
    with_catalog(|cat| {
        let asset = cat
            .find(kind, name)
            .ok_or_else(|| IpcError::new(E_ASSET_NOT_FOUND, format!("{} {name} is not in the catalog", kind.as_str())))?;
        let previews = harness::all()
            .iter()
            .map(|h| match h.render(asset) {
                Ok(plan) => Preview { harness: h.id().into(), plan: Some(plan), unsupported: None },
                Err(u) => Preview { harness: h.id().into(), plan: None, unsupported: Some(u.into_ipc().message) },
            })
            .collect();
        Ok(AssetDetail { asset: asset.clone(), previews, hosts: host_states(&rows, kind, name) })
    })
}
```

`Store` needs a tiny pass-through so the service can emit `catalog:loaded` without reaching into the private `bus` field. Add to `impl Store` in `store.rs`:

```rust
    /// Emit `catalog:loaded` (the catalog itself is not a store row).
    pub fn bus_catalog_loaded(&self, summary: &crate::events::CatalogSummary) {
        self.bus.catalog_loaded(summary);
    }
```

- [ ] **Step 4: Implement the scan runner in `inventory.rs`**

```rust
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SCAN_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, serde::Serialize)]
pub struct HostScanResult {
    pub host: String,
    /// "scanned" | "skipped" | "failed"
    pub status: String,
    pub detail: Option<String>,
    pub rows: usize,
}

/// Run a bash script on a host and return stdout. `local` runs in-process;
/// remote hosts go through the SSH multiplexer with the whole script as one
/// quoted `bash -lc` word (ssh space-joins argv).
pub async fn run_host_script(ssh: &Arc<SshClient>, host: &str, script: &str) -> Result<String, crate::ipc_error::IpcError> {
    if host == "local" {
        let out = tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output()
            .await
            .map_err(|e| crate::ipc_error::IpcError::new("E_IO", format!("spawn bash: {e}")))?;
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let quoted = quote(script);
    let out = ssh.run(host, &["bash", "-lc", &quoted], SCAN_TIMEOUT).await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Scan every non-hidden reachable host (or just `only_host`) with every
/// harness that supports scanning, persisting rows per (host, harness).
/// Per-host failures never abort the others (mirrors `provision_hosts`).
pub async fn scan_hosts(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    only_host: Option<&str>,
) -> Result<Vec<HostScanResult>, crate::ipc_error::IpcError> {
    let hosts = {
        let s = store.lock().map_err(|_| crate::ipc_error::IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
    };
    let catalog = {
        let g = super::CATALOG.read().map_err(|_| crate::ipc_error::IpcError::new("E_LOCK", "catalog lock poisoned"))?;
        g.clone().ok_or_else(|| crate::ipc_error::IpcError::new(super::E_CATALOG_NOT_CONFIGURED, "catalog not loaded; call catalog_load"))?
    };
    let mut results = Vec::new();
    for h in hosts {
        if h.hidden || only_host.is_some_and(|o| o != h.alias) {
            continue;
        }
        if h.alias != "local" && !h.reachable {
            results.push(HostScanResult { host: h.alias, status: "skipped".into(), detail: Some("unreachable".into()), rows: 0 });
            continue;
        }
        let mut total = 0usize;
        let mut failure: Option<String> = None;
        for harness in super::harness::all() {
            let Some(script) = harness.scan_script() else { continue };
            let scanned_at = super::now_secs();
            let rows = match run_host_script(ssh, &h.alias, &script).await.and_then(|out| harness.parse_scan(&out)) {
                Ok(snap) => compute_states(&catalog, harness.as_ref(), &h.alias, &snap, scanned_at),
                Err(e) => {
                    failure = Some(format!("{}: {}", harness.id(), e.message));
                    continue;
                }
            };
            total += rows.len();
            if let Ok(s) = store.lock() {
                if let Err(e) = s.replace_host_inventory(&h.alias, harness.id(), &rows) {
                    failure = Some(format!("persist inventory: {e}"));
                }
            }
        }
        results.push(match failure {
            None => HostScanResult { host: h.alias, status: "scanned".into(), detail: None, rows: total },
            Some(d) => HostScanResult { host: h.alias, status: "failed".into(), detail: Some(d), rows: total },
        });
    }
    Ok(results)
}
```

`Catalog` must derive `Clone` (it does, from Task 5).

- [ ] **Step 5: Run to verify pass**

Run: `cd src-tauri && cargo test service::catalog`
Expected: all pass. The `scan_hosts_scans_local_and_persists_rows` test runs the real scan script against the developer's own `~/.claude`; it only asserts that the catalog skill row exists, so it is safe on any machine.

- [ ] **Step 6: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/service/catalog src/store.rs
git commit -m "feat(catalog): configure/load/list/get service functions and host scan runner

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 9: Importer from a Claude config directory

**Files:**
- Create: `src-tauri/src/service/catalog/import.rs`
- Modify: `src-tauri/src/service/catalog/mod.rs` (`pub mod import;`, `import_host` entry point)

**Interfaces:**
- Consumes: Task 1 model, Task 3 `claude::{unmap_tool, unmap_tier, unmap_event, hook_asset_name}`, Task 5 `repo::write_asset`.
- Produces:
  - `pub struct ImportReport { pub created: Vec<(String, String)> /* (kind, name) */, pub problems: Vec<Problem>, pub flagged_secrets: Vec<String>, pub dry_run: bool }`
  - `pub struct ImportSources { pub claude_dir: PathBuf, pub claude_json: PathBuf }` and `ImportSources::for_local() -> Result<Self, IpcError>`
  - `pub fn import_claude(src: &ImportSources, repo_root: &Path, host_alias: &str, fleet_token: Option<&str>, dry_run: bool) -> Result<ImportReport, IpcError>`
  - `pub fn parse_frontmatter(text: &str) -> (serde_yaml::Mapping, String)` (mapping + body)
  - In `mod.rs`: `pub struct ImportArgs { pub host_alias: String, pub dry_run: bool }`, `pub fn import_host(args: ImportArgs, store: &Mutex<Store>, fleet_token: Option<&str>) -> Result<ImportReport, IpcError>`.

- [ ] **Step 1: Write the failing tests**

```rust
//! Import a Claude Code config directory into the catalog as IR assets.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::model::{AssetSpec, Kind};
    use crate::service::catalog::repo::load_dir;
    use std::fs;

    fn fixture(tag: &str) -> (ImportSources, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("fleet-import-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let claude_dir = base.join("home/.claude");
        let w = |rel: &str, c: &str| {
            let p = base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        };
        w("home/.claude/skills/worktree/SKILL.md", "---\nname: worktree\ndescription: Make a worktree.\nallowed-tools: Bash, Read\ndisable-model-invocation: true\n---\n# Body\n");
        w("home/.claude/skills/worktree/scripts/go.sh", "echo\n");
        w("linked/skill-x/SKILL.md", "---\nname: skill-x\ndescription: Linked.\n---\nlinked body\n");
        std::os::unix::fs::symlink(base.join("linked/skill-x"), claude_dir.join("skills/skill-x")).unwrap();
        std::os::unix::fs::symlink(base.join("nowhere"), claude_dir.join("skills/broken")).unwrap();
        w("home/.claude/agents/pm-qa.md", "---\nname: pm-qa\ndescription: QA lens.\ntools: Read, Grep, Weird\nmodel: opus\ncolor: blue\n---\nYou are QA.\n");
        w("home/.claude/agents/odd.md", "---\nname: odd\ndescription: Odd model.\nmodel: claude-haiku-4-5-20251001\n---\np\n");
        w("home/.claude/settings.json", r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"exec rtk hook claude"}]}],"Stop":[{"hooks":[{"type":"command","command":"node stop.mjs","timeout":20}]},{"matcher":"","hooks":[{"type":"http","url":"http://127.0.0.1:4180/hook","timeout":5,"headers":{"Authorization":"Bearer SECRET123"}}]}]}}"#);
        w("home/.claude.json", r#"{"mcpServers":{"claude-fleet":{"type":"http","url":"http://127.0.0.1:4180/mcp","headers":{"Authorization":"Bearer SECRET123"}},"jira":{"type":"stdio","command":"npx","args":["-y","jira"],"env":{"JIRA_TOKEN":"abc"}}},"other":1}"#);
        w("home/.claude/plugins/installed_plugins.json", r#"{"version":2,"plugins":{"superpowers@superpowers-marketplace":[{"scope":"user","version":"6.3.0"}]}}"#);
        w("home/.claude/plugins/known_marketplaces.json", r#"{"superpowers-marketplace":{"source":{"source":"github","repo":"obra/superpowers-marketplace"}}}"#);
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        (ImportSources { claude_dir, claude_json: base.join("home/.claude.json") }, repo)
    }

    #[test]
    fn frontmatter_splits_mapping_and_body() {
        let (m, body) = parse_frontmatter("---\nname: a\nx: 1\n---\nrest\nmore\n");
        assert_eq!(m.get("name").unwrap().as_str(), Some("a"));
        assert_eq!(body, "rest\nmore\n");
        let (m, body) = parse_frontmatter("no frontmatter\n");
        assert!(m.is_empty());
        assert_eq!(body, "no frontmatter\n");
    }

    #[test]
    fn dry_run_reports_without_writing() {
        let (src, repo) = fixture("dry");
        let rep = import_claude(&src, &repo, "local", Some("SECRET123"), true).unwrap();
        assert!(rep.dry_run);
        assert!(!repo.join("skills").exists());
        assert!(rep.created.contains(&("skill".into(), "worktree".into())));
        assert!(rep.problems.iter().any(|p| p.path.ends_with("skills/broken")));
    }

    #[test]
    fn import_converts_every_kind_losslessly() {
        let (src, repo) = fixture("full");
        let rep = import_claude(&src, &repo, "local", Some("SECRET123"), false).unwrap();
        let cat = load_dir(&repo).unwrap();
        assert!(cat.problems.is_empty(), "{:?}", cat.problems);

        let skill = cat.find(Kind::Skill, "worktree").unwrap();
        assert_eq!(skill.body, "# Body\n");
        assert_eq!(skill.header.description, "Make a worktree.");
        match &skill.spec { AssetSpec::Skill { allowed_tools, .. } => assert_eq!(allowed_tools, &vec!["bash".to_string(), "read".to_string()]), _ => panic!() }
        assert_eq!(skill.header.targets["claude"].extra["disable-model-invocation"], serde_json::Value::Bool(true));
        assert_eq!(skill.resources[0].rel_path, "resources/scripts/go.sh");
        assert_eq!(skill.header.source.as_ref().unwrap().imported_from.as_deref(), Some("local"));

        let linked = cat.find(Kind::Skill, "skill-x").unwrap();
        assert_eq!(linked.body, "linked body\n");
        assert!(linked.header.source.as_ref().unwrap().symlink_target.as_deref().unwrap().ends_with("linked/skill-x"));

        let agent = cat.find(Kind::Agent, "pm-qa").unwrap();
        match &agent.spec { AssetSpec::Agent { tools, model } => { assert_eq!(tools, &vec!["read".to_string(), "grep".to_string()]); assert_eq!(model, "strong"); } _ => panic!() }
        assert_eq!(agent.header.targets["claude"].extra["tools"], serde_json::json!(["Weird"]));
        assert_eq!(agent.header.targets["claude"].extra["color"], serde_json::json!("blue"));
        let odd = cat.find(Kind::Agent, "odd").unwrap();
        match &odd.spec { AssetSpec::Agent { model, .. } => assert_eq!(model, "default"), _ => panic!() }
        assert_eq!(odd.header.targets["claude"].model.as_deref(), Some("claude-haiku-4-5-20251001"));

        let rtk = cat.find(Kind::Hook, "before-tool-bash").unwrap();
        match &rtk.spec { AssetSpec::Hook { event, r#match, action } => { assert_eq!(event, "before_tool"); assert_eq!(r#match.as_ref().unwrap().tool, "bash"); assert_eq!(action.command.as_deref(), Some("exec rtk hook claude")); } _ => panic!() }
        assert!(cat.find(Kind::Hook, "stop").is_some());
        let stop2 = cat.find(Kind::Hook, "stop-2").unwrap();
        match &stop2.spec { AssetSpec::Hook { action, .. } => { assert_eq!(action.headers["Authorization"], "Bearer ${FLEET_MCP_TOKEN}"); assert_eq!(action.timeout_s, Some(5)); } _ => panic!() }

        let fleet = cat.find(Kind::McpServer, "claude-fleet").unwrap();
        match &fleet.spec { AssetSpec::McpServer { headers, .. } => assert_eq!(headers["Authorization"], "Bearer ${FLEET_MCP_TOKEN}"), _ => panic!() }
        let jira = cat.find(Kind::McpServer, "jira").unwrap();
        match &jira.spec { AssetSpec::McpServer { transport, args, env, .. } => { assert_eq!(transport, "stdio"); assert_eq!(args, &vec!["-y".to_string(), "jira".to_string()]); assert_eq!(env["JIRA_TOKEN"], "abc"); } _ => panic!() }
        assert!(rep.flagged_secrets.iter().any(|s| s.contains("jira") && s.contains("JIRA_TOKEN")), "{:?}", rep.flagged_secrets);

        let plugin = cat.find(Kind::PluginRef, "superpowers").unwrap();
        match &plugin.spec { AssetSpec::PluginRef { marketplace, version, .. } => { assert_eq!(marketplace.repo, "obra/superpowers-marketplace"); assert_eq!(version, "6.3.0"); } _ => panic!() }

        // Re-import collides on everything and creates nothing new.
        let again = import_claude(&src, &repo, "local", Some("SECRET123"), false).unwrap();
        assert!(again.created.is_empty());
        assert!(again.problems.iter().all(|p| p.message.contains("already exists")));
    }

    #[test]
    fn import_then_render_reproduces_skill_and_agent() {
        let (src, repo) = fixture("rt");
        import_claude(&src, &repo, "local", None, false).unwrap();
        let cat = load_dir(&repo).unwrap();
        let claude = crate::service::catalog::harness::claude::Claude;
        use crate::service::catalog::harness::Harness;
        let plan = claude.render(cat.find(Kind::Skill, "worktree").unwrap()).unwrap();
        let rendered = String::from_utf8(plan.files.iter().find(|f| f.path.ends_with("SKILL.md")).unwrap().bytes.clone()).unwrap();
        assert_eq!(rendered, fs::read_to_string(src.claude_dir.join("skills/worktree/SKILL.md")).unwrap());
        let plan = claude.render(cat.find(Kind::Agent, "pm-qa").unwrap()).unwrap();
        let rendered = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        // Field order is canonical (name, description, tools, model, extras); content is preserved.
        assert!(rendered.contains("tools: Read, Grep, Weird"), "{rendered}");
        assert!(rendered.contains("model: opus"));
        assert!(rendered.contains("color: blue"));
        assert!(rendered.ends_with("---\nYou are QA.\n"));
    }
}
```

Note for the round-trip assertion on `tools`: the Claude renderer must append `targets.claude.extra.tools` (unmapped names) to the mapped tool list, in that order. Update `render_agent` in `claude.rs` accordingly: after mapping `tools`, if `t.extra` has a `tools` array, extend the list with its string items and do not emit `tools` again as an extra key. Add the same handling to `render_skill` for `allowed_tools`. Add this test to `claude.rs`:

```rust
    #[test]
    fn extra_tools_are_appended_not_duplicated() {
        let plan = render("kind: agent\nname: a\ndescription: d\ntools: [read]\ntargets:\n  claude:\n    extra:\n      tools: [Weird]\n", "p\n");
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        assert!(text.contains("tools: Read, Weird"), "{text}");
        assert_eq!(text.matches("tools:").count(), 1);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test service::catalog::import service::catalog::harness::claude`
Expected: compile errors / failing `extra_tools_are_appended_not_duplicated`.

- [ ] **Step 3: Implement**

```rust
use super::harness::claude::{hook_asset_name, unmap_event, unmap_tier, unmap_tool};
use super::model::{Asset, AssetSpec, Header, HookAction, HookMatch, Kind, Marketplace, Problem, Resource, Source, TargetOverride};
use super::repo::{asset_path, write_asset};
use super::E_ASSET_EXISTS;
use crate::ipc_error::IpcError;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportReport {
    pub created: Vec<(String, String)>,
    pub problems: Vec<Problem>,
    pub flagged_secrets: Vec<String>,
    pub dry_run: bool,
}

pub struct ImportSources {
    pub claude_dir: PathBuf,
    pub claude_json: PathBuf,
}

impl ImportSources {
    pub fn for_local() -> Result<Self, IpcError> {
        let home = std::env::var("HOME").map_err(|_| IpcError::new("E_IO", "HOME not set"))?;
        Ok(Self { claude_dir: Path::new(&home).join(".claude"), claude_json: Path::new(&home).join(".claude.json") })
    }
}

/// Split `---\n…\n---\n` frontmatter from a markdown file.
pub fn parse_frontmatter(text: &str) -> (serde_yaml::Mapping, String) {
    let Some(rest) = text.strip_prefix("---\n") else { return (serde_yaml::Mapping::new(), text.to_string()) };
    let Some(end) = rest.find("\n---\n") else { return (serde_yaml::Mapping::new(), text.to_string()) };
    let yaml = &rest[..end];
    let body = &rest[end + 5..];
    let map = serde_yaml::from_str::<serde_yaml::Value>(yaml)
        .ok()
        .and_then(|v| v.as_mapping().cloned())
        .unwrap_or_default();
    (map, body.to_string())
}

fn yaml_to_json(v: &serde_yaml::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn str_of(m: &serde_yaml::Mapping, key: &str) -> Option<String> {
    m.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// Split a Claude tool list (`Read, Grep` or a YAML sequence) into
/// (neutral names, unknown Claude names).
fn split_tools(v: Option<&serde_yaml::Value>) -> (Vec<String>, Vec<String>) {
    let items: Vec<String> = match v {
        Some(serde_yaml::Value::String(s)) => s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        Some(serde_yaml::Value::Sequence(seq)) => seq.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        _ => vec![],
    };
    let mut neutral = Vec::new();
    let mut unknown = Vec::new();
    for t in items {
        if let Some(n) = unmap_tool(&t) {
            neutral.push(n.to_string());
        } else if let Some(server) = t.strip_prefix("mcp__").and_then(|r| r.strip_suffix("__*")) {
            neutral.push(format!("mcp:{server}"));
        } else {
            unknown.push(t);
        }
    }
    (neutral, unknown)
}

fn header(kind: Kind, name: &str, description: String, host: &str, original: &Path, symlink: Option<PathBuf>) -> Header {
    Header {
        kind,
        name: name.to_string(),
        version: "1".into(),
        description,
        tags: vec![],
        source: Some(Source {
            imported_from: Some(host.to_string()),
            original_path: Some(original.to_string_lossy().to_string()),
            symlink_target: symlink.map(|p| p.to_string_lossy().to_string()),
        }),
        targets: BTreeMap::new(),
    }
}

fn claude_override(extra: BTreeMap<String, Value>, model: Option<String>) -> BTreeMap<String, TargetOverride> {
    let mut t = BTreeMap::new();
    if !extra.is_empty() || model.is_some() {
        t.insert("claude".to_string(), TargetOverride { enabled: true, model, render_as: None, extra });
    }
    t
}

const KNOWN_SKILL_KEYS: &[&str] = &["name", "description", "allowed-tools"];
const KNOWN_AGENT_KEYS: &[&str] = &["name", "description", "tools", "model"];

fn import_skill(dir: &Path, name: &str, host: &str) -> Result<Asset, String> {
    let symlink = std::fs::read_link(dir).ok();
    let skill_md = dir.join("SKILL.md");
    let text = std::fs::read_to_string(&skill_md).map_err(|e| format!("{}: {e}", skill_md.display()))?;
    let (fm, body) = parse_frontmatter(&text);
    let (allowed_tools, unknown) = split_tools(fm.get("allowed-tools"));
    let mut extra: BTreeMap<String, Value> = fm
        .iter()
        .filter_map(|(k, v)| k.as_str().filter(|k| !KNOWN_SKILL_KEYS.contains(k)).map(|k| (k.to_string(), yaml_to_json(v))))
        .collect();
    if !unknown.is_empty() {
        extra.insert("tools".into(), serde_json::json!(unknown));
    }
    let mut resources = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&d).map_err(|e| e.to_string())?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                stack.push(p);
            } else if p != skill_md {
                let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().to_string();
                resources.push(Resource { rel_path: format!("resources/{rel}"), bytes: std::fs::read(&p).map_err(|e| e.to_string())? });
            }
        }
    }
    let mut h = header(Kind::Skill, name, str_of(&fm, "description").unwrap_or_default(), host, dir, symlink);
    h.targets = claude_override(extra, None);
    Ok(Asset { header: h, spec: AssetSpec::Skill { allowed_tools, user_invocable: true, triggers: vec![] }, body, resources })
}

fn import_agent(file: &Path, name: &str, host: &str) -> Result<Asset, String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let (fm, body) = parse_frontmatter(&text);
    let (tools, unknown) = split_tools(fm.get("tools"));
    let (model, explicit) = match str_of(&fm, "model") {
        Some(m) => match unmap_tier(&m) {
            Some(tier) => (tier.to_string(), None),
            None => ("default".to_string(), Some(m)),
        },
        None => ("default".to_string(), None),
    };
    let mut extra: BTreeMap<String, Value> = fm
        .iter()
        .filter_map(|(k, v)| k.as_str().filter(|k| !KNOWN_AGENT_KEYS.contains(k)).map(|k| (k.to_string(), yaml_to_json(v))))
        .collect();
    if !unknown.is_empty() {
        extra.insert("tools".into(), serde_json::json!(unknown));
    }
    let mut h = header(Kind::Agent, name, str_of(&fm, "description").unwrap_or_default(), host, file, None);
    h.targets = claude_override(extra, explicit);
    Ok(Asset { header: h, spec: AssetSpec::Agent { tools, model }, body, resources: vec![] })
}

/// Replace any header value equal to `Bearer <fleet_token>` with the placeholder.
fn scrub_headers(headers: &mut BTreeMap<String, String>, fleet_token: Option<&str>) {
    if let Some(tok) = fleet_token {
        for v in headers.values_mut() {
            if v.contains(tok) {
                *v = v.replace(tok, "${FLEET_MCP_TOKEN}");
            }
        }
    }
}

fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["token", "secret", "key", "password", "authorization"].iter().any(|s| k.contains(s))
}

fn import_hooks(settings: &Value, host: &str, path: &Path, fleet_token: Option<&str>, taken: &mut Vec<String>) -> Vec<Asset> {
    let mut out = Vec::new();
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else { return out };
    for (claude_event, entries) in hooks {
        let Some(event) = unmap_event(claude_event) else { continue };
        for entry in entries.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let matcher = entry.get("matcher").and_then(Value::as_str).filter(|m| !m.is_empty());
            let base = hook_asset_name(claude_event, matcher);
            let mut name = base.clone();
            let mut n = 2;
            while taken.contains(&name) {
                name = format!("{base}-{n}");
                n += 1;
            }
            taken.push(name.clone());
            for h in entry.get("hooks").and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[]) {
                let kind = h.get("type").and_then(Value::as_str).unwrap_or("command").to_string();
                let mut headers: BTreeMap<String, String> = h
                    .get("headers")
                    .and_then(Value::as_object)
                    .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                    .unwrap_or_default();
                scrub_headers(&mut headers, fleet_token);
                let extra: BTreeMap<String, Value> = h
                    .as_object()
                    .map(|o| o.iter().filter(|(k, _)| !["type", "command", "url", "headers", "timeout"].contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                let mut hd = header(Kind::Hook, &name, format!("Imported {claude_event} hook{}", matcher.map(|m| format!(" for {m}")).unwrap_or_default()), host, path, None);
                hd.targets = claude_override(extra, None);
                let tool = matcher.map(|m| unmap_tool(m).map(String::from).unwrap_or_else(|| m.to_string()));
                out.push(Asset {
                    header: hd,
                    spec: AssetSpec::Hook {
                        event: event.to_string(),
                        r#match: tool.map(|t| HookMatch { tool: t }),
                        action: HookAction {
                            kind,
                            command: h.get("command").and_then(Value::as_str).map(String::from),
                            url: h.get("url").and_then(Value::as_str).map(String::from),
                            headers,
                            timeout_s: h.get("timeout").and_then(Value::as_u64),
                        },
                    },
                    body: String::new(),
                    resources: vec![],
                });
                break; // one hook per entry in v1; extra entries would need distinct names
            }
        }
    }
    out
}

fn import_mcp(claude_json: &Value, host: &str, path: &Path, fleet_token: Option<&str>, flagged: &mut Vec<String>) -> Vec<Asset> {
    let mut out = Vec::new();
    let Some(servers) = claude_json.get("mcpServers").and_then(Value::as_object) else { return out };
    for (name, s) in servers {
        let transport = match s.get("type").and_then(Value::as_str) {
            Some("http") | Some("sse") => "http",
            _ => "stdio",
        };
        let mut headers: BTreeMap<String, String> = s
            .get("headers")
            .and_then(Value::as_object)
            .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|x| (k.clone(), x.to_string()))).collect())
            .unwrap_or_default();
        scrub_headers(&mut headers, fleet_token);
        let env: BTreeMap<String, String> = s
            .get("env")
            .and_then(Value::as_object)
            .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|x| (k.clone(), x.to_string()))).collect())
            .unwrap_or_default();
        for (k, v) in headers.iter().chain(env.iter()) {
            if looks_secret(k) && !v.contains("${") {
                flagged.push(format!("mcp_server {name}: {k} holds a literal secret; replace with a ${{PLACEHOLDER}}"));
            }
        }
        let extra: BTreeMap<String, Value> = s
            .as_object()
            .map(|o| o.iter().filter(|(k, _)| !["type", "url", "headers", "command", "args", "env"].contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let mut hd = header(Kind::McpServer, name, format!("Imported MCP server {name}"), host, path, None);
        hd.targets = claude_override(extra, None);
        out.push(Asset {
            header: hd,
            spec: AssetSpec::McpServer {
                transport: transport.to_string(),
                url: s.get("url").and_then(Value::as_str).map(String::from),
                headers,
                command: s.get("command").and_then(Value::as_str).map(String::from),
                args: s.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                env,
            },
            body: String::new(),
            resources: vec![],
        });
    }
    out
}

fn import_plugins(installed: &Value, known: &Value, host: &str, path: &Path) -> Vec<Asset> {
    let mut out = Vec::new();
    let Some(plugins) = installed.get("plugins").and_then(Value::as_object) else { return out };
    for (key, records) in plugins {
        let Some((plugin, market)) = key.split_once('@') else { continue };
        let version = records
            .as_array()
            .and_then(|a| a.first())
            .and_then(|r| r.get("version"))
            .and_then(Value::as_str)
            .unwrap_or("latest")
            .to_string();
        let src = known.get(market).and_then(|m| m.get("source"));
        let marketplace = Marketplace {
            name: market.to_string(),
            source: src.and_then(|s| s.get("source")).and_then(Value::as_str).unwrap_or("github").to_string(),
            repo: src.and_then(|s| s.get("repo")).and_then(Value::as_str).unwrap_or_default().to_string(),
        };
        out.push(Asset {
            header: header(Kind::PluginRef, plugin, format!("Imported plugin {plugin} from {market}"), host, path, None),
            spec: AssetSpec::PluginRef { harness: "claude".into(), marketplace, plugin: plugin.to_string(), version },
            body: String::new(),
            resources: vec![],
        });
    }
    out
}

fn read_json(p: &Path) -> Value {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

/// Convert a Claude config directory into catalog assets. Never overwrites;
/// collisions become problems. `dry_run` writes nothing.
pub fn import_claude(
    src: &ImportSources,
    repo_root: &Path,
    host: &str,
    fleet_token: Option<&str>,
    dry_run: bool,
) -> Result<ImportReport, IpcError> {
    let mut report = ImportReport { created: vec![], problems: vec![], flagged_secrets: vec![], dry_run };
    let mut assets: Vec<Asset> = Vec::new();

    let skills_dir = src.claude_dir.join("skills");
    if skills_dir.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&skills_dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if !p.is_dir() {
                if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                    report.problems.push(Problem { path: p.to_string_lossy().to_string(), message: "broken symlink".into() });
                }
                continue;
            }
            if !p.join("SKILL.md").is_file() {
                continue;
            }
            match import_skill(&p, &name, host) {
                Ok(a) => assets.push(a),
                Err(message) => report.problems.push(Problem { path: p.to_string_lossy().to_string(), message }),
            }
        }
    }
    let agents_dir = src.claude_dir.join("agents");
    if agents_dir.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&agents_dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let name = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            match import_agent(&p, &name, host) {
                Ok(a) => assets.push(a),
                Err(message) => report.problems.push(Problem { path: p.to_string_lossy().to_string(), message }),
            }
        }
    }
    let settings_path = src.claude_dir.join("settings.json");
    let mut taken = Vec::new();
    assets.extend(import_hooks(&read_json(&settings_path), host, &settings_path, fleet_token, &mut taken));
    assets.extend(import_mcp(&read_json(&src.claude_json), host, &src.claude_json, fleet_token, &mut report.flagged_secrets));
    let installed_path = src.claude_dir.join("plugins/installed_plugins.json");
    let known_path = src.claude_dir.join("plugins/known_marketplaces.json");
    assets.extend(import_plugins(&read_json(&installed_path), &read_json(&known_path), host, &installed_path));

    for a in assets {
        let kind = a.kind();
        let problems = a.validate();
        if !problems.is_empty() {
            report.problems.push(Problem { path: format!("{}/{}", kind.dir(), a.header.name), message: problems.join("; ") });
            continue;
        }
        if asset_path(repo_root, kind, &a.header.name).exists() {
            report.problems.push(Problem {
                path: format!("{}/{}", kind.dir(), a.header.name),
                message: format!("{} {} already exists in the catalog", kind.as_str(), a.header.name),
            });
            continue;
        }
        if !dry_run {
            if let Err(e) = write_asset(repo_root, &a, false) {
                if e.code == E_ASSET_EXISTS {
                    report.problems.push(Problem { path: format!("{}/{}", kind.dir(), a.header.name), message: e.message });
                } else {
                    return Err(e);
                }
                continue;
            }
        }
        report.created.push((kind.as_str().to_string(), a.header.name.clone()));
    }
    Ok(report)
}
```

In `catalog/mod.rs`:

```rust
pub mod import;

#[derive(Debug, Clone, Deserialize)]
pub struct ImportArgs {
    pub host_alias: String,
    #[serde(default)]
    pub dry_run: bool,
}

/// Import from a host's Claude config. v1 supports the controller (`local`)
/// only; other hosts return E_ASSET_UNSUPPORTED.
pub fn import_host(args: ImportArgs, store: &Mutex<Store>, fleet_token: Option<&str>) -> Result<import::ImportReport, IpcError> {
    let cfg = require_config(store)?;
    if args.host_alias != "local" {
        return Err(IpcError::new(E_ASSET_UNSUPPORTED, "importing from remote hosts is not supported yet; use local"));
    }
    let src = import::ImportSources::for_local()?;
    import::import_claude(&src, std::path::Path::new(&cfg.repo_path), &args.host_alias, fleet_token, args.dry_run)
}
```

Update `claude.rs` `render_skill` / `render_agent` so `targets.claude.extra.tools` (a JSON array of strings) is appended to the mapped list and skipped when writing extras:

```rust
        let mut mapped: Vec<String> = tools.iter().map(|x| map_tool(x)).collect();
        if let Some(arr) = t.extra.get("tools").and_then(Value::as_array) {
            mapped.extend(arr.iter().filter_map(Value::as_str).map(String::from));
        }
        if !mapped.is_empty() {
            fields.push(("tools", yaml_str(&mapped.join(", "))));
        }
        // …
        for (k, v) in t.extra.iter().filter(|(k, _)| k.as_str() != "tools") {
```

(same shape with `allowed_tools` / `"allowed-tools"` in `render_skill`).

- [ ] **Step 4: Run to verify pass**

Run: `cd src-tauri && cargo test service::catalog`
Expected: all pass, including the round-trip test. If `serde_yaml` renders the description `Make a worktree.` quoted, the round-trip assertion fails on the skill; in that case compare frontmatter fields semantically (parse both with `parse_frontmatter` and compare mappings + body) instead of byte equality, and note it in the commit message.

- [ ] **Step 5: Format, lint, commit**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/service/catalog
git commit -m "feat(catalog): import skills, agents, hooks, MCP servers and plugins from ~/.claude

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 10: Tauri IPC commands

**Files:**
- Create: `src-tauri/src/commands/assets.rs`
- Modify: `src-tauri/src/commands/mod.rs` (`pub mod assets;`)
- Modify: `src-tauri/src/lib.rs` (`generate_handler!` list, before `pty::pty_open`)

**Interfaces:**
- Consumes: Task 8 and 9 service functions; `mcp::SETTING_TOKEN` + `Store::get_setting` for the fleet token.
- Produces commands: `catalog_config`, `catalog_configure`, `catalog_load`, `catalog_list_assets`, `catalog_get_asset`, `catalog_import_host`, `assets_scan_hosts`, `assets_inventory`. Wire shapes are the service structs (snake_case).

- [ ] **Step 1: Write the command wrappers**

`src-tauri/src/commands/assets.rs`:

```rust
//! Tauri IPC wrappers for the asset catalog. Logic lives in
//! `service::catalog`; this file only adapts `tauri::State` to plain refs.

use crate::ipc_error::IpcError;
use crate::service::catalog::{self, import::ImportReport, inventory, model::Kind, AssetDetail, AssetListing, ConfigureArgs, ImportArgs};
use crate::ssh::SshClient;
use crate::store::{AssetInventoryRow, CatalogConfigRow, Store};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(serde::Deserialize)]
pub struct LoadArgs {
    #[serde(default)]
    pub pull: bool,
}

#[derive(serde::Deserialize)]
pub struct GetAssetArgs {
    pub kind: Kind,
    pub name: String,
}

#[derive(serde::Deserialize)]
pub struct ScanArgs {
    pub host_alias: Option<String>,
}

#[tauri::command]
pub fn catalog_config(store: State<'_, Arc<Mutex<Store>>>) -> Result<Option<CatalogConfigRow>, IpcError> {
    catalog::config(&store)
}

#[tauri::command]
pub fn catalog_configure(args: ConfigureArgs, store: State<'_, Arc<Mutex<Store>>>) -> Result<CatalogConfigRow, IpcError> {
    catalog::configure(args, &store)
}

#[tauri::command]
pub fn catalog_load(args: LoadArgs, store: State<'_, Arc<Mutex<Store>>>) -> Result<crate::events::CatalogSummary, IpcError> {
    catalog::load(args.pull, &store)
}

#[tauri::command]
pub fn catalog_list_assets(store: State<'_, Arc<Mutex<Store>>>) -> Result<AssetListing, IpcError> {
    catalog::list_assets(&store)
}

#[tauri::command]
pub fn catalog_get_asset(args: GetAssetArgs, store: State<'_, Arc<Mutex<Store>>>) -> Result<AssetDetail, IpcError> {
    if !catalog::model::is_valid_name(&args.name) {
        return Err(IpcError::new("E_VALIDATION", format!("invalid asset name '{}'", args.name)));
    }
    catalog::get_asset(args.kind, &args.name, &store)
}

#[tauri::command]
pub fn catalog_import_host(args: ImportArgs, store: State<'_, Arc<Mutex<Store>>>) -> Result<ImportReport, IpcError> {
    let token = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.get_setting(crate::mcp::SETTING_TOKEN)?
    };
    catalog::import_host(args, &store, token.as_deref())
}

#[tauri::command]
pub async fn assets_scan_hosts(
    args: ScanArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<inventory::HostScanResult>, IpcError> {
    inventory::scan_hosts(&store, &ssh, args.host_alias.as_deref()).await
}

#[tauri::command]
pub fn assets_inventory(store: State<'_, Arc<Mutex<Store>>>) -> Result<Vec<AssetInventoryRow>, IpcError> {
    catalog::inventory(&store)
}
```

- [ ] **Step 2: Register**

`commands/mod.rs`: add `pub mod assets;` (alphabetically first). In `lib.rs` `generate_handler!`, immediately before `pty::pty_open,`:

```rust
            commands::assets::catalog_config,
            commands::assets::catalog_configure,
            commands::assets::catalog_load,
            commands::assets::catalog_list_assets,
            commands::assets::catalog_get_asset,
            commands::assets::catalog_import_host,
            commands::assets::assets_scan_hosts,
            commands::assets::assets_inventory,
```

- [ ] **Step 3: Build, lint, test**

Run: `cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: clean build, all tests pass. The `doc_gen` test that string-parses `generate_handler!` must still pass (it only lists command names).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands src-tauri/src/lib.rs
git commit -m "feat(catalog): Tauri IPC commands for catalog configure/load/list/get/import/scan

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 11: MCP tools `list_assets`, `scan_assets`, `import_assets`

**Files:**
- Modify: `src-tauri/src/mcp/tools.rs` (params structs near the other params; three tools at the end of the `#[tool_router] impl FleetTools` block)
- Regenerate: `docs/control-api-reference.md`

**Interfaces:**
- Consumes: Task 8 `catalog::list_assets`, Task 8 `inventory::scan_hosts`, Task 9 `catalog::import_host`.

- [ ] **Step 1: Add params and tools**

Params (next to `RepoCommitDiffParams`):

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ScanAssetsParams {
    /// Only scan this host alias. Omit to scan every reachable host.
    #[serde(default)]
    pub host_alias: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ImportAssetsParams {
    /// Host to import from. Only `local` (the fleet controller) is supported.
    pub host_alias: String,
    /// Report what would be created without writing anything. Default false.
    #[serde(default)]
    pub dry_run: bool,
}
```

Tools (at the end of the impl block, after the last existing tool):

```rust
    // ---- asset catalog ----

    #[tool(description = "List the asset catalog (skills, agents, hooks, MCP \
        servers, plugin refs) with each asset's per-host drift state from the \
        last scan, plus unmanaged assets found on hosts and catalog parse \
        problems. Requires catalog_configure + catalog_load in the app. Returns JSON.")]
    async fn list_assets(&self) -> Result<CallToolResult, McpError> {
        audit("list_assets", "");
        ok_json_compact(&catalog::list_assets(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Scan hosts for installed skills/agents/hooks/MCP \
        servers/plugins and recompute each catalog asset's state (in_sync | \
        drifted | missing | unmanaged | unsupported). Read-only on hosts. \
        Returns per-host results as JSON.")]
    async fn scan_assets(
        &self,
        Parameters(p): Parameters<ScanAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("scan_assets", &format!("host_alias={}", p.host_alias.as_deref().unwrap_or("*")));
        let res = catalog::inventory::scan_hosts(&self.store, &self.ssh, p.host_alias.as_deref())
            .await
            .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    #[tool(description = "Import a host's Claude config (~/.claude skills, \
        agents, hooks, ~/.claude.json MCP servers, installed plugins) into the \
        catalog repo working tree as IR assets. Never overwrites; collisions \
        are reported. Only host_alias `local` is supported. Returns the import \
        report as JSON.")]
    async fn import_assets(
        &self,
        Parameters(p): Parameters<ImportAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("import_assets", &format!("host_alias={} dry_run={}", p.host_alias, p.dry_run));
        let token = {
            let s = self.store.lock().map_err(|_| McpError::internal_error("store mutex poisoned", None))?;
            s.get_setting(crate::mcp::SETTING_TOKEN).map_err(|e| to_mcp_err(e.into()))?
        };
        let args = catalog::ImportArgs { host_alias: p.host_alias, dry_run: p.dry_run };
        let rep = catalog::import_host(args, &self.store, token.as_deref()).map_err(to_mcp_err)?;
        ok_json(&rep)
    }
```

Add `catalog` to the `use crate::service::{…}` import line.

- [ ] **Step 2: Regenerate the reference and run tests**

```bash
cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings
REGEN_DOCS=1 cargo test --manifest-path Cargo.toml reference_is_current
cargo test
```
Expected: `docs/control-api-reference.md` changes to include the three tools; all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/mcp/tools.rs docs/control-api-reference.md
git commit -m "feat(mcp): list_assets, scan_assets and import_assets tools

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 12: Frontend store and event wiring

**Files:**
- Create: `src/lib/assets.ts`, `src/lib/assets.test.ts`
- Modify: `src/lib/events.ts`, `src/lib/events.test.ts`

**Interfaces:**
- Produces TS types mirroring Rust: `CatalogConfigRow`, `CatalogSummary`, `AssetInventoryRow`, `HostState`, `AssetSummary`, `AssetListing`, `Problem`, `FileWrite`, `ConfigMerge`, `RenderPlan`, `Preview`, `AssetDetail`, `HostScanResult`, `ImportReport`, `AssetKind`.
- Stores: `catalogConfig: writable<CatalogConfigRow | null>`, `catalog: writable<AssetListing | null>`, `inventory: writable<AssetInventoryRow[]>`.
- Functions: `loadCatalogConfig`, `configureCatalog(repoPath, remoteUrl)`, `loadCatalog(pull)`, `loadAssets()`, `getAsset(kind, name)`, `importHost(hostAlias, dryRun)`, `scanHosts(hostAlias?)`, `loadInventory()`, `mergeInventoryRow(row)`, `clearInventoryFor(hostAlias, harness)`, `groupByKind(listing)`, `stateCounts(hosts)`.
- Events: `onAssetInventoryUpdated`, `onAssetInventoryCleared`, `onCatalogLoaded` handlers in `RowEventHandlers`.

- [ ] **Step 1: Write the failing tests**

`src/lib/assets.test.ts`:

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  inventory, catalog, catalogConfig,
  loadAssets, scanHosts, importHost, configureCatalog, loadCatalog,
  mergeInventoryRow, clearInventoryFor, groupByKind, stateCounts,
  type AssetInventoryRow, type AssetListing,
} from './assets';

const row = (over: Partial<AssetInventoryRow> = {}): AssetInventoryRow => ({
  host_alias: 'local', harness: 'claude', kind: 'skill', name: 's', state: 'in_sync',
  catalog_hash: 'c', host_hash: 'h', scanned_at: 1, ...over,
});

const listing: AssetListing = {
  head: 'abc', loaded_at: 1, problems: [],
  unmanaged: [row({ name: 'extra', state: 'unmanaged' })],
  assets: [
    { kind: 'skill', name: 's', version: '1', description: 'd', tags: [], hosts: [
      { host_alias: 'local', harness: 'claude', state: 'in_sync' },
      { host_alias: 'mefistos', harness: 'claude', state: 'drifted' },
    ] },
    { kind: 'agent', name: 'pm', version: '1', description: 'd', tags: [], hosts: [] },
  ],
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  inventory.set([]); catalog.set(null); catalogConfig.set(null);
});

describe('assets store', () => {
  it('loadAssets populates catalog', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(listing);
    const r = await loadAssets();
    expect(r.ok).toBe(true);
    expect(get(catalog)?.assets).toHaveLength(2);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_list_assets', undefined);
  });

  it('configureCatalog and loadCatalog pass args and patch config', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ repo_path: '/r', remote_url: null, head_commit: null, last_loaded_at: null });
    await configureCatalog('/r', '');
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_configure', { args: { repo_path: '/r', remote_url: null } });
    expect(get(catalogConfig)?.repo_path).toBe('/r');
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ head: 'h', loaded_at: 2, asset_count: 1, problem_count: 0 });
    const r = await loadCatalog(true);
    expect(r.ok).toBe(true);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_load', { args: { pull: true } });
    expect(get(catalogConfig)?.head_commit).toBe('h');
  });

  it('scanHosts and importHost pass optional args', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    await scanHosts();
    expect(mockedInvoke).toHaveBeenCalledWith('assets_scan_hosts', { args: { host_alias: null } });
    await scanHosts('mefistos');
    expect(mockedInvoke).toHaveBeenCalledWith('assets_scan_hosts', { args: { host_alias: 'mefistos' } });
    await importHost('local', true);
    expect(mockedInvoke).toHaveBeenCalledWith('catalog_import_host', { args: { host_alias: 'local', dry_run: true } });
  });

  it('mergeInventoryRow upserts by identity and clearInventoryFor prunes one host+harness', () => {
    mergeInventoryRow(row());
    mergeInventoryRow(row({ state: 'drifted' }));
    mergeInventoryRow(row({ host_alias: 'mefistos' }));
    expect(get(inventory)).toHaveLength(2);
    expect(get(inventory).find((r) => r.host_alias === 'local')?.state).toBe('drifted');
    clearInventoryFor('local', 'claude');
    expect(get(inventory)).toHaveLength(1);
    expect(get(inventory)[0].host_alias).toBe('mefistos');
  });

  it('groupByKind keeps kind order and stateCounts tallies', () => {
    const groups = groupByKind(listing);
    expect(groups.map((g) => g.kind)).toEqual(['skill', 'agent']);
    expect(groups[0].assets[0].name).toBe('s');
    expect(stateCounts(listing.assets[0].hosts)).toEqual({ in_sync: 1, drifted: 1, missing: 0, unsupported: 0 });
  });
});
```

Add to `src/lib/events.test.ts`:

```ts
  it('fires asset inventory and catalog handlers', async () => {
    const seen: string[] = [];
    await subscribeToRowEvents({
      onAssetInventoryUpdated: (row) => seen.push(`upd:${row.host_alias}:${row.name}`),
      onAssetInventoryCleared: (p) => seen.push(`clr:${p.host_alias}:${p.harness}`),
      onCatalogLoaded: (s) => seen.push(`cat:${s.head}`),
    });
    await vi.mocked(emit)('asset_inventory:cleared', { host_alias: 'local', harness: 'claude' });
    await vi.mocked(emit)('asset_inventory:updated', { host_alias: 'local', harness: 'claude', kind: 'skill', name: 's', state: 'in_sync', catalog_hash: null, host_hash: null, scanned_at: 1 });
    await vi.mocked(emit)('catalog:loaded', { head: 'h', loaded_at: 1, asset_count: 0, problem_count: 0 });
    expect(seen).toEqual(['clr:local:claude', 'upd:local:s', 'cat:h']);
  });
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm run test -- src/lib/assets.test.ts src/lib/events.test.ts`
Expected: FAIL (module `./assets` missing; handlers unknown).

- [ ] **Step 3: Implement `assets.ts`**

```ts
// Asset catalog store (sub-project 1). Mirrors src-tauri/src/service/catalog.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export type AssetKind = 'skill' | 'agent' | 'hook' | 'mcp_server' | 'plugin_ref';
export const KIND_ORDER: AssetKind[] = ['skill', 'agent', 'hook', 'mcp_server', 'plugin_ref'];
export const KIND_LABEL: Record<AssetKind, string> = {
  skill: 'Skills', agent: 'Agents', hook: 'Hooks', mcp_server: 'MCP servers', plugin_ref: 'Plugins',
};
export type AssetState = 'in_sync' | 'drifted' | 'missing' | 'unmanaged' | 'unsupported';

export interface CatalogConfigRow {
  repo_path: string;
  remote_url: string | null;
  head_commit: string | null;
  last_loaded_at: number | null;
}
export interface CatalogSummary { head: string; loaded_at: number; asset_count: number; problem_count: number }
export interface AssetInventoryRow {
  host_alias: string; harness: string; kind: string; name: string; state: string;
  catalog_hash: string | null; host_hash: string | null; scanned_at: number;
}
export interface HostState { host_alias: string; harness: string; state: string }
export interface Problem { path: string; message: string }
export interface AssetSummary {
  kind: string; name: string; version: string; description: string; tags: string[]; hosts: HostState[];
}
export interface AssetListing {
  head: string; loaded_at: number; assets: AssetSummary[]; unmanaged: AssetInventoryRow[]; problems: Problem[];
}
export interface FileWrite { path: string; bytes: string }
export interface ConfigMerge { file: string; json_path: string[]; mode: 'set' | 'append_unique' | 'subset'; value: unknown }
export interface RenderPlan { files: FileWrite[]; merges: ConfigMerge[]; placeholders: string[]; warnings: string[] }
export interface Preview { harness: string; plan: RenderPlan | null; unsupported: string | null }
export interface AssetDetail {
  asset: { kind: string; name: string; version: string; description: string; tags: string[]; body: string } & Record<string, unknown>;
  previews: Preview[];
  hosts: HostState[];
}
export interface HostScanResult { host: string; status: string; detail: string | null; rows: number }
export interface ImportReport { created: [string, string][]; problems: Problem[]; flagged_secrets: string[]; dry_run: boolean }

export const catalogConfig = writable<CatalogConfigRow | null>(null);
export const catalog = writable<AssetListing | null>(null);
export const inventory = writable<AssetInventoryRow[]>([]);

export async function loadCatalogConfig(): Promise<Result<CatalogConfigRow | null>> {
  const r = await invokeCmd<CatalogConfigRow | null>('catalog_config');
  if (r.ok) catalogConfig.set(r.value);
  return r;
}

export async function configureCatalog(repoPath: string, remoteUrl: string): Promise<Result<CatalogConfigRow>> {
  const r = await invokeCmd<CatalogConfigRow>('catalog_configure', {
    args: { repo_path: repoPath, remote_url: remoteUrl.trim() === '' ? null : remoteUrl.trim() },
  });
  if (r.ok) catalogConfig.set(r.value);
  return r;
}

export async function loadCatalog(pull: boolean): Promise<Result<CatalogSummary>> {
  const r = await invokeCmd<CatalogSummary>('catalog_load', { args: { pull } });
  if (r.ok) {
    catalogConfig.update((c) => (c ? { ...c, head_commit: r.value.head, last_loaded_at: r.value.loaded_at } : c));
  }
  return r;
}

export async function loadAssets(): Promise<Result<AssetListing>> {
  const r = await invokeCmd<AssetListing>('catalog_list_assets');
  if (r.ok) catalog.set(r.value);
  return r;
}

export function getAsset(kind: string, name: string): Promise<Result<AssetDetail>> {
  return invokeCmd<AssetDetail>('catalog_get_asset', { args: { kind, name } });
}

export function importHost(hostAlias: string, dryRun: boolean): Promise<Result<ImportReport>> {
  return invokeCmd<ImportReport>('catalog_import_host', { args: { host_alias: hostAlias, dry_run: dryRun } });
}

export function scanHosts(hostAlias?: string): Promise<Result<HostScanResult[]>> {
  return invokeCmd<HostScanResult[]>('assets_scan_hosts', { args: { host_alias: hostAlias ?? null } });
}

export async function loadInventory(): Promise<Result<AssetInventoryRow[]>> {
  const r = await invokeCmd<AssetInventoryRow[]>('assets_inventory');
  if (r.ok) inventory.set(r.value);
  return r;
}

const key = (r: { host_alias: string; harness: string; kind: string; name: string }) =>
  [r.host_alias, r.harness, r.kind, r.name].join('::');

export function mergeInventoryRow(row: AssetInventoryRow): void {
  inventory.update((arr) => {
    const i = arr.findIndex((r) => key(r) === key(row));
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export function clearInventoryFor(hostAlias: string, harness: string): void {
  inventory.update((arr) => arr.filter((r) => !(r.host_alias === hostAlias && r.harness === harness)));
}

export interface KindGroup { kind: AssetKind; label: string; assets: AssetSummary[] }

export function groupByKind(listing: AssetListing): KindGroup[] {
  return KIND_ORDER
    .map((kind) => ({ kind, label: KIND_LABEL[kind], assets: listing.assets.filter((a) => a.kind === kind) }))
    .filter((g) => g.assets.length > 0);
}

export function stateCounts(hosts: HostState[]): Record<'in_sync' | 'drifted' | 'missing' | 'unsupported', number> {
  const c = { in_sync: 0, drifted: 0, missing: 0, unsupported: 0 };
  for (const h of hosts) if (h.state in c) c[h.state as keyof typeof c] += 1;
  return c;
}
```

In `events.ts`: import `type { AssetInventoryRow, CatalogSummary } from './assets'`, add to `RowEventHandlers`:

```ts
  onAssetInventoryUpdated?: (row: AssetInventoryRow) => void;
  onAssetInventoryCleared?: (payload: { host_alias: string; harness: string }) => void;
  onCatalogLoaded?: (summary: CatalogSummary) => void;
```

and to the `Promise.all` array:

```ts
    sub<AssetInventoryRow>('asset_inventory:updated', handlers.onAssetInventoryUpdated),
    sub<{ host_alias: string; harness: string }>('asset_inventory:cleared', handlers.onAssetInventoryCleared),
    sub<CatalogSummary>('catalog:loaded', handlers.onCatalogLoaded),
```

- [ ] **Step 4: Run to verify pass**

Run: `pnpm run test -- src/lib/assets.test.ts src/lib/events.test.ts && pnpm run check`
Expected: PASS, no type errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/assets.ts src/lib/assets.test.ts src/lib/events.ts src/lib/events.test.ts
git commit -m "feat(ui): assets store and catalog/inventory row events

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 13: Assets tab UI

**Files:**
- Create: `src/lib/AssetsPanel.svelte`, `src/lib/AssetList.svelte`, `src/lib/AssetDetail.svelte`, `src/lib/ImportDialog.svelte`, `src/lib/AssetsPanel.test.ts`
- Modify: `src/App.svelte` (imports, `viewMode` state, third tab, slot, event handlers, bootstrap)

**Interfaces:**
- Consumes: Task 12 store and functions; `hosts` store from `./hosts`.
- Produces: `AssetsPanel` (no props), `AssetList` props `{ listing: AssetListing; selected: {kind, name} | null; filter: string; onselect(kind, name); onimport(row) }`, `AssetDetail` props `{ kind: string; name: string; hosts: HostRow[] }`, `ImportDialog` props `{ onclose(); ondone() }`.

- [ ] **Step 1: Write the failing component tests**

`src/lib/AssetsPanel.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AssetsPanel from './AssetsPanel.svelte';
import { catalog, catalogConfig, inventory } from './assets';
import { hosts } from './hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

function byCmd(map: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in map) return map[cmd];
    throw { code: 'E_TEST', message: `unexpected ${cmd}` };
  });
}

const listing = {
  head: 'abcdef1234567890', loaded_at: 1, problems: [{ path: 'hooks/bad.yaml', message: 'name' }],
  unmanaged: [{ host_alias: 'local', harness: 'claude', kind: 'skill', name: 'extra', state: 'unmanaged', catalog_hash: null, host_hash: null, scanned_at: 1 }],
  assets: [
    { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: [], hosts: [
      { host_alias: 'local', harness: 'claude', state: 'in_sync' },
      { host_alias: 'mefistos', harness: 'claude', state: 'missing' },
    ] },
    { kind: 'mcp_server', name: 'fleet', version: '1', description: 'd', tags: [], hosts: [] },
  ],
};

beforeEach(() => {
  invoke.mockReset();
  catalog.set(null); catalogConfig.set(null); inventory.set([]);
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: false, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: null, account_uuid: null, provisioned: true },
  ]);
});

describe('AssetsPanel', () => {
  it('shows the setup card when no catalog is configured and configures on submit', async () => {
    byCmd({ catalog_config: null, catalog_configure: { repo_path: '/r', remote_url: null, head_commit: null, last_loaded_at: null }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 0, problem_count: 0 }, catalog_list_assets: { ...listing, assets: [], unmanaged: [], problems: [] }, assets_inventory: [] });
    render(AssetsPanel);
    await tick(); await tick();
    expect(screen.getByTestId('assets-setup')).toBeTruthy();
    await fireEvent.input(screen.getByTestId('assets-setup-path'), { target: { value: '/r' } });
    await fireEvent.click(screen.getByTestId('assets-setup-submit'));
    await tick(); await tick(); await tick();
    expect(invoke).toHaveBeenCalledWith('catalog_configure', { args: { repo_path: '/r', remote_url: null } });
    expect(invoke).toHaveBeenCalledWith('catalog_load', { args: { pull: false } });
    expect(screen.queryByTestId('assets-setup')).toBeNull();
  });

  it('lists assets grouped by kind with state chips, unmanaged group and problems badge', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'abcdef1234567890', last_loaded_at: 1 }, catalog_load: { head: 'abcdef1234567890', loaded_at: 1, asset_count: 2, problem_count: 1 }, catalog_list_assets: listing, assets_inventory: [] });
    render(AssetsPanel);
    await tick(); await tick(); await tick();
    expect(screen.getByText('Skills')).toBeTruthy();
    expect(screen.getByText('MCP servers')).toBeTruthy();
    expect(screen.getByTestId('asset-row-skill-worktree').textContent).toContain('1 in sync');
    expect(screen.getByTestId('asset-row-skill-worktree').textContent).toContain('1 missing');
    expect(screen.getByText('On hosts, not in catalog')).toBeTruthy();
    expect(screen.getByTestId('unmanaged-row-local-claude-skill-extra')).toBeTruthy();
    expect(screen.getByTestId('assets-problems').textContent).toContain('1');
    expect(screen.getByTestId('assets-head').textContent).toContain('abcdef1');
  });

  it('selecting an asset loads the detail with host matrix and preview switcher', async () => {
    byCmd({
      catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 },
      catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 },
      catalog_list_assets: listing, assets_inventory: [],
      catalog_get_asset: {
        asset: { kind: 'skill', name: 'worktree', version: '1', description: 'Make one.', tags: ['core'], body: '# b' },
        previews: [
          { harness: 'claude', plan: { files: [{ path: '~/.claude/skills/worktree/SKILL.md', bytes: '---\nname: worktree\n---\n# b' }], merges: [], placeholders: [], warnings: [] }, unsupported: null },
          { harness: 'codex', plan: null, unsupported: 'codex cannot render skill assets' },
        ],
        hosts: [{ host_alias: 'local', harness: 'claude', state: 'in_sync' }],
      },
    });
    render(AssetsPanel);
    await tick(); await tick(); await tick();
    await fireEvent.click(screen.getByTestId('asset-row-skill-worktree'));
    await tick(); await tick(); await tick();
    expect(invoke).toHaveBeenCalledWith('catalog_get_asset', { args: { kind: 'skill', name: 'worktree' } });
    expect(screen.getByTestId('asset-detail-title').textContent).toContain('worktree');
    expect(screen.getByTestId('matrix-cell-local-claude').textContent).toContain('in sync');
    expect(screen.getByTestId('matrix-cell-mefistos-claude').textContent).toContain('skipped');
    expect(screen.getByTestId('preview-file-path').textContent).toContain('SKILL.md');
    await fireEvent.click(screen.getByTestId('preview-tab-codex'));
    await tick();
    expect(screen.getByText(/codex cannot render/)).toBeTruthy();
  });

  it('scan button calls assets_scan_hosts and refreshes', async () => {
    byCmd({ catalog_config: { repo_path: '/r', remote_url: null, head_commit: 'h', last_loaded_at: 1 }, catalog_load: { head: 'h', loaded_at: 1, asset_count: 2, problem_count: 0 }, catalog_list_assets: listing, assets_inventory: [], assets_scan_hosts: [{ host: 'local', status: 'scanned', detail: null, rows: 3 }] });
    render(AssetsPanel);
    await tick(); await tick(); await tick();
    await fireEvent.click(screen.getByTestId('assets-scan'));
    await tick(); await tick(); await tick();
    expect(invoke).toHaveBeenCalledWith('assets_scan_hosts', { args: { host_alias: null } });
    expect(screen.getByTestId('assets-scan-result').textContent).toContain('local: scanned');
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm run test -- src/lib/AssetsPanel.test.ts`
Expected: FAIL (component missing).

- [ ] **Step 3: Implement the components**

`src/lib/AssetList.svelte`:

```svelte
<script lang="ts">
  import { groupByKind, stateCounts, type AssetListing, type AssetInventoryRow } from './assets';

  let {
    listing,
    selected,
    filter,
    onselect,
    onimport,
  }: {
    listing: AssetListing;
    selected: { kind: string; name: string } | null;
    filter: string;
    onselect: (kind: string, name: string) => void;
    onimport: (row: AssetInventoryRow) => void;
  } = $props();

  const groups = $derived(
    groupByKind(listing).map((g) => ({
      ...g,
      assets: g.assets.filter((a) => filter === '' || a.name.includes(filter) || a.description.toLowerCase().includes(filter.toLowerCase())),
    })).filter((g) => g.assets.length > 0),
  );
  const unmanaged = $derived(listing.unmanaged.filter((r) => filter === '' || r.name.includes(filter)));
  const isSel = (kind: string, name: string) => selected?.kind === kind && selected?.name === name;
</script>

<div class="asset-list">
  {#each groups as g (g.kind)}
    <div class="group-header">{g.label} <span class="count">{g.assets.length}</span></div>
    {#each g.assets as a (a.name)}
      {@const c = stateCounts(a.hosts)}
      <button class="row" class:selected={isSel(a.kind, a.name)} onclick={() => onselect(a.kind, a.name)} data-testid={`asset-row-${a.kind}-${a.name}`}>
        <span class="name">{a.name}</span>
        <span class="chips">
          {#if c.in_sync}<span class="chip ok">{c.in_sync} in sync</span>{/if}
          {#if c.drifted}<span class="chip warn">{c.drifted} drifted</span>{/if}
          {#if c.missing}<span class="chip muted">{c.missing} missing</span>{/if}
          {#if c.unsupported}<span class="chip muted">{c.unsupported} unsupported</span>{/if}
        </span>
      </button>
    {/each}
  {/each}
  {#if unmanaged.length > 0}
    <div class="group-header">On hosts, not in catalog <span class="count">{unmanaged.length}</span></div>
    {#each unmanaged as r (`${r.host_alias}:${r.harness}:${r.kind}:${r.name}`)}
      <div class="row unmanaged" data-testid={`unmanaged-row-${r.host_alias}-${r.harness}-${r.kind}-${r.name}`}>
        <span class="name">{r.name}</span>
        <span class="meta">{r.kind} · {r.host_alias}</span>
        <button class="link" onclick={() => onimport(r)} title="Import from this host">Import</button>
      </div>
    {/each}
  {/if}
</div>

<style>
  .asset-list { overflow: auto; height: 100%; font-size: 13px; }
  .group-header { padding: 8px 10px 4px; color: var(--fg-muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; }
  .count { opacity: 0.7; margin-left: 4px; }
  .row { display: flex; align-items: center; gap: 8px; width: 100%; text-align: left; padding: 5px 10px; background: none; border: 0; color: var(--fg); cursor: pointer; }
  .row:hover { background: var(--bg-pane); }
  .row.selected { background: var(--bg-pane); box-shadow: inset 2px 0 0 var(--accent); }
  .row.unmanaged { cursor: default; }
  .name { flex: 1; font-family: ui-monospace, monospace; }
  .meta { color: var(--fg-muted); font-size: 11px; }
  .chips { display: flex; gap: 4px; }
  .chip { font-size: 10px; padding: 1px 6px; border-radius: 8px; border: 1px solid var(--border); }
  .chip.ok { color: #16a34a; } .chip.warn { color: #d97706; } .chip.muted { color: var(--fg-muted); }
  .link { background: none; border: 0; color: var(--accent); cursor: pointer; font-size: 12px; }
</style>
```

`src/lib/AssetDetail.svelte`:

```svelte
<script lang="ts">
  import { getAsset, type AssetDetail } from './assets';
  import type { HostRow } from './hosts';

  let { kind, name, hosts }: { kind: string; name: string; hosts: HostRow[] } = $props();

  let detail = $state<AssetDetail | null>(null);
  let error = $state<string | null>(null);
  let harnessTab = $state('claude');

  $effect(() => {
    const k = kind, n = name;
    detail = null; error = null;
    getAsset(k, n).then((r) => {
      if (k !== kind || n !== name) return;
      if (r.ok) detail = r.value; else error = r.error.message;
    });
  });

  const harnesses = $derived(detail ? detail.previews.map((p) => p.harness) : []);
  const preview = $derived(detail?.previews.find((p) => p.harness === harnessTab) ?? null);
  const visibleHosts = $derived(hosts.filter((h) => !h.hidden));

  function cell(hostAlias: string, harness: string): string {
    const host = visibleHosts.find((h) => h.alias === hostAlias);
    if (host && host.alias !== 'local' && !host.reachable) return 'skipped';
    const s = detail?.hosts.find((h) => h.host_alias === hostAlias && h.harness === harness)?.state;
    return s ? s.replace('_', ' ') : 'not scanned';
  }
</script>

<div class="detail">
  {#if error}
    <p class="error">{error}</p>
  {:else if !detail}
    <p class="muted">Loading…</p>
  {:else}
    <h3 data-testid="asset-detail-title"><span class="kind">{detail.asset.kind}</span> {detail.asset.name} <span class="ver">v{detail.asset.version}</span></h3>
    <p class="desc">{detail.asset.description}</p>
    {#if detail.asset.tags.length}<p class="tags">{#each detail.asset.tags as t}<span class="tag">{t}</span>{/each}</p>{/if}

    <h4>Hosts</h4>
    <table class="matrix">
      <thead><tr><th>host</th>{#each harnesses as h}<th>{h}</th>{/each}</tr></thead>
      <tbody>
        {#each visibleHosts as host (host.alias)}
          <tr>
            <td>{host.alias}</td>
            {#each harnesses as h}
              {@const s = cell(host.alias, h)}
              <td class={`state-${s.replace(' ', '-')}`} data-testid={`matrix-cell-${host.alias}-${h}`} title={s === 'skipped' ? 'host unreachable' : ''}>{s}</td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>

    <h4>Preview</h4>
    <div class="tabs" role="tablist">
      {#each harnesses as h}
        <button role="tab" class:active={harnessTab === h} aria-selected={harnessTab === h} onclick={() => (harnessTab = h)} data-testid={`preview-tab-${h}`}>{h}</button>
      {/each}
    </div>
    {#if preview?.unsupported}
      <p class="muted">{preview.unsupported}</p>
    {:else if preview?.plan}
      {#each preview.plan.warnings as w}<p class="warn">{w}</p>{/each}
      {#if preview.plan.placeholders.length}<p class="warn">Unresolved placeholders: {preview.plan.placeholders.join(', ')}</p>{/if}
      {#each preview.plan.files as f (f.path)}
        <div class="file">
          <div class="path" data-testid="preview-file-path">{f.path}</div>
          <pre>{f.bytes}</pre>
        </div>
      {/each}
      {#each preview.plan.merges as m (m.file + m.json_path.join('/'))}
        <div class="file">
          <div class="path">{m.file} → {m.json_path.join('.')} <span class="mode">({m.mode})</span></div>
          <pre>{JSON.stringify(m.value, null, 2)}</pre>
        </div>
      {/each}
      {#if preview.plan.files.length === 0 && preview.plan.merges.length === 0}<p class="muted">Nothing to install.</p>{/if}
    {/if}
  {/if}
</div>

<style>
  .detail { padding: 10px 14px; overflow: auto; height: 100%; font-size: 13px; }
  h3 { margin: 0 0 4px; font-size: 15px; font-family: ui-monospace, monospace; }
  .kind, .ver { color: var(--fg-muted); font-size: 11px; font-family: system-ui; }
  h4 { margin: 14px 0 6px; font-size: 11px; text-transform: uppercase; color: var(--fg-muted); }
  .desc { margin: 0; } .tags { margin: 4px 0 0; } .tag { border: 1px solid var(--border); border-radius: 8px; padding: 0 6px; font-size: 11px; margin-right: 4px; }
  .matrix { border-collapse: collapse; } .matrix th, .matrix td { text-align: left; padding: 3px 10px 3px 0; border-bottom: 1px solid var(--border); }
  .state-in-sync { color: #16a34a; } .state-drifted { color: #d97706; } .state-skipped, .state-not-scanned, .state-missing, .state-unsupported { color: var(--fg-muted); }
  .tabs { display: flex; gap: 2px; margin-bottom: 6px; } .tabs button { background: none; border: 1px solid var(--border); border-radius: 4px; padding: 2px 8px; color: var(--fg-muted); cursor: pointer; } .tabs button.active { color: var(--fg); border-color: var(--accent); }
  .file { margin: 6px 0; } .path { font-family: ui-monospace, monospace; font-size: 12px; color: var(--fg-muted); } .mode { opacity: 0.7; }
  pre { margin: 2px 0 0; padding: 8px; background: var(--bg-pane); border: 1px solid var(--border); border-radius: 4px; overflow: auto; max-height: 320px; font-size: 12px; }
  .muted { color: var(--fg-muted); } .warn { color: #d97706; margin: 2px 0; } .error { color: #dc2626; }
</style>
```

`src/lib/ImportDialog.svelte`:

```svelte
<script lang="ts">
  import { importHost, type ImportReport } from './assets';
  import { hosts } from './hosts';

  let { onclose, ondone }: { onclose: () => void; ondone: () => void } = $props();

  let hostAlias = $state('local');
  let report = $state<ImportReport | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function run(dryRun: boolean) {
    busy = true; error = null;
    const r = await importHost(hostAlias, dryRun);
    busy = false;
    if (!r.ok) { error = r.error.message; return; }
    report = r.value;
    if (!dryRun) ondone();
  }
</script>

<div class="modal-backdrop" onclick={onclose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Import assets">
    <h3>Import from host</h3>
    <p class="muted">Reads the host's Claude config and writes new assets into the catalog working tree. Existing catalog assets are never overwritten. Nothing is committed.</p>
    <label>Host
      <select bind:value={hostAlias} data-testid="import-host">
        {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
          <option value={h.alias} disabled={h.alias !== 'local'}>{h.alias}{h.alias !== 'local' ? ' (local only in this version)' : ''}</option>
        {/each}
      </select>
    </label>
    {#if error}<p class="error">{error}</p>{/if}
    {#if report}
      <h4>{report.dry_run ? 'Would create' : 'Created'} {report.created.length}</h4>
      <ul class="list">{#each report.created as [kind, name]}<li>{kind} <code>{name}</code></li>{/each}</ul>
      {#if report.problems.length}<h4>Problems {report.problems.length}</h4><ul class="list">{#each report.problems as p}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>{/if}
      {#if report.flagged_secrets.length}<h4>Secrets to replace</h4><ul class="list">{#each report.flagged_secrets as s}<li>{s}</li>{/each}</ul>{/if}
    {/if}
    <div class="actions">
      <button onclick={onclose}>Close</button>
      <button onclick={() => run(true)} disabled={busy} data-testid="import-dry-run">Dry run</button>
      <button class="primary" onclick={() => run(false)} disabled={busy || !report?.dry_run} data-testid="import-confirm" title={report?.dry_run ? '' : 'Run a dry run first'}>Import</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop { position: fixed; inset: 0; background: rgba(0,0,0,0.4); display: flex; align-items: center; justify-content: center; z-index: 50; }
  .dialog { background: var(--bg); border: 1px solid var(--border); border-radius: 8px; padding: 16px; width: 520px; max-height: 80vh; overflow: auto; }
  h3 { margin: 0 0 6px; } h4 { margin: 10px 0 4px; font-size: 12px; }
  .muted { color: var(--fg-muted); font-size: 12px; } .error { color: #dc2626; }
  .list { margin: 0; padding-left: 18px; font-size: 12px; max-height: 200px; overflow: auto; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; margin-top: 12px; }
  .primary { background: var(--accent); color: white; border: 0; border-radius: 4px; padding: 4px 10px; }
</style>
```

`src/lib/AssetsPanel.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    catalog, catalogConfig, loadCatalogConfig, configureCatalog, loadCatalog, loadAssets, loadInventory, scanHosts,
    type HostScanResult, type AssetInventoryRow,
  } from './assets';
  import { hosts } from './hosts';
  import AssetList from './AssetList.svelte';
  import AssetDetail from './AssetDetail.svelte';
  import ImportDialog from './ImportDialog.svelte';

  let setupPath = $state('~/agent-assets');
  let setupRemote = $state('');
  let busy = $state<'' | 'setup' | 'pull' | 'scan'>('');
  let error = $state<string | null>(null);
  let scanResults = $state<HostScanResult[] | null>(null);
  let showProblems = $state(false);
  let showImport = $state(false);
  let filter = $state('');
  let selected = $state<{ kind: string; name: string } | null>(null);

  async function refresh() {
    const [a, i] = await Promise.all([loadAssets(), loadInventory()]);
    if (!a.ok) error = a.error.message;
    if (!i.ok) error = i.error.message;
  }

  onMount(async () => {
    const c = await loadCatalogConfig();
    if (c.ok && c.value) {
      const l = await loadCatalog(false);
      if (!l.ok) { error = l.error.message; return; }
      await refresh();
    }
  });

  async function setup() {
    busy = 'setup'; error = null;
    const c = await configureCatalog(setupPath, setupRemote);
    if (!c.ok) { error = c.error.message; busy = ''; return; }
    const l = await loadCatalog(false);
    if (!l.ok) error = l.error.message; else await refresh();
    busy = '';
  }

  async function pull() {
    busy = 'pull'; error = null;
    const l = await loadCatalog(true);
    if (!l.ok) error = l.error.message; else await refresh();
    busy = '';
  }

  async function scan() {
    busy = 'scan'; error = null; scanResults = null;
    const r = await scanHosts();
    if (!r.ok) error = r.error.message; else { scanResults = r.value; await refresh(); }
    busy = '';
  }

  function onImportUnmanaged(_row: AssetInventoryRow) {
    showImport = true;
  }

  const shortHead = $derived(($catalogConfig?.head_commit ?? '').slice(0, 7));
</script>

<div class="assets-panel">
  {#if !$catalogConfig}
    <div class="setup" data-testid="assets-setup">
      <h3>Asset catalog</h3>
      <p class="muted">Point fleet at a git repo of skills, agents, hooks, MCP servers and plugin refs. A remote URL is cloned into the path when the path is empty.</p>
      <label>Local path <input bind:value={setupPath} data-testid="assets-setup-path" /></label>
      <label>Remote URL (optional) <input bind:value={setupRemote} placeholder="git@github.com:you/agent-assets.git" /></label>
      {#if error}<p class="error">{error}</p>{/if}
      <button class="primary" onclick={setup} disabled={busy !== ''} data-testid="assets-setup-submit">{busy === 'setup' ? 'Setting up…' : 'Use this catalog'}</button>
    </div>
  {:else}
    <div class="toolbar">
      <span class="path" title={$catalogConfig.repo_path}>{$catalogConfig.repo_path}</span>
      <span class="head" data-testid="assets-head">@ {shortHead || '—'}</span>
      <button onclick={pull} disabled={busy !== ''}>{busy === 'pull' ? 'Pulling…' : 'Pull'}</button>
      <button onclick={scan} disabled={busy !== ''} data-testid="assets-scan">{busy === 'scan' ? 'Scanning…' : 'Scan hosts'}</button>
      <button onclick={() => (showImport = true)} disabled={busy !== ''}>Import from host</button>
      {#if $catalog && $catalog.problems.length > 0}
        <button class="badge" onclick={() => (showProblems = !showProblems)} data-testid="assets-problems">{$catalog.problems.length} problems</button>
      {/if}
      <input class="filter" placeholder="filter" bind:value={filter} />
    </div>
    {#if error}<p class="error">{error}</p>{/if}
    {#if scanResults}
      <p class="scan-result" data-testid="assets-scan-result">{scanResults.map((r) => `${r.host}: ${r.status}${r.detail ? ` (${r.detail})` : ''}`).join(' · ')}</p>
    {/if}
    {#if showProblems && $catalog}
      <ul class="problems">{#each $catalog.problems as p}<li><code>{p.path}</code> {p.message}</li>{/each}</ul>
    {/if}
    <div class="body">
      <div class="left">
        {#if $catalog}
          <AssetList listing={$catalog} {selected} {filter} onselect={(kind, name) => (selected = { kind, name })} onimport={onImportUnmanaged} />
        {:else}
          <p class="muted">Loading…</p>
        {/if}
      </div>
      <div class="right">
        {#if selected}
          <AssetDetail kind={selected.kind} name={selected.name} hosts={$hosts} />
        {:else}
          <p class="muted empty">Select an asset.</p>
        {/if}
      </div>
    </div>
  {/if}
  {#if showImport}
    <ImportDialog onclose={() => (showImport = false)} ondone={() => { showImport = false; pull(); }} />
  {/if}
</div>

<style>
  .assets-panel { display: flex; flex-direction: column; height: 100%; }
  .setup { max-width: 480px; margin: 40px auto; display: flex; flex-direction: column; gap: 10px; }
  .setup label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; }
  .toolbar { display: flex; align-items: center; gap: 8px; padding: 6px 10px; border-bottom: 1px solid var(--border); font-size: 12px; }
  .path { color: var(--fg-muted); max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .head { font-family: ui-monospace, monospace; color: var(--fg-muted); }
  .badge { color: #d97706; }
  .filter { margin-left: auto; width: 160px; }
  .body { display: grid; grid-template-columns: 300px 1fr; flex: 1; min-height: 0; }
  .left { border-right: 1px solid var(--border); min-height: 0; overflow: auto; }
  .right { min-height: 0; overflow: auto; }
  .muted { color: var(--fg-muted); } .empty { padding: 14px; } .error { color: #dc2626; padding: 4px 10px; margin: 0; }
  .scan-result { font-size: 12px; padding: 4px 10px; margin: 0; color: var(--fg-muted); }
  .problems { font-size: 12px; margin: 0; padding: 4px 10px 4px 28px; }
  .primary { background: var(--accent); color: white; border: 0; border-radius: 4px; padding: 6px 10px; }
</style>
```

- [ ] **Step 4: Wire the tab into `App.svelte`**

Import: `import AssetsPanel from './lib/AssetsPanel.svelte';` and `import { mergeInventoryRow, clearInventoryFor, loadAssets } from './lib/assets';`.

Replace the boolean `filesMode` with a three-way mode, keeping the existing semantics:

```ts
  type ViewMode = 'terminal' | 'files' | 'assets';
  let viewMode = $state<ViewMode>('terminal');
  const filesMode = $derived(viewMode === 'files');
  $effect(() => {
    if (viewMode === 'files' && (!$selectedSession || $selectedSession.kind === 'bg')) viewMode = 'terminal';
  });
  function showTerminal() { viewMode = 'terminal'; }
  function showFiles() { if ($selectedSession) viewMode = 'files'; }
  function showAssets() { viewMode = 'assets'; }
```

In `onKeydown`, `filesMode = false` becomes `viewMode = 'terminal'`, and the condition `filesMode` becomes `viewMode !== 'terminal'`. Every other read of `filesMode` stays valid through the `$derived`.

Tabs: after the Files button add

```svelte
      <button
        class="view-tab"
        class:active={viewMode === 'assets'}
        role="tab"
        aria-selected={viewMode === 'assets'}
        onclick={showAssets}
        data-testid="tab-assets">Assets</button
      >
```

Slot: the Assets overlay is independent of the selected session, so place it outside the `{#if $selectedSession?.kind === 'bg'}` branch, at the end of `.right-body`:

```svelte
      {#if viewMode === 'assets'}
        <div class="view-slot overlay">
          <AssetsPanel />
        </div>
      {/if}
```

and change the Files condition to `{#if viewMode === 'files' && $selectedSession}`.

Row events, added to the `subscribeToRowEvents({...})` call:

```ts
      onAssetInventoryUpdated: mergeInventoryRow,
      onAssetInventoryCleared: (p) => clearInventoryFor(p.host_alias, p.harness),
      onCatalogLoaded: () => { void loadAssets(); },
```

- [ ] **Step 5: Run the tests, type-check, build**

Run: `pnpm run test -- src/lib/AssetsPanel.test.ts && pnpm run check && pnpm run test && pnpm run build`
Expected: the four new tests pass; `check` clean; the full suite shows only the pre-existing `localStorage` failures (compare with `main` if anything else fails); build succeeds.

- [ ] **Step 6: Commit**

```bash
git add src/lib/AssetsPanel.svelte src/lib/AssetList.svelte src/lib/AssetDetail.svelte src/lib/ImportDialog.svelte src/lib/AssetsPanel.test.ts src/App.svelte
git commit -m "feat(ui): Assets tab with catalog list, host matrix, previews and import dialog

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

### Task 14: Docs

**Files:**
- Modify: `docs/concepts.md` (new section after "Control API & tunnels")
- Modify: `docs/control-api.md` (three rows in the Tools table)

- [ ] **Step 1: concepts.md**

Insert before `## The terminal`:

```markdown
## Asset catalog

Skills, subagents, hooks, MCP servers and plugin references can be kept in a
git repo in a harness-neutral format and managed from the **Assets** tab.
Fleet loads the repo on the controller, renders every asset the way each
harness expects it (Claude Code fully; Codex CLI for skills and MCP servers),
scans hosts read-only for what is actually installed, and shows each asset
as in sync, drifted, missing or unsupported per host. Assets found on a host
but not in the catalog are listed as unmanaged and can be imported. This
version never writes to hosts; sync and in-app editing are later iterations.
The format and layout are specified in
`docs/superpowers/specs/2026-09-14-asset-catalog-design.md`.
```

- [ ] **Step 2: control-api.md**

Add to the Tools table, after the last existing row:

```markdown
| `list_assets` | Catalog assets with per-host drift state, unmanaged assets, parse problems. |
| `scan_assets` | Re-scan hosts (read-only) and recompute asset states. |
| `import_assets` | Import the controller's `~/.claude` into the catalog working tree (dry-run supported). |
```

- [ ] **Step 3: Commit**

```bash
git add docs/concepts.md docs/control-api.md
git commit -m "docs: describe the asset catalog and its MCP tools

Claude-Session: https://claude.ai/code/session_015qiY9nS4iAUqiuLXumrMvS"
```

---

## Plan self-review notes

- **Spec coverage.** IR (Task 1), render plan and trait (2), Claude renderer (3), Codex renderer (4), repo load/write/git and process-wide state (5), migration 018 and events (6), scan script, parser, `installed`, state computation (7), service entry points and scan runner (8), importer with symlinks, `extra` passthrough, placeholder substitution, dry-run, collisions (9), IPC commands (10), three MCP tools plus reference regen (11), store and events (12), Assets tab with setup card, toolbar, grouped list, unmanaged group, host matrix with skipped column, preview switcher, import dialog (13), docs (14). Error codes: `E_CATALOG_NOT_CONFIGURED` (8), `E_CATALOG_GIT` (5), `E_CATALOG_PARSE` (5), `E_ASSET_UNSUPPORTED` (2, 4, 9), `E_ASSET_EXISTS` (5, 9), `E_ASSET_NOT_FOUND` (8).
- **Deviations from the spec, deliberate.** (a) The loaded catalog is a process-wide `Lazy<RwLock<Option<Catalog>>>` rather than a managed Tauri state, so the MCP server needs no new plumbing. (b) `ConfigMerge` gained a `mode` (`set`, `append_unique`, `subset`) because hooks are array entries and plugin records carry extra install metadata; the sync engine will need the same distinction. (c) `asset_inventory:cleared` was added alongside `:updated` so the UI can prune rows a re-scan no longer sees without re-fetching. (d) The spec's `E_*` codes live as constants in `service/catalog/mod.rs`, not in `ipc_error.rs`, matching how other modules use string codes.
- **Type consistency checked.** `AssetInventoryRow` fields are identical in Rust (Task 6), TS (Task 12), and the tests in Tasks 7, 8, 13. `HostState`, `AssetListing`, `Preview`, `RenderPlan` shapes match between Task 8 and Task 12. `hook_asset_name` is defined in Task 7 and used in Task 9; `is_valid_name` in Task 1 and used in Task 10.
