//! Codex CLI renderer (experimental): skills and MCP servers only.
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
    ConfigMerge, FileWrite, Harness, HostSnapshot, ManifestMerge, MergeMode, RenderPlan,
    Unsupported,
};
use crate::ipc_error::IpcError;
use crate::service::catalog::model::{Asset, AssetSpec, Kind};
use serde_json::{json, Value};

pub const CODEX_SKILLS_DIR: &str = "~/.codex/skills";
pub const CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";
pub const CODEX_MANIFEST_PATH: &str = "~/.codex/.fleet-assets.json";

const CONFIG_FILES: &[&str] = &[CODEX_CONFIG_PATH, CODEX_MANIFEST_PATH];

pub struct Codex;

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
            AssetSpec::Agent { .. } | AssetSpec::Hook { .. } | AssetSpec::PluginRef { .. } => {
                return Err(unsupported());
            }
        }
        Ok(plan)
    }

    /// Same shape as `Claude::scan_script`: hasher detection, `##HASHES` +
    /// file hashes under `.codex/skills`, a hash for each config file, then
    /// one `##CONFIG <path>` block per config file (base64, one line), then
    /// `##END`. No single quotes: the caller wraps the whole script in
    /// `shell::quote`.
    fn scan_script(&self) -> Option<String> {
        let mut s = String::new();
        s.push_str("cd \"$HOME\" || exit 0; ");
        s.push_str(
            "if command -v sha256sum >/dev/null 2>&1; then H=sha256sum; else H=\"shasum -a 256\"; fi; ",
        );
        s.push_str("echo \"##HASHES\"; ");
        // `-exec $H {} +` (not `-print0 | xargs -0 $H`): see `Claude::scan_script`
        // for why this matters for an existing-but-empty directory.
        s.push_str(
            "for d in .codex/skills; do if [ -d \"$d\" ]; then find -L \"$d\" -type f -exec $H {} + 2>/dev/null; fi; done; ",
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
        s.push_str("echo \"##END\"");
        Some(s)
    }

    fn parse_scan(&self, stdout: &str) -> Result<HostSnapshot, IpcError> {
        super::parse_scan_blocks(stdout, &|path, bytes| {
            if path == CODEX_CONFIG_PATH {
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
            if let Some(rest) = path.strip_prefix(&format!("{CODEX_SKILLS_DIR}/")) {
                if let Some((name, _)) = rest.split_once('/') {
                    push(Kind::Skill, name.to_string());
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

    fn manifest_path(&self) -> &'static str {
        CODEX_MANIFEST_PATH
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
    fn skill_renders_to_codex_skills_dir() {
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: worktree\ndescription: Make one.\nallowed_tools: [bash]\n",
        )
        .unwrap();
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
    fn skill_renders_under_install_as() {
        let mut a = Asset::from_yaml(
            None,
            "kind: skill\nname: s\ndescription: d\ninstall_as: foo_bar\n",
        )
        .unwrap();
        a.body = "b\n".into();
        let plan = Codex.render(&a).unwrap();
        assert_eq!(plan.files[0].path, "~/.codex/skills/foo_bar/SKILL.md");
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
    fn hooks_and_plugins_are_unsupported_agents_unless_render_as_skill() {
        let hook = Asset::from_yaml(None, "kind: hook\nname: h\ndescription: d\nevent: stop\naction: { type: command, command: x }\n").unwrap();
        assert!(Codex.render(&hook).is_err());
        let plugin = Asset::from_yaml(None, "kind: plugin_ref\nname: p\ndescription: d\nharness: claude\nmarketplace: { name: m, source: github, repo: o/r }\nplugin: p\nversion: latest\n").unwrap();
        assert!(Codex.render(&plugin).is_err());
        let agent = Asset::from_yaml(None, "kind: agent\nname: pm\ndescription: d\n").unwrap();
        assert!(Codex.render(&agent).is_err());
        let mut as_skill = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    render_as: skill\n",
        )
        .unwrap();
        as_skill.body = "prompt\n".into();
        let plan = Codex.render(&as_skill).unwrap();
        assert_eq!(plan.files[0].path, "~/.codex/skills/pm/SKILL.md");
        assert_eq!(
            plan.warnings,
            vec!["agent rendered as a codex skill (targets.codex.render_as)"]
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
        assert_eq!(plan.files[0].path, "~/.codex/skills/foo_bar/SKILL.md");
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

    /// Builds `##HASHES`/`##CONFIG` scan output with a real base64-encoded
    /// TOML `config.toml` block, the shape a live scan would produce.
    fn scan_fixture() -> String {
        use base64::Engine;
        let toml_text = "[mcp_servers.fleet]\nurl = \"http://127.0.0.1:4180/mcp\"\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(toml_text);
        format!(
            "##HASHES\naaaa  .codex/skills/worktree/SKILL.md\n##CONFIG ~/.codex/config.toml\n{b64}\n##CONFIG ~/.codex/.fleet-assets.json\n##END\n"
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
}
