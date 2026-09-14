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
        (
            "description",
            serde_yaml::Value::String(description.to_string()),
        ),
    ]);
    let text = format!("{fm}{body}");
    plan.note_placeholders(&text);
    plan.files.push(FileWrite {
        path: format!("{CODEX_SKILLS_DIR}/{name}/SKILL.md"),
        bytes: text.into_bytes(),
    });
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
                skill_file(&asset.header.name, &description, &asset.body, &mut plan);
                let dir = format!("{CODEX_SKILLS_DIR}/{}", asset.header.name);
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
        Err(IpcError::new(
            super::super::E_ASSET_UNSUPPORTED,
            "codex hosts are not scanned in this version",
        ))
    }

    fn installed(&self, _snap: &HostSnapshot) -> Vec<(Kind, String)> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::MergeMode;
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
    fn no_scanning() {
        assert!(Codex.scan_script().is_none());
        assert!(Codex.installed(&Default::default()).is_empty());
    }
}
