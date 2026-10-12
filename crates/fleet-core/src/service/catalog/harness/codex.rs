//! Codex CLI renderer (experimental): skills, subagents (TOML, one file per agent) and MCP servers.
//!
//! Codex's config file is TOML (`~/.codex/config.toml`), unlike Claude's JSON
//! files. `merge_config` bridges the two by parsing TOML into a
//! `serde_json::Value` tree (`toml::from_str::<toml::Value>` +
//! `serde_json::to_value`), running the same `apply_merges`/`remove_merges`
//! machinery Claude uses, then converting back
//! (`toml::Value::try_from` + `toml::to_string_pretty`). JSON's `null` has no
//! TOML representation, so any null left after merging is stripped before
//! the conversion back (`strip_nulls`). Because the file is fully
//! re-serialized, **comments and formatting in an existing `config.toml` are
//! not preserved** — acceptable for now since Codex support is experimental.

use super::claude::frontmatter;
use super::{
    canonical_json, dir_hash, mcp_secret_like, ConfigMerge, FileWrite, Harness, HostSnapshot,
    InstalledAsset, ManifestMerge, MergeMode, RenderPlan, Unsupported,
};
use crate::ipc_error::IpcError;
use crate::service::catalog::model::{sha256_hex, Asset, AssetSpec, Kind};
use crate::service::catalog::sync::plan::ActionOp;
use serde_json::{json, Value};

/// Where Codex reads user skills (multi-harness F3c): `~/.agents/skills`, the
/// cross-harness skills directory — not `~/.codex/skills`, which Codex keeps
/// for its own built-ins (`.system`).
pub const CODEX_SKILLS_DIR: &str = "~/.agents/skills";
pub const CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";
pub const CODEX_MANIFEST_PATH: &str = "~/.codex/.fleet-assets.json";
pub const CODEX_AGENTS_DIR: &str = "~/.codex/agents";
/// Where fleet rendered Codex skills before F3c. Still hashed by the scan
/// (minus Codex's `.system`) so a manifest entry pointing here can be
/// deleted under compare-and-swap when its skill moves to
/// `CODEX_SKILLS_DIR`; never listed as installed, since Codex does not read
/// it.
pub const CODEX_LEGACY_SKILLS_DIR: &str = "~/.codex/skills";

/// Render warning for an agent with `tools` (F3b): Codex subagents have no
/// per-agent tool allowlist. Shown in Asset detail's Codex preview.
pub const CODEX_AGENT_TOOLS_WARNING: &str = "codex subagents have no tool allowlist; `tools` is not applied (targets.codex.extra.sandbox_mode can restrict the agent)";

/// `targets.codex.extra` keys the agent render already sets itself: an
/// `extra` entry under one of these is dropped (with a warning) rather than
/// silently overwriting the asset's identity or its dedicated
/// `targets.codex.model`.
const CODEX_AGENT_RESERVED_KEYS: &[&str] =
    &["name", "description", "developer_instructions", "model"];

const CONFIG_FILES: &[&str] = &[CODEX_CONFIG_PATH, CODEX_MANIFEST_PATH];

/// The scan's Codex presence probe (see `scan_script`). POSIX sh with no
/// single quote: the whole script is wrapped in `shell::quote`.
const CODEX_PRESENT_PROBE: &str = "if command -v codex >/dev/null 2>&1 || [ -e .codex/auth.json ] || [ -d .codex/sessions ]; then echo \"##PRESENT\"; fi; ";

/// The scan's symlink probe (multi-harness F3c): `~/.agents`,
/// `~/.agents/skills` and each entry in it (where Codex skills are written,
/// including a dot-entry), and `~/.codex/skills` and each entry in IT
/// (review fix round 1, I1/M3) — where the migration deletes old copies, so
/// a *per-skill* symlink there (e.g. `~/.codex/skills/s` pointed at
/// `~/.claude/skills/s`) is just as dangerous as the whole directory: its
/// compare-and-swap hash would pass through the link and `rm` would delete
/// Claude's own file. `~/.codex/skills/.system` (Codex's own built-ins,
/// never fleet's) is excluded even though it starts with a dot. Some setups
/// point one of these at `~/.claude/skills`; fleet must never write or
/// remove Codex skills through it (`sync::plan`). POSIX sh, no single
/// quote; a glob that matches nothing stays literal and fails `-L`.
///
/// Each hit is two lines, like a `##CONFIG` block: `##LINK <path>` (via
/// `printf`, not `echo` — no behavioural difference here, just consistent
/// no-single-quote style), then the `readlink` target alone on the next
/// line (`tr -d` strips any newline it printed, and the trailing `echo`
/// guarantees exactly one line even when `readlink` fails). Never
/// `##LINK <path> -> <target>` on one line — a directory whose own name
/// contains `" -> "` would otherwise corrupt the parsed path
/// (`harness::parse_scan_blocks`).
const CODEX_LINK_PROBE: &str = "for l in .agents .agents/skills .agents/skills/* .agents/skills/.[!.]* .codex/skills .codex/skills/* .codex/skills/.[!.]*; do case \"$l\" in .codex/skills/.system) continue ;; esac; if [ -L \"$l\" ]; then printf \"%s\\n\" \"##LINK ~/$l\"; readlink \"$l\" 2>/dev/null | tr -d \"\\n\"; echo; fi; done; ";

/// A `targets.codex.extra` value as TOML: nulls stripped (TOML has none);
/// `None` when nothing representable is left.
fn json_to_toml(v: &Value) -> Option<toml::Value> {
    let mut v = v.clone();
    strip_nulls(&mut v);
    if v.is_null() {
        return None;
    }
    toml::Value::try_from(&v).ok()
}

pub struct Codex;

/// A top-level `~/.codex/agents/<name>.toml`, the only agent files
/// `installed` lists.
fn is_agent_toml(path: &str) -> bool {
    path.strip_prefix(&format!("{CODEX_AGENTS_DIR}/"))
        .and_then(|rest| rest.strip_suffix(".toml"))
        .is_some_and(|stem| !stem.is_empty() && !stem.contains('/'))
}

/// `mcp_secret_like` for a Codex MCP server table, which names its HTTP
/// headers `http_headers` rather than `headers`.
fn codex_mcp_secret_like(v: &Value) -> bool {
    mcp_secret_like(v)
        || v.get("http_headers")
            .and_then(Value::as_object)
            .is_some_and(|h| !h.is_empty())
}

/// Whether a scanned subagent TOML (as JSON) looks like it carries a
/// credential: any of its own `mcp_servers` does, by the MCP rule.
fn agent_secret_like(agent: &Value) -> bool {
    agent
        .get("mcp_servers")
        .and_then(Value::as_object)
        .is_some_and(|servers| servers.values().any(codex_mcp_secret_like))
}

/// `name` is the catalog name that stays in the `SKILL.md` frontmatter;
/// `install_name` (`Asset::install_name()`) is the identifier the skill
/// directory is derived from, so an `install_as` renders under the host
/// identifier while the frontmatter still names the catalog asset.
fn skill_file(
    name: &str,
    install_name: &str,
    description: &str,
    body: &str,
    plan: &mut RenderPlan,
) {
    let fm = frontmatter(&[
        ("name", serde_yaml::Value::String(name.to_string())),
        (
            "description",
            serde_yaml::Value::String(description.to_string()),
        ),
    ]);
    let text = format!("{fm}{body}");
    plan.note_placeholders(&text);
    plan.files.push(FileWrite {
        path: format!("{CODEX_SKILLS_DIR}/{install_name}/SKILL.md"),
        bytes: text.into_bytes(),
    });
}

/// Recursively drop JSON `null` values from objects and arrays. TOML has no
/// null representation, so anything left over after a merge/removal must be
/// stripped before `toml::Value::try_from` — otherwise the conversion fails.
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|_, val| !val.is_null());
            for val in map.values_mut() {
                strip_nulls(val);
            }
        }
        Value::Array(arr) => {
            arr.retain(|val| !val.is_null());
            for val in arr.iter_mut() {
                strip_nulls(val);
            }
        }
        _ => {}
    }
}

/// `serde`'s wire format for `toml::Value::Datetime`: a single-field struct
/// serialized as a map with this one private key
/// (`toml_datetime::datetime::FIELD`). `serde_json::to_value` on a
/// `toml::Value::Datetime` round-trips through this shape rather than
/// erroring, so a config file with a native TOML datetime would otherwise
/// silently turn into this bogus table on the way back to TOML.
const TOML_PRIVATE_DATETIME_KEY: &str = "$__toml_private_datetime";

/// Walk a parsed `toml::Value` for any `Datetime` node and return its dotted
/// path (`a.b`, with `[i]` array indices) the first time one is found.
/// `fleet` cannot round-trip TOML datetimes through JSON (see
/// `TOML_PRIVATE_DATETIME_KEY`), so `merge_config` must refuse to touch a
/// file containing one rather than silently corrupt it.
fn find_datetime_path(v: &toml::Value, path: &str) -> Option<String> {
    match v {
        toml::Value::Datetime(_) => Some(if path.is_empty() {
            "<root>".to_string()
        } else {
            path.to_string()
        }),
        toml::Value::Table(map) => map.iter().find_map(|(k, val)| {
            let child = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            find_datetime_path(val, &child)
        }),
        toml::Value::Array(arr) => arr
            .iter()
            .enumerate()
            .find_map(|(i, val)| find_datetime_path(val, &format!("{path}[{i}]"))),
        _ => None,
    }
}

/// Second guard, applied to the JSON tree right before converting it back to
/// TOML: is there any object anywhere in `v` whose only key is the toml
/// crate's private datetime marker? A well-formed merge value never
/// produces one (assets never set a "$__toml_private_datetime" field), so
/// this only fires if a raw private-datetime object slipped through
/// unnoticed — belt-and-suspenders alongside the `find_datetime_path` check
/// on the parsed input.
fn contains_toml_private_datetime(v: &Value) -> bool {
    match v {
        Value::Object(map) => {
            (map.len() == 1 && map.contains_key(TOML_PRIVATE_DATETIME_KEY))
                || map.values().any(contains_toml_private_datetime)
        }
        Value::Array(arr) => arr.iter().any(contains_toml_private_datetime),
        _ => false,
    }
}

impl Harness for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported> {
        let mut plan = RenderPlan::default();
        let t = asset.target("codex");
        if !t.enabled {
            plan.warnings
                .push("disabled for codex by targets.codex.enabled".into());
            return Ok(plan);
        }
        let unsupported = || Unsupported {
            harness: "codex",
            kind: asset.kind(),
        };
        match &asset.spec {
            AssetSpec::Skill { triggers, .. } => {
                // Codex skills have no dedicated triggers field either, so
                // fold them into the description exactly like
                // `claude::render_skill` does.
                let mut description = asset.header.description.clone();
                if !triggers.is_empty() {
                    description = format!(
                        "{} Triggers: {}",
                        description.trim_end(),
                        triggers.join(", ")
                    );
                }
                skill_file(
                    &asset.header.name,
                    asset.install_name(),
                    &description,
                    &asset.body,
                    &mut plan,
                );
                let dir = format!("{CODEX_SKILLS_DIR}/{}", asset.install_name());
                for r in &asset.resources {
                    let rel = r.rel_path.strip_prefix("resources/").unwrap_or(&r.rel_path);
                    plan.files.push(FileWrite {
                        path: format!("{dir}/{rel}"),
                        bytes: r.bytes.clone(),
                    });
                }
            }
            AssetSpec::Agent { .. } if t.render_as.as_deref() == Some("skill") => {
                skill_file(
                    &asset.header.name,
                    asset.install_name(),
                    &asset.header.description,
                    &asset.body,
                    &mut plan,
                );
                plan.warnings
                    .push("agent rendered as a codex skill (targets.codex.render_as)".into());
            }
            AssetSpec::McpServer {
                transport,
                url,
                headers,
                command,
                args,
                env,
            } => {
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
                    json_path: vec!["mcp_servers".into(), asset.install_name().to_string()],
                    mode: MergeMode::Set,
                    value,
                });
            }
            AssetSpec::Agent { tools, .. } => {
                // A Codex subagent (F3b): one TOML file per agent with the
                // required `name`, `description` and `developer_instructions`.
                // Built as a table and serialised by the toml crate, so
                // whatever the description or prompt holds is escaped.
                let mut table = toml::Table::new();
                table.insert(
                    "name".into(),
                    toml::Value::String(asset.header.name.clone()),
                );
                table.insert(
                    "description".into(),
                    toml::Value::String(asset.header.description.clone()),
                );
                table.insert(
                    "developer_instructions".into(),
                    toml::Value::String(asset.body.clone()),
                );
                // No tier → model mapping for Codex: without an explicit
                // `targets.codex.model` the user's configured model applies.
                if let Some(model) = &t.model {
                    table.insert("model".into(), toml::Value::String(model.clone()));
                }
                if !tools.is_empty() {
                    plan.warnings.push(CODEX_AGENT_TOOLS_WARNING.into());
                }
                for (k, v) in &t.extra {
                    if CODEX_AGENT_RESERVED_KEYS.contains(&k.as_str()) {
                        plan.warnings.push(if k == "model" {
                            "targets.codex.extra.model ignored: use targets.codex.model".to_string()
                        } else {
                            format!("targets.codex.extra.{k} ignored: set by the asset")
                        });
                        continue;
                    }
                    match json_to_toml(v) {
                        Some(tv) => {
                            table.insert(k.clone(), tv);
                        }
                        None => plan.warnings.push(format!(
                            "targets.codex.extra.{k} has no TOML form; not written"
                        )),
                    }
                }
                let text = match toml::to_string_pretty(&table) {
                    Ok(text) => text,
                    Err(e) => {
                        // Never an empty file in place of the agent: with no
                        // file the plan is a no-op whose reason is this
                        // warning (put first, which `plan_sync` reports).
                        tracing::warn!(
                            asset = %asset.header.name,
                            error = %e,
                            "codex subagent TOML could not be serialized"
                        );
                        plan.warnings
                            .insert(0, format!("codex subagent TOML could not be written: {e}"));
                        return Ok(plan);
                    }
                };
                plan.note_placeholders(&text);
                plan.files.push(FileWrite {
                    path: format!("{CODEX_AGENTS_DIR}/{}.toml", asset.install_name()),
                    bytes: text.into_bytes(),
                });
            }
            AssetSpec::Hook { .. } | AssetSpec::PluginRef { .. } | AssetSpec::Command { .. } => {
                return Err(unsupported());
            }
        }
        Ok(plan)
    }

    /// Same shape as `Claude::scan_script`: hasher detection, the presence
    /// probe, the symlink probe (`CODEX_LINK_PROBE`, `##LINK` lines),
    /// `##HASHES` + file hashes under `.agents/skills`,
    /// `.codex/skills` (legacy, minus `.system`) and `.codex/agents`, a hash
    /// for each config file, then one `##CONFIG <path>` block per config
    /// file (base64, one line), then `##END`. No single quotes: the caller
    /// wraps the whole script in `shell::quote`.
    fn scan_script(&self) -> Option<String> {
        let mut s = String::new();
        s.push_str("cd \"$HOME\" || exit 0; ");
        s.push_str(
            "if command -v sha256sum >/dev/null 2>&1; then H=sha256sum; else H=\"shasum -a 256\"; fi; ",
        );
        // Multi-harness F3a: is Codex itself here? `harness_set::harness_gate`
        // serves Codex on an auto host only when this line is printed (or
        // fleet already manages Codex assets there). Only evidence fleet
        // never writes counts: the CLI, its login (`auth.json`) or its
        // session logs — not `~/.codex` itself, which a Codex sync creates.
        s.push_str(CODEX_PRESENT_PROBE);
        s.push_str(CODEX_LINK_PROBE);
        s.push_str("echo \"##HASHES\"; ");
        // `-exec $H {} +` (not `-print0 | xargs -0 $H`): see `Claude::scan_script`
        // for why this matters for an existing-but-empty directory.
        // F3c: `.agents/skills` is where Codex reads skills. `.codex/skills`
        // is where fleet put them before — hashed only so a manifest entry
        // pointing there can be deleted under compare-and-swap; Codex's own
        // `.codex/skills/.system` is pruned, it is never fleet's.
        s.push_str(
            "for d in .agents/skills .codex/skills .codex/agents; do if [ -d \"$d\" ]; then find -L \"$d\" -path .codex/skills/.system -prune -o -type f -exec $H {} + 2>/dev/null; fi; done; ",
        );
        let config_rel: Vec<&str> = CONFIG_FILES
            .iter()
            .map(|f| f.trim_start_matches("~/"))
            .collect();
        s.push_str(&format!(
            "for f in {}; do if [ -f \"$f\" ]; then $H \"$f\"; fi; done; ",
            config_rel.join(" ")
        ));
        for f in CONFIG_FILES {
            let rel = f.trim_start_matches("~/");
            s.push_str(&format!(
                "echo \"##CONFIG {f}\"; if [ -f \"{rel}\" ]; then base64 < \"{rel}\" | tr -d \"\\n\"; fi; echo; "
            ));
        }
        // Each subagent's TOML too, so `installed_detail` can tell whether an
        // unmanaged one carries a credential (its `mcp_servers`).
        s.push_str(
            "for f in .codex/agents/*.toml; do if [ -f \"$f\" ]; then echo \"##CONFIG ~/$f\"; base64 < \"$f\" | tr -d \"\\n\"; echo; fi; done; ",
        );
        s.push_str("echo \"##END\"");
        Some(s)
    }

    fn parse_scan(&self, stdout: &str) -> Result<HostSnapshot, IpcError> {
        super::parse_scan_blocks(stdout, &|path, bytes| {
            if path == CODEX_CONFIG_PATH || is_agent_toml(path) {
                let text = std::str::from_utf8(bytes).ok()?;
                let toml_value: toml::Value = toml::from_str(text).ok()?;
                serde_json::to_value(toml_value).ok()
            } else {
                serde_json::from_slice::<Value>(bytes).ok()
            }
        })
    }

    fn installed(&self, snap: &HostSnapshot) -> Vec<(Kind, String)> {
        let mut out: Vec<(Kind, String)> = Vec::new();
        let mut push = |k: Kind, n: String| {
            if !out.iter().any(|(kk, nn)| *kk == k && *nn == n) {
                out.push((k, n));
            }
        };
        for path in snap.files.keys() {
            // F3c: only `CODEX_SKILLS_DIR`. A skill left in
            // `CODEX_LEGACY_SKILLS_DIR` is invisible to Codex, so it is not
            // Codex inventory (fleet's own copies there migrate on sync).
            if let Some(rest) = path.strip_prefix(&format!("{CODEX_SKILLS_DIR}/")) {
                if let Some((name, _)) = rest.split_once('/') {
                    push(Kind::Skill, name.to_string());
                }
            }
            if let Some(rest) = path.strip_prefix(&format!("{CODEX_AGENTS_DIR}/")) {
                if let Some(stem) = rest.strip_suffix(".toml") {
                    if !stem.contains('/') {
                        push(Kind::Agent, stem.to_string());
                    }
                }
            }
        }
        if let Some(servers) = snap
            .configs
            .get(CODEX_CONFIG_PATH)
            .and_then(|v| v.get("mcp_servers"))
            .and_then(Value::as_object)
        {
            for name in servers.keys() {
                push(Kind::McpServer, name.clone());
            }
        }
        out
    }

    fn installed_detail(&self, snap: &HostSnapshot) -> Vec<InstalledAsset> {
        self.installed(snap)
            .into_iter()
            .map(|(kind, name)| {
                let (hash, secret_like) = match kind {
                    Kind::Skill => (
                        dir_hash(snap, &format!("{CODEX_SKILLS_DIR}/{name}/")),
                        false,
                    ),
                    Kind::McpServer => {
                        let v = snap
                            .configs
                            .get(CODEX_CONFIG_PATH)
                            .and_then(|c| c.get("mcp_servers"))
                            .and_then(|m| m.get(&name));
                        (
                            v.map(|v| sha256_hex(canonical_json(v).as_bytes())),
                            v.is_some_and(codex_mcp_secret_like),
                        )
                    }
                    Kind::Agent => {
                        let path = format!("{CODEX_AGENTS_DIR}/{name}.toml");
                        (
                            snap.files.get(&path).cloned(),
                            snap.configs.get(&path).is_some_and(agent_secret_like),
                        )
                    }
                    _ => (None, false),
                };
                InstalledAsset {
                    kind,
                    name,
                    hash,
                    secret_like,
                    fleet_owned: false,
                }
            })
            .collect()
    }

    fn manifest_path(&self) -> &'static str {
        CODEX_MANIFEST_PATH
    }

    /// F3c, extended in review fix round 1 (I1/M5). A linked legacy
    /// `~/.codex/skills` — the whole directory, or any one entry under it
    /// (`~/.codex/skills/<name>`, the per-skill case the link probe now
    /// also reports) — only blocks removing old copies, and turning Codex
    /// off would not avoid that (a retiring host still runs those
    /// removals), so both get the same "old copies" wording: a per-entry
    /// link is itself an old copy, same as the whole directory. Elsewhere
    /// (today, only the *current* `~/.agents/skills`), a `Remove` (an
    /// orphaned manifest entry) reads differently from every other blocked
    /// op, since nothing is being written there either.
    fn symlink_reason(&self, link: &str, target: &str, op: ActionOp) -> String {
        // Case-insensitive (M3's reasoning applies here too, review fix
        // round 2): `linked_dir` already matches `link` against a path
        // case-insensitively, so a host whose `readlink` reports
        // `~/.Codex/Skills/s` must still read as the legacy directory.
        let link_lower = link.to_ascii_lowercase();
        if link_lower == CODEX_LEGACY_SKILLS_DIR
            || link_lower.starts_with(&format!("{CODEX_LEGACY_SKILLS_DIR}/"))
        {
            format!("{link} is a symlink (to {target}); fleet won't remove old Codex skill copies through it — replace it with a real directory")
        } else if op == ActionOp::Remove {
            format!("{link} is a symlink (to {target}); fleet won't remove Codex skills through it — replace it with a real directory")
        } else {
            format!("{link} is a symlink (to {target}); fleet won't write Codex skills through it — replace it with a real directory or turn Codex off for this host")
        }
    }

    /// Parses `existing` as TOML (an empty string is an empty document; a
    /// parse failure is `E_INVALID`), converts it to JSON, applies `merges`
    /// then `remove` via the shared `apply_merges`/`remove_merges`, strips
    /// any resulting JSON `null` (TOML cannot represent it), and converts
    /// back to TOML text. Comments and formatting in `existing` are not
    /// preserved — the whole file is re-serialized from the merged value.
    fn merge_config(
        &self,
        file: &str,
        existing: &str,
        merges: &[ConfigMerge],
        remove: &[ManifestMerge],
    ) -> Result<String, IpcError> {
        let mut root: Value = if existing.trim().is_empty() {
            json!({})
        } else {
            let toml_value: toml::Value = toml::from_str(existing).map_err(|e| {
                tracing::warn!(file, error = %e, "config file is not valid TOML");
                IpcError::new(
                    crate::ipc_error::codes::E_INVALID,
                    format!("{file} is not a valid TOML document"),
                )
            })?;
            // Fail closed on a native TOML datetime: `serde_json::to_value`
            // does not error on `toml::Value::Datetime`, it silently
            // round-trips it through the toml crate's private wire format
            // (`TOML_PRIVATE_DATETIME_KEY`) — converting that back to TOML
            // later would emit a bogus table in its place, corrupting
            // unrelated config. Refuse the whole merge instead.
            if let Some(path) = find_datetime_path(&toml_value, "") {
                tracing::warn!(file, key = %path, "config.toml contains a datetime; refusing to merge");
                return Err(IpcError::new(
                    crate::ipc_error::codes::E_INVALID,
                    format!(
                        "config.toml contains a datetime at {path}; fleet cannot merge this file yet"
                    ),
                ));
            }
            serde_json::to_value(toml_value).unwrap_or_else(|_| json!({}))
        };
        super::apply_merges(&mut root, merges);
        super::remove_merges(&mut root, remove);
        strip_nulls(&mut root);
        // Second guard, right before the JSON->TOML conversion: refuse a
        // raw toml-private-datetime marker object anywhere in the merged
        // tree rather than silently emitting it as a bogus TOML table (see
        // `contains_toml_private_datetime`).
        if contains_toml_private_datetime(&root) {
            tracing::warn!(
                file,
                "merged config contains a raw toml-private datetime marker; refusing to write"
            );
            return Err(IpcError::new(
                crate::ipc_error::codes::E_INVALID,
                format!("{file} merge result contains a raw datetime marker; fleet cannot write this file"),
            ));
        }
        let toml_value = toml::Value::try_from(&root).map_err(|e| {
            tracing::warn!(file, error = %e, "merged config cannot be represented as TOML");
            IpcError::new(
                crate::ipc_error::codes::E_INVALID,
                format!("{file} merge result cannot be represented as TOML"),
            )
        })?;
        let mut out = toml::to_string_pretty(&toml_value).unwrap_or_default();
        out.push('\n');
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::{value_hash, MergeMode};
    use crate::service::catalog::model::Asset;

    #[test]
    fn skill_renders_to_agents_skills_dir() {
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: worktree\ndescription: Make one.\nallowed_tools: [bash]\n",
        )
        .unwrap();
        a.body = "body\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "~/.agents/skills/worktree/SKILL.md");
        assert_eq!(
            String::from_utf8(plan.files[0].bytes.clone()).unwrap(),
            "---\nname: worktree\ndescription: Make one.\n---\nbody\n"
        );
    }

    #[test]
    fn skill_renders_under_install_as() {
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: s\ndescription: d\ninstall_as: foo_bar\n",
        )
        .unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files[0].path, "~/.agents/skills/foo_bar/SKILL.md");
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        let yaml = text.split("---\n").nth(1).expect("frontmatter block");
        let map: serde_yaml::Mapping = serde_yaml::from_str(yaml).expect("valid yaml");
        assert_eq!(
            map.get("name").and_then(|v| v.as_str()),
            Some("s"),
            "{text}"
        );
    }

    #[test]
    fn skill_triggers_fold_into_description() {
        // Asserts the semantic value of `description` via a YAML parse
        // rather than pinning the exact quoting `serde_yaml` chooses to
        // emit (a `": "` inside the folded string may need quoting) —
        // same rationale as `claude::render_skill`'s equivalent golden.
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: s\ndescription: Base.\ntriggers: [\"foo\", \"bar\"]\n",
        )
        .unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        let yaml = text.split("---\n").nth(1).expect("frontmatter block");
        let map: serde_yaml::Mapping = serde_yaml::from_str(yaml).expect("valid yaml");
        assert_eq!(
            map.get("description").and_then(|v| v.as_str()),
            Some("Base. Triggers: foo, bar"),
            "{text}"
        );
        assert!(text.ends_with("---\nb\n"), "{text}");
    }

    #[test]
    fn mcp_renders_toml_table_merge() {
        let a = Asset::from_yaml(None, "kind: mcp_server\nname: fleet\ndescription: d\ntransport: http\nurl: http://127.0.0.1:4180/mcp\n").unwrap();
        let plan = Codex.render(&a).unwrap();
        let m = &plan.merges[0];
        assert_eq!(m.file, CODEX_CONFIG_PATH);
        assert_eq!(m.json_path, vec!["mcp_servers", "fleet"]);
        assert_eq!(m.mode, MergeMode::Set);
        assert_eq!(
            m.value,
            serde_json::json!({"url": "http://127.0.0.1:4180/mcp"})
        );
        let s = Asset::from_yaml(None, "kind: mcp_server\nname: j\ndescription: d\ntransport: stdio\ncommand: npx\nargs: [x]\nenv: { A: b }\n").unwrap();
        assert_eq!(
            Codex.render(&s).unwrap().merges[0].value,
            serde_json::json!({"command": "npx", "args": ["x"], "env": {"A": "b"}})
        );
    }

    #[test]
    fn mcp_server_merges_under_install_as() {
        let a = Asset::from_yaml(None, "kind: mcp_server\nname: docs\ndescription: d\ntransport: http\nurl: u\ninstall_as: claude_ai_Docs\n").unwrap();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(
            plan.merges[0].json_path,
            vec!["mcp_servers", "claude_ai_Docs"]
        );
    }

    #[test]
    fn mcp_server_install_as_with_a_dot() {
        // `is_valid_install_name` permits `.`, so a dotted install name is
        // reachable. The merge is applied to a JSON tree and only then
        // converted to TOML, so the dot must become a *quoted key*, never a
        // nested table.
        let a = Asset::from_yaml(None, "kind: mcp_server\nname: docs\ndescription: d\ntransport: http\nurl: http://x\ninstall_as: a.b\n").unwrap();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.merges[0].json_path, vec!["mcp_servers", "a.b"]);

        let out = Codex
            .merge_config(CODEX_CONFIG_PATH, "", &plan.merges, &[])
            .unwrap();
        assert!(out.contains("[mcp_servers.\"a.b\"]"), "{out}");

        // ... and it round-trips back as the single key "a.b".
        let v: toml::Value = toml::from_str(&out).expect("output must be valid TOML");
        let servers = v["mcp_servers"].as_table().unwrap();
        assert_eq!(servers.keys().collect::<Vec<_>>(), vec!["a.b"]);
        assert_eq!(servers["a.b"]["url"].as_str(), Some("http://x"));
    }

    #[test]
    fn hooks_and_plugins_are_unsupported_and_render_as_skill_still_wins() {
        let hook = Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        assert!(Codex.render(&hook).is_err());
        let plugin = Asset::from_yaml(None, "kind: plugin_ref\nname: p\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: p\nversion: latest\n").unwrap();
        assert!(Codex.render(&plugin).is_err());
        let agent = Asset::from_yaml(None, "kind: agent\nname: pm\ndescription: d\n").unwrap();
        assert_eq!(
            Codex.render(&agent).unwrap().files[0].path,
            "~/.codex/agents/pm.toml",
            "since F3b a plain agent is a Codex subagent"
        );
        let mut as_skill = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    render_as: skill\n",
        )
        .unwrap();
        as_skill.body = "prompt\n".into();
        let plan = Codex.render(&as_skill).unwrap();
        assert_eq!(plan.files[0].path, "~/.agents/skills/pm/SKILL.md");
        assert_eq!(
            plan.warnings,
            vec!["agent rendered as a codex skill (targets.codex.render_as)"]
        );
    }

    fn toml_of(plan: &RenderPlan) -> toml::Table {
        toml::from_str(std::str::from_utf8(&plan.files[0].bytes).unwrap()).expect("valid TOML")
    }

    #[test]
    fn agent_renders_a_codex_subagent_toml() {
        let mut a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: Plans the work.\n",
        )
        .unwrap();
        a.body = "You plan.\nStep by step.\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "~/.codex/agents/pm.toml");
        let mut want = toml::Table::new();
        want.insert("name".into(), "pm".into());
        want.insert("description".into(), "Plans the work.".into());
        want.insert(
            "developer_instructions".into(),
            "You plan.\nStep by step.\n".into(),
        );
        assert_eq!(toml_of(&plan), want, "no model unless targets.codex.model");
        assert!(plan.warnings.is_empty());
        assert!(plan.merges.is_empty());
    }

    /// `targets.codex.extra` cannot override the identity fields the render
    /// itself sets (`name`, `description`, `developer_instructions`) or
    /// `model` (which has its own dedicated `targets.codex.model`) — each
    /// attempt is skipped with a warning naming the key, other keys still
    /// merge.
    #[test]
    fn agent_extra_cannot_override_identity_or_model() {
        let mut a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    extra:\n      name: other\n      description: other\n      developer_instructions: other\n      model: x\n      sandbox_mode: read-only\n",
        )
        .unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        let v = toml_of(&plan);
        assert_eq!(v["name"].as_str(), Some("pm"), "the catalog name wins");
        assert_eq!(v["description"].as_str(), Some("d"));
        assert_eq!(v["developer_instructions"].as_str(), Some("b\n"));
        assert!(
            v.get("model").is_none(),
            "no model unless targets.codex.model, {v:?}"
        );
        assert_eq!(v["sandbox_mode"].as_str(), Some("read-only"));
        let mut warnings = plan.warnings.clone();
        warnings.sort();
        assert_eq!(
            warnings,
            vec![
                "targets.codex.extra.description ignored: set by the asset".to_string(),
                "targets.codex.extra.developer_instructions ignored: set by the asset".to_string(),
                "targets.codex.extra.model ignored: use targets.codex.model".to_string(),
                "targets.codex.extra.name ignored: set by the asset".to_string(),
            ]
        );
    }

    #[test]
    fn agent_model_and_extra_come_from_targets_codex_only() {
        let mut a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntools: [read, bash]\nmodel: strong\ntargets:\n  codex:\n    model: gpt-5.4\n    extra:\n      model_reasoning_effort: high\n      sandbox_mode: read-only\n      dropped: null\n",
        )
        .unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        let v = toml_of(&plan);
        assert_eq!(
            v["model"].as_str(),
            Some("gpt-5.4"),
            "the tier is never mapped for codex"
        );
        assert_eq!(v["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(v["sandbox_mode"].as_str(), Some("read-only"));
        assert!(v.get("dropped").is_none());
        assert!(v.get("tools").is_none());
        assert_eq!(
            plan.warnings,
            vec![
                CODEX_AGENT_TOOLS_WARNING.to_string(),
                "targets.codex.extra.dropped has no TOML form; not written".to_string(),
            ]
        );
    }

    /// The TOML comes from the toml crate, so nothing in a description or
    /// prompt can close a string and inject a key.
    #[test]
    fn agent_toml_escapes_whatever_the_text_holds() {
        let mut a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: 'Says \"hi\" = [x]'\n",
        )
        .unwrap();
        a.body = "'''\n\"\"\"\nname = \"evil\"\n".into();
        let v = toml_of(&Codex.render(&a).unwrap());
        assert_eq!(v["name"].as_str(), Some("pm"));
        assert_eq!(v["description"].as_str(), Some("Says \"hi\" = [x]"));
        assert_eq!(
            v["developer_instructions"].as_str(),
            Some("'''\n\"\"\"\nname = \"evil\"\n")
        );
    }

    #[test]
    fn agent_renders_under_install_as_keeping_the_catalog_name() {
        let mut a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ninstall_as: pm_agent\n",
        )
        .unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files[0].path, "~/.codex/agents/pm_agent.toml");
        assert_eq!(toml_of(&plan)["name"].as_str(), Some("pm"));
    }

    #[test]
    fn a_codex_disabled_agent_renders_nothing() {
        let a = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    enabled: false\n",
        )
        .unwrap();
        let plan = Codex.render(&a).unwrap();
        assert!(plan.files.is_empty());
        assert_eq!(
            plan.warnings,
            vec!["disabled for codex by targets.codex.enabled"]
        );
    }

    #[test]
    fn installed_lists_agents_and_detail_hashes_their_toml() {
        let mut s = HostSnapshot::default();
        s.files
            .insert(format!("{CODEX_AGENTS_DIR}/pm.toml"), "ab".into());
        s.files
            .insert(format!("{CODEX_AGENTS_DIR}/notes.md"), "cd".into());
        s.files
            .insert(format!("{CODEX_AGENTS_DIR}/nested/x.toml"), "ef".into());
        assert_eq!(Codex.installed(&s), vec![(Kind::Agent, "pm".to_string())]);
        let d = Codex.installed_detail(&s);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].hash.as_deref(), Some("ab"));
        assert!(!d[0].secret_like && !d[0].fleet_owned);
    }

    /// An unmanaged subagent whose own `mcp_servers` carry `env` or
    /// `http_headers` reads as `secret_like`, by the same rule as a Codex MCP
    /// server in `config.toml`; one without reads clean.
    #[test]
    fn installed_detail_flags_a_subagent_with_secret_like_mcp_servers() {
        let mut s = HostSnapshot::default();
        for name in ["plain", "env", "headers"] {
            s.files
                .insert(format!("{CODEX_AGENTS_DIR}/{name}.toml"), "ab".into());
        }
        s.configs.insert(
            format!("{CODEX_AGENTS_DIR}/plain.toml"),
            json!({"name": "plain", "mcp_servers": {"fs": {"command": "npx"}}}),
        );
        s.configs.insert(
            format!("{CODEX_AGENTS_DIR}/env.toml"),
            json!({"name": "env", "mcp_servers": {"jira": {"command": "npx", "env": {"T": "x"}}}}),
        );
        s.configs.insert(
            format!("{CODEX_AGENTS_DIR}/headers.toml"),
            json!({"name": "headers", "mcp_servers": {"api": {"url": "https://x", "http_headers": {"Authorization": "Bearer x"}}}}),
        );
        let d = Codex.installed_detail(&s);
        let flag = |n: &str| d.iter().find(|a| a.name == n).unwrap().secret_like;
        assert!(!flag("plain"));
        assert!(flag("env"));
        assert!(flag("headers"));
    }

    /// The real scan hashes `~/.codex/agents`, and `installed` reads the
    /// agent back from it.
    #[cfg(unix)]
    #[test]
    fn scan_script_hashes_codex_agents_under_bash() {
        let tmp = tempfile::TempDir::new().unwrap();
        let agents = tmp.path().join(".codex/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("pm.toml"), b"name = \"pm\"\n").unwrap();
        let out = std::process::Command::new("bash")
            .arg("-lc")
            .arg(Codex.scan_script().unwrap())
            .env("HOME", tmp.path())
            .output()
            .expect("run scan script");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let snap = Codex
            .parse_scan(&String::from_utf8(out.stdout).unwrap())
            .unwrap();
        assert!(
            snap.files.contains_key("~/.codex/agents/pm.toml"),
            "{:?}",
            snap.files
        );
        assert_eq!(
            Codex.installed(&snap),
            vec![(Kind::Agent, "pm".to_string())]
        );
        assert_eq!(
            snap.configs["~/.codex/agents/pm.toml"]["name"], "pm",
            "the agent's TOML is read back for the secret-like check"
        );
    }

    #[test]
    fn agent_as_skill_renders_under_install_as() {
        let mut as_skill = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ninstall_as: foo_bar\ntargets:\n  codex:\n    render_as: skill\n",
        )
        .unwrap();
        as_skill.body = "prompt\n".into();
        let plan = Codex.render(&as_skill).unwrap();
        assert_eq!(plan.files[0].path, "~/.agents/skills/foo_bar/SKILL.md");
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        let yaml = text.split("---\n").nth(1).expect("frontmatter block");
        let map: serde_yaml::Mapping = serde_yaml::from_str(yaml).expect("valid yaml");
        assert_eq!(
            map.get("name").and_then(|v| v.as_str()),
            Some("pm"),
            "{text}"
        );
    }

    #[test]
    fn scan_script_is_home_relative_and_quoted() {
        let s = Codex.scan_script().unwrap();
        assert!(s.contains("##HASHES"));
        assert!(s.contains("##CONFIG ~/.codex/config.toml"));
        assert!(s.contains("##CONFIG ~/.codex/.fleet-assets.json"));
        assert!(s.contains(".codex/skills"));
        assert!(s.contains("base64"));
        assert!(
            !s.contains('\''),
            "no single quotes: the whole script is passed through shell::quote"
        );
        assert_eq!(Codex.manifest_path(), CODEX_MANIFEST_PATH);
    }

    /// F3a: the scan probes for Codex itself — the CLI on PATH, its login
    /// (`~/.codex/auth.json`) or its session logs (`~/.codex/sessions`),
    /// evidence a fleet sync never writes — before `##HASHES`, without any
    /// single quote (the caller wraps the whole script in `shell::quote`).
    #[test]
    fn scan_script_probes_for_codex() {
        let s = Codex.scan_script().unwrap();
        assert!(
            s.contains("if command -v codex >/dev/null 2>&1 || [ -e .codex/auth.json ] || [ -d .codex/sessions ]; then echo \"##PRESENT\"; fi; "),
            "{s}"
        );
        assert!(
            s.find("##PRESENT").unwrap() < s.find("##HASHES").unwrap(),
            "{s}"
        );
        assert!(!s.contains("[ -d .codex ]"), "{s}");
        assert!(!s.contains('\''));
    }

    /// Runs the scan under plain `sh` (no login profile) against a temp
    /// `$HOME`, with `PATH` limited to the system directories so the codex
    /// CLI of the machine running the test cannot answer for it.
    #[cfg(unix)]
    fn present_with(setup: impl FnOnce(&std::path::Path)) -> bool {
        let tmp = tempfile::TempDir::new().unwrap();
        setup(tmp.path());
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(Codex.scan_script().unwrap())
            .env("HOME", tmp.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run scan script");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Codex
            .parse_scan(&String::from_utf8(out.stdout).unwrap())
            .unwrap()
            .present
    }

    /// Codex's own login or session logs read as present; a `~/.codex` that
    /// holds only what a fleet sync writes (skills, agents, `config.toml`,
    /// the manifest) does not — that is fleet's own trace, not Codex.
    #[cfg(unix)]
    #[test]
    fn only_codex_own_state_reads_as_present() {
        if ["/usr/bin/codex", "/bin/codex"]
            .iter()
            .any(|p| std::path::Path::new(p).exists())
        {
            return; // the CLI itself answers on this machine
        }
        assert!(present_with(|h| {
            std::fs::create_dir_all(h.join(".codex/sessions")).unwrap();
        }));
        assert!(present_with(|h| {
            std::fs::create_dir_all(h.join(".codex")).unwrap();
            std::fs::write(h.join(".codex/auth.json"), b"{}").unwrap();
        }));
        assert!(!present_with(|h| {
            std::fs::create_dir_all(h.join(".codex/skills/s")).unwrap();
            std::fs::create_dir_all(h.join(".codex/agents")).unwrap();
            std::fs::write(h.join(".codex/skills/s/SKILL.md"), b"x").unwrap();
            std::fs::write(h.join(".codex/agents/pm.toml"), b"name = \"pm\"\n").unwrap();
            std::fs::write(h.join(".codex/config.toml"), b"").unwrap();
            std::fs::write(h.join(".codex/.fleet-assets.json"), b"{}").unwrap();
        }));
        assert!(!present_with(|_| {}));
    }

    /// Builds `##HASHES`/`##CONFIG` scan output with a real base64-encoded
    /// TOML `config.toml` block, the shape a live scan would produce.
    fn scan_fixture() -> String {
        use base64::Engine;
        let toml_text = "[mcp_servers.fleet]\nurl = \"http://127.0.0.1:4180/mcp\"\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(toml_text);
        format!(
            "##HASHES\naaaa  .agents/skills/worktree/SKILL.md\n##CONFIG ~/.codex/config.toml\n{b64}\n##CONFIG ~/.codex/.fleet-assets.json\n##END\n"
        )
    }

    #[test]
    fn parse_scan_reads_base64_toml_config() {
        let snap = Codex.parse_scan(&scan_fixture()).unwrap();
        assert_eq!(
            snap.configs[CODEX_CONFIG_PATH]["mcp_servers"]["fleet"]["url"],
            "http://127.0.0.1:4180/mcp"
        );
    }

    #[test]
    fn installed_lists_skill_and_server() {
        let snap = Codex.parse_scan(&scan_fixture()).unwrap();
        let mut got = Codex.installed(&snap);
        got.sort();
        assert_eq!(
            got,
            vec![
                (Kind::Skill, "worktree".to_string()),
                (Kind::McpServer, "fleet".to_string()),
            ]
        );
    }

    #[test]
    fn merge_config_golden_round_trip_and_removal() {
        let existing = "[mcp_servers.a]\nurl = \"x\"\n";
        let set = ConfigMerge {
            file: CODEX_CONFIG_PATH.into(),
            json_path: vec!["mcp_servers".into(), "fleet".into()],
            mode: MergeMode::Set,
            value: json!({"url": "http://127.0.0.1:4180/mcp"}),
        };
        let out = Codex
            .merge_config(CODEX_CONFIG_PATH, existing, std::slice::from_ref(&set), &[])
            .unwrap();
        assert!(out.ends_with('\n'));
        let v: toml::Value = toml::from_str(&out).expect("output must be valid TOML");
        assert_eq!(v["mcp_servers"]["a"]["url"].as_str(), Some("x"));
        assert_eq!(
            v["mcp_servers"]["fleet"]["url"].as_str(),
            Some("http://127.0.0.1:4180/mcp")
        );

        let remove = ManifestMerge {
            file: CODEX_CONFIG_PATH.into(),
            json_path: vec!["mcp_servers".into(), "a".into()],
            mode: MergeMode::Set,
            value_hash: value_hash(&json!({"url": "x"})),
        };
        let out2 = Codex
            .merge_config(CODEX_CONFIG_PATH, &out, &[], &[remove])
            .unwrap();
        let v2: toml::Value = toml::from_str(&out2).expect("output must be valid TOML");
        assert!(v2.get("mcp_servers").unwrap().get("a").is_none());
        assert_eq!(
            v2["mcp_servers"]["fleet"]["url"].as_str(),
            Some("http://127.0.0.1:4180/mcp")
        );
    }

    #[test]
    fn merge_config_empty_existing_produces_only_the_merge() {
        let set = ConfigMerge {
            file: CODEX_CONFIG_PATH.into(),
            json_path: vec!["mcp_servers".into(), "fleet".into()],
            mode: MergeMode::Set,
            value: json!({"url": "http://x"}),
        };
        let out = Codex
            .merge_config(CODEX_CONFIG_PATH, "", &[set], &[])
            .unwrap();
        let v: toml::Value = toml::from_str(&out).unwrap();
        assert_eq!(v["mcp_servers"]["fleet"]["url"].as_str(), Some("http://x"));
    }

    #[test]
    fn merge_config_rejects_invalid_toml() {
        let err = Codex
            .merge_config(CODEX_CONFIG_PATH, "not [ valid = toml =", &[], &[])
            .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
    }

    /// Regression for a controller-ruled fail-closed fix: a native TOML
    /// datetime survives `serde_json::to_value` as the toml crate's private
    /// wire format instead of erroring, and converting that back to TOML
    /// would silently emit a bogus table — corrupting unrelated config.
    /// `merge_config` must refuse the whole merge instead, naming the
    /// offending dotted key path.
    #[test]
    fn merge_config_rejects_existing_toml_with_a_datetime() {
        let existing = "created = 2024-01-01T00:00:00Z\n[mcp_servers.a]\nurl = \"x\"\n";
        let err = Codex
            .merge_config(CODEX_CONFIG_PATH, existing, &[], &[])
            .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(err.message.contains("created"), "{}", err.message);
        assert!(err.message.contains("datetime"), "{}", err.message);

        // Nested under a table too, not just at the top level.
        let nested = "[mcp_servers.a]\nurl = \"x\"\nupdated = 2024-01-01T00:00:00Z\n";
        let err2 = Codex
            .merge_config(CODEX_CONFIG_PATH, nested, &[], &[])
            .unwrap_err();
        assert_eq!(err2.code, "E_INVALID");
        assert!(
            err2.message.contains("mcp_servers.a.updated"),
            "{}",
            err2.message
        );
    }

    /// Regression: an existing `config.toml` with no datetimes anywhere
    /// still merges normally (the datetime guard must not be overbroad).
    #[test]
    fn merge_config_still_merges_when_no_datetime_present() {
        let existing = "[mcp_servers.a]\nurl = \"x\"\nenabled = true\ncount = 3\n";
        let set = ConfigMerge {
            file: CODEX_CONFIG_PATH.into(),
            json_path: vec!["mcp_servers".into(), "fleet".into()],
            mode: MergeMode::Set,
            value: json!({"url": "http://127.0.0.1:4180/mcp"}),
        };
        let out = Codex
            .merge_config(CODEX_CONFIG_PATH, existing, &[set], &[])
            .unwrap();
        let v: toml::Value = toml::from_str(&out).expect("output must be valid TOML");
        assert_eq!(v["mcp_servers"]["a"]["url"].as_str(), Some("x"));
        assert_eq!(v["mcp_servers"]["a"]["enabled"].as_bool(), Some(true));
        assert_eq!(v["mcp_servers"]["a"]["count"].as_integer(), Some(3));
        assert_eq!(
            v["mcp_servers"]["fleet"]["url"].as_str(),
            Some("http://127.0.0.1:4180/mcp")
        );
    }

    /// Runs the real `scan_script()` under `bash -lc` (through
    /// `shell::quote`, same as the real caller) against a temp `$HOME` with
    /// one real skill file plus a real `config.toml`, and feeds the output
    /// back through `parse_scan`. Mirrors
    /// `Claude::scan_script_runs_under_bash_and_parses_cleanly`.
    #[cfg(unix)]
    #[test]
    fn scan_script_runs_under_bash_and_parses_cleanly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".codex/skills/worktree");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            b"---\nname: worktree\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            b"[mcp_servers.fleet]\nurl = \"http://127.0.0.1:4180/mcp\"\n",
        )
        .unwrap();

        // `bash -lc <script>` via `std::process::Command`: see the Claude
        // equivalent test for why no extra quoting is applied here.
        let script = Codex.scan_script().unwrap();
        let out = std::process::Command::new("bash")
            .arg("-lc")
            .arg(&script)
            .env("HOME", home)
            .output()
            .expect("run scan script");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
        let snap = Codex.parse_scan(&stdout).unwrap();
        assert_eq!(
            snap.files.keys().collect::<Vec<_>>(),
            vec!["~/.codex/config.toml", "~/.codex/skills/worktree/SKILL.md"]
        );
        assert_eq!(
            snap.configs[CODEX_CONFIG_PATH]["mcp_servers"]["fleet"]["url"],
            "http://127.0.0.1:4180/mcp"
        );
    }

    #[test]
    fn codex_installed_detail_hashes_skills_and_flags_env() {
        let mut s = HostSnapshot::default();
        s.files
            .insert(format!("{CODEX_SKILLS_DIR}/worktree/SKILL.md"), "aa".into());
        s.configs.insert(
            CODEX_CONFIG_PATH.into(),
            serde_json::json!({"mcp_servers": {"jira": {"command": "npx", "env": {"T": "x"}}}}),
        );
        let d = Codex.installed_detail(&s);
        let skill = d.iter().find(|a| a.kind == Kind::Skill).unwrap();
        assert_eq!(
            skill.hash.as_deref(),
            Some(sha256_hex(b"SKILL.md=aa").as_str())
        );
        assert!(
            d.iter()
                .find(|a| a.kind == Kind::McpServer)
                .unwrap()
                .secret_like
        );
    }

    /// Runs the real scan under plain `sh` against `home`.
    #[cfg(unix)]
    fn scan_home(home: &std::path::Path) -> HostSnapshot {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(Codex.scan_script().unwrap())
            .env("HOME", home)
            .output()
            .expect("run scan script");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Codex
            .parse_scan(&String::from_utf8(out.stdout).unwrap())
            .unwrap()
    }

    /// F3c: skills are listed only from `~/.agents/skills`, where Codex reads
    /// them. A copy left in `~/.codex/skills` is invisible to Codex, and
    /// Codex's own `.system` is never a skill of the user's.
    #[test]
    fn codex_skills_are_listed_only_from_agents_skills() {
        let mut s = HostSnapshot::default();
        s.files
            .insert("~/.agents/skills/new/SKILL.md".into(), "aa".into());
        s.files
            .insert("~/.codex/skills/old/SKILL.md".into(), "bb".into());
        s.files.insert(
            "~/.codex/skills/.system/builtin/SKILL.md".into(),
            "cc".into(),
        );
        assert_eq!(Codex.installed(&s), vec![(Kind::Skill, "new".to_string())]);
        let d = Codex.installed_detail(&s);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].hash.as_deref(),
            Some(sha256_hex(b"SKILL.md=aa").as_str())
        );
    }

    /// F3c: the real scan hashes `~/.agents/skills` and still the legacy
    /// `~/.codex/skills` — a pre-F3c manifest entry there needs its hash for
    /// the compare-and-swap that deletes the old copy — but never Codex's
    /// `.system` built-ins.
    #[cfg(unix)]
    #[test]
    fn scan_hashes_agents_skills_and_the_legacy_dir_but_never_codex_system() {
        let home = tempfile::TempDir::new().unwrap();
        let h = home.path();
        for (rel, body) in [
            (".agents/skills/new/SKILL.md", "n"),
            (".codex/skills/old/SKILL.md", "o"),
            (".codex/skills/.system/builtin/SKILL.md", "b"),
        ] {
            let p = h.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        let snap = scan_home(h);
        assert!(
            snap.files.contains_key("~/.agents/skills/new/SKILL.md"),
            "{:?}",
            snap.files
        );
        assert!(
            snap.files.contains_key("~/.codex/skills/old/SKILL.md"),
            "the legacy copy keeps a hash for its removal: {:?}",
            snap.files
        );
        assert!(
            !snap.files.keys().any(|p| p.contains(".system")),
            "{:?}",
            snap.files
        );
        assert_eq!(
            Codex.installed(&snap),
            vec![(Kind::Skill, "new".to_string())]
        );
    }

    /// F3c: the scan reports a symlinked `~/.agents/skills` with its target
    /// (as `readlink` prints it) and still hashes what it points at — that
    /// is what Codex sees. A real directory reports no link.
    #[cfg(unix)]
    #[test]
    fn scan_reports_a_symlinked_agents_skills_dir() {
        use std::os::unix::fs::symlink;
        let home = tempfile::TempDir::new().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude/skills/s")).unwrap();
        std::fs::write(h.join(".claude/skills/s/SKILL.md"), b"claude").unwrap();
        std::fs::create_dir_all(h.join(".agents")).unwrap();
        symlink(h.join(".claude/skills"), h.join(".agents/skills")).unwrap();
        let snap = scan_home(h);
        assert_eq!(
            snap.links,
            std::collections::BTreeMap::from([(
                "~/.agents/skills".to_string(),
                h.join(".claude/skills").to_string_lossy().to_string()
            )])
        );
        assert!(
            snap.files.contains_key("~/.agents/skills/s/SKILL.md"),
            "{:?}",
            snap.files
        );

        let plain = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(plain.path().join(".agents/skills/s")).unwrap();
        assert!(scan_home(plain.path()).links.is_empty());
    }

    /// A whole linked `~/.agents`, one linked skill inside a real
    /// `~/.agents/skills`, and a linked legacy `~/.codex/skills` are each
    /// reported.
    #[cfg(unix)]
    #[test]
    fn scan_reports_a_linked_agents_dir_a_linked_skill_and_a_linked_legacy_dir() {
        use std::os::unix::fs::symlink;
        let whole = tempfile::TempDir::new().unwrap();
        let w = whole.path();
        std::fs::create_dir_all(w.join("dotfiles/agents/skills")).unwrap();
        symlink(w.join("dotfiles/agents"), w.join(".agents")).unwrap();
        let keys: Vec<String> = scan_home(w).links.into_keys().collect();
        assert_eq!(keys, vec!["~/.agents"]);

        let mixed = tempfile::TempDir::new().unwrap();
        let m = mixed.path();
        std::fs::create_dir_all(m.join(".claude/skills/x")).unwrap();
        std::fs::create_dir_all(m.join(".agents/skills")).unwrap();
        std::fs::create_dir_all(m.join(".codex")).unwrap();
        symlink(m.join(".claude/skills/x"), m.join(".agents/skills/x")).unwrap();
        symlink(m.join(".claude/skills"), m.join(".codex/skills")).unwrap();
        let keys: Vec<String> = scan_home(m).links.into_keys().collect();
        assert_eq!(keys, vec!["~/.agents/skills/x", "~/.codex/skills"]);
    }

    /// I1/M3 (F3c review fix round 1): a *per-entry* symlink under the
    /// legacy `~/.codex/skills/<name>` is reported, same as the whole
    /// directory — the migration's removal would otherwise `rm` straight
    /// through it. A dot-entry under `~/.agents/skills` is reported too,
    /// but Codex's own `~/.codex/skills/.system` never is, even though it
    /// also starts with a dot.
    #[cfg(unix)]
    #[test]
    fn scan_reports_a_per_entry_legacy_symlink_and_a_dot_entry_but_never_dot_system() {
        use std::os::unix::fs::symlink;
        let home = tempfile::TempDir::new().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".claude/skills/s")).unwrap();
        std::fs::create_dir_all(h.join(".codex/skills/.system/builtin")).unwrap();
        std::fs::create_dir_all(h.join(".agents/skills")).unwrap();
        symlink(h.join(".claude/skills/s"), h.join(".codex/skills/s")).unwrap();
        symlink(h.join(".claude/skills/s"), h.join(".agents/skills/.hidden")).unwrap();
        let snap = scan_home(h);
        assert_eq!(
            snap.links.into_keys().collect::<Vec<_>>(),
            vec!["~/.agents/skills/.hidden", "~/.codex/skills/s"],
            "no entry for .codex/skills/.system"
        );
    }

    /// F3c (plan 5/6 review fix): `symlink_reason` compares the legacy-dir
    /// check case-insensitively too, matching `linked_dir`'s own ASCII
    /// case-folding (M3) — a host whose `readlink`/scan reports
    /// `~/.Codex/Skills/s` (a case-insensitive filesystem) still gets the
    /// legacy "remove old copies" wording, not the generic "write ... or
    /// turn Codex off" one.
    #[test]
    fn symlink_reason_matches_the_legacy_dir_case_insensitively() {
        let reason = Codex.symlink_reason("~/.Codex/Skills/s", "/x", ActionOp::Create);
        assert_eq!(
            reason,
            "~/.Codex/Skills/s is a symlink (to /x); fleet won't remove old Codex skill copies through it — replace it with a real directory"
        );
    }
}
