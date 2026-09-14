//! Claude Code renderer, scanner and installed-asset enumeration.
#![allow(dead_code)]

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
    (
        "browser",
        "mcp__plugin_superpowers-chrome_chrome__use_browser",
    ),
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
    TIER_MAP
        .iter()
        .find(|(t, _)| *t == tier)
        .map(|(_, m)| *m)
        .unwrap_or("sonnet")
}
pub fn unmap_tier(model: &str) -> Option<&'static str> {
    TIER_MAP.iter().find(|(_, m)| *m == model).map(|(t, _)| *t)
}
pub fn map_event(event: &str) -> Option<&'static str> {
    EVENT_MAP.iter().find(|(n, _)| *n == event).map(|(_, c)| *c)
}
pub fn unmap_event(claude: &str) -> Option<&'static str> {
    EVENT_MAP
        .iter()
        .find(|(_, c)| *c == claude)
        .map(|(n, _)| *n)
}

/// Name of a hook asset derived from its Claude event and matcher, e.g.
/// `before-tool-bash`. Used by both the importer and `installed()` so the
/// two agree on identity.
pub fn hook_asset_name(claude_event: &str, matcher: Option<&str>) -> String {
    let event = unmap_event(claude_event)
        .unwrap_or(claude_event)
        .replace('_', "-")
        .to_lowercase();
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

/// Render a YAML frontmatter block. Values are emitted with `serde_yaml`,
/// which leaves an unambiguous plain scalar (e.g. `Make a worktree.`)
/// unquoted and quotes anything that would otherwise reparse differently or
/// not at all (a `": "`, a leading `#`/`-`/`:`/`*`, embedded newlines, ...) —
/// the rendered file must be YAML Claude Code can parse back, so quoting is
/// never optional here.
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
        let AssetSpec::Skill {
            allowed_tools,
            triggers,
            ..
        } = &a.spec
        else {
            return;
        };
        let t = a.target("claude");
        let mut description = a.header.description.clone();
        if !triggers.is_empty() {
            description = format!(
                "{} Triggers: {}",
                description.trim_end(),
                triggers.join(", ")
            );
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
        plan.files.push(FileWrite {
            path: format!("{dir}/SKILL.md"),
            bytes: text.into_bytes(),
        });
        for r in &a.resources {
            let rel = r.rel_path.strip_prefix("resources/").unwrap_or(&r.rel_path);
            plan.files.push(FileWrite {
                path: format!("{dir}/{rel}"),
                bytes: r.bytes.clone(),
            });
        }
    }

    fn render_agent(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::Agent { tools, model } = &a.spec else {
            return;
        };
        let t = a.target("claude");
        let mut fields: Vec<(&str, serde_yaml::Value)> = vec![
            ("name", yaml_str(&a.header.name)),
            ("description", yaml_str(&a.header.description)),
        ];
        if !tools.is_empty() {
            let mapped: Vec<String> = tools.iter().map(|x| map_tool(x)).collect();
            fields.push(("tools", yaml_str(&mapped.join(", "))));
        }
        let model_id = t
            .model
            .clone()
            .unwrap_or_else(|| map_tier(model).to_string());
        fields.push(("model", yaml_str(&model_id)));
        for (k, v) in &t.extra {
            fields.push((k.as_str(), json_to_yaml(v)));
        }
        let text = format!("{}{}", frontmatter(&fields), a.body);
        plan.note_placeholders(&text);
        plan.files.push(FileWrite {
            path: format!("{AGENTS_DIR}/{}.md", a.header.name),
            bytes: text.into_bytes(),
        });
    }

    fn render_hook(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::Hook {
            event,
            r#match,
            action,
        } = &a.spec
        else {
            return;
        };
        let Some(claude_event) = map_event(event) else {
            plan.warnings.push(format!("unknown hook event '{event}'"));
            return;
        };
        let mut entry = serde_json::Map::new();
        if let Some(m) = r#match {
            entry.insert("matcher".into(), json!(map_tool(&m.tool)));
        }
        let mut inner = serde_json::Map::new();
        inner.insert("type".into(), json!(action.kind));
        // Note placeholders in field order (command, then url, then
        // headers, ...) rather than from the final serialized JSON: a
        // `serde_json::Map` without the `preserve_order` feature is
        // BTreeMap-backed, so serializing it sorts keys alphabetically and
        // would report placeholders in that (alphabetical-key) order
        // instead of the order fields appear in the source asset.
        if let Some(c) = &action.command {
            plan.note_placeholders(c);
            inner.insert("command".into(), json!(c));
        }
        if let Some(u) = &action.url {
            plan.note_placeholders(u);
            inner.insert("url".into(), json!(u));
        }
        if !action.headers.is_empty() {
            for v in action.headers.values() {
                plan.note_placeholders(v);
            }
            inner.insert("headers".into(), json!(action.headers));
        }
        if let Some(t) = action.timeout_s {
            inner.insert("timeout".into(), json!(t));
        }
        for (k, v) in &a.target("claude").extra {
            plan.note_placeholders(&v.to_string());
            inner.insert(k.clone(), v.clone());
        }
        entry.insert("hooks".into(), json!([Value::Object(inner)]));
        let value = Value::Object(entry);
        plan.merges.push(ConfigMerge {
            file: SETTINGS_PATH.into(),
            json_path: vec!["hooks".into(), claude_event.into()],
            mode: MergeMode::AppendUnique,
            value,
        });
    }

    fn render_mcp(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::McpServer {
            transport,
            url,
            headers,
            command,
            args,
            env,
        } = &a.spec
        else {
            return;
        };
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), json!(transport));
        // Note placeholders per field, in the same order the value is
        // built (url, headers, command, args, env, extra), rather than
        // from the finished JSON: a `serde_json::Map` without the
        // `preserve_order` feature is BTreeMap-backed, so serializing it
        // sorts keys alphabetically and would report placeholders in that
        // order instead of the order fields appear in the source asset
        // (matches the same fix in `render_hook`).
        if transport == "http" {
            if let Some(u) = url {
                plan.note_placeholders(u);
            }
            obj.insert("url".into(), json!(url.clone().unwrap_or_default()));
            if !headers.is_empty() {
                for v in headers.values() {
                    plan.note_placeholders(v);
                }
                obj.insert("headers".into(), json!(headers));
            }
        } else {
            if let Some(c) = command {
                plan.note_placeholders(c);
            }
            obj.insert("command".into(), json!(command.clone().unwrap_or_default()));
            if !args.is_empty() {
                for arg in args {
                    plan.note_placeholders(arg);
                }
                obj.insert("args".into(), json!(args));
            }
            if !env.is_empty() {
                for v in env.values() {
                    plan.note_placeholders(v);
                }
                obj.insert("env".into(), json!(env));
            }
        }
        for (k, v) in &a.target("claude").extra {
            plan.note_placeholders(&v.to_string());
            obj.insert(k.clone(), v.clone());
        }
        let value = Value::Object(obj);
        plan.merges.push(ConfigMerge {
            file: CLAUDE_JSON_PATH.into(),
            json_path: vec!["mcpServers".into(), a.header.name.clone()],
            mode: MergeMode::Set,
            value,
        });
    }

    fn render_plugin(&self, a: &Asset, plan: &mut RenderPlan) {
        let AssetSpec::PluginRef {
            marketplace,
            plugin,
            version,
            ..
        } = &a.spec
        else {
            return;
        };
        let entry = if version == "latest" {
            json!({})
        } else {
            json!({ "version": version })
        };
        plan.merges.push(ConfigMerge {
            file: PLUGINS_PATH.into(),
            json_path: vec!["plugins".into(), format!("{plugin}@{}", marketplace.name)],
            mode: MergeMode::Subset,
            value: json!([entry]),
        });
    }
}

const CONFIG_FILES: &[&str] = &[SETTINGS_PATH, CLAUDE_JSON_PATH, PLUGINS_PATH];

impl Harness for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported> {
        let mut plan = RenderPlan::default();
        if !asset.target("claude").enabled {
            plan.warnings
                .push("disabled for claude by targets.claude.enabled".into());
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

    /// Prints `##HASHES` + `<sha256>  <home-relative path>` lines for every
    /// file under skills/ and agents/ (symlinks followed), then one
    /// `##CONFIG <path>` block per config file with its base64 content on
    /// one line, then `##END`. No single quotes: the caller wraps the whole
    /// script in `shell::quote`.
    fn scan_script(&self) -> Option<String> {
        let mut s = String::new();
        s.push_str("cd \"$HOME\" || exit 0; ");
        s.push_str(
            "if command -v sha256sum >/dev/null 2>&1; then H=sha256sum; else H=\"shasum -a 256\"; fi; ",
        );
        s.push_str("echo \"##HASHES\"; ");
        // `-exec $H {} +` (not `-print0 | xargs -0 $H`): `-exec ... +` only
        // ever runs the hasher when find has at least one match, so an
        // existing-but-empty directory produces no output instead of
        // invoking the hasher with zero paths (which reads stdin, producing
        // a bogus `<hash>  -` line or hanging on an open pipe).
        s.push_str(
            "for d in .claude/skills .claude/agents; do if [ -d \"$d\" ]; then find -L \"$d\" -type f -exec $H {} + 2>/dev/null; fi; done; ",
        );
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
                let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
                    continue;
                };
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    snap.configs.insert(path, v);
                }
                continue;
            }
            // `<hash>  <path>` (two spaces from sha256sum / shasum).
            if let Some((hash, path)) = line.split_once("  ") {
                let path = path.trim_start_matches("./");
                // Defensive: a hasher invoked with no path argument (should
                // no longer happen with `-exec ... +`, but tolerate it if a
                // host's `find`/`xargs` behaves unexpectedly) prints its
                // hash against stdin as `-`; that is not a real file.
                if path == "-" {
                    continue;
                }
                snap.files
                    .insert(format!("~/{path}"), hash.trim().to_string());
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
        if let Some(hooks) = snap
            .configs
            .get(SETTINGS_PATH)
            .and_then(|v| v.get("hooks"))
            .and_then(Value::as_object)
        {
            for (event, entries) in hooks {
                for e in entries.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                    let matcher = e.get("matcher").and_then(Value::as_str);
                    push(Kind::Hook, hook_asset_name(event, matcher));
                }
            }
        }
        if let Some(servers) = snap
            .configs
            .get(CLAUDE_JSON_PATH)
            .and_then(|v| v.get("mcpServers"))
            .and_then(Value::as_object)
        {
            for name in servers.keys() {
                push(Kind::McpServer, name.clone());
            }
        }
        if let Some(plugins) = snap
            .configs
            .get(PLUGINS_PATH)
            .and_then(|v| v.get("plugins"))
            .and_then(Value::as_object)
        {
            for key in plugins.keys() {
                let name = key.split('@').next().unwrap_or(key).to_string();
                push(Kind::PluginRef, name);
            }
        }
        out
    }
}

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
        a.resources.push(Resource {
            rel_path: "resources/scripts/go.sh".into(),
            bytes: b"echo hi\n".to_vec(),
        });
        let plan = Claude.render(&a).unwrap();
        assert_eq!(plan.files.len(), 2);
        let skill = plan
            .files
            .iter()
            .find(|f| f.path == "~/.claude/skills/worktree/SKILL.md")
            .unwrap();
        let text = String::from_utf8(skill.bytes.clone()).unwrap();
        assert_eq!(
            text,
            "---\nname: worktree\ndescription: Make a worktree.\nallowed-tools: Bash, Read\ndisable-model-invocation: true\n---\n# Worktree\n\nDo it.\n"
        );
        let res = plan
            .files
            .iter()
            .find(|f| f.path == "~/.claude/skills/worktree/scripts/go.sh")
            .unwrap();
        assert_eq!(res.bytes, b"echo hi\n");
        assert!(plan.merges.is_empty());
    }

    /// Parse the YAML frontmatter block out of a rendered `SKILL.md`/agent
    /// file (`---\n<yaml>---\n<body>`) into a mapping, so tests can assert
    /// on the semantic value of a field regardless of whether `serde_yaml`
    /// chose to quote it.
    fn parse_frontmatter(text: &str) -> serde_yaml::Mapping {
        let yaml = text.split("---\n").nth(1).expect("frontmatter block");
        serde_yaml::from_str(yaml).expect("frontmatter is valid YAML")
    }

    #[test]
    fn skill_triggers_fold_into_description() {
        // The controller's ruling on this golden: rendered SKILL.md must be
        // YAML Claude Code can parse back, so this asserts the semantic
        // value of `description` (via a YAML parse) rather than pinning the
        // exact quoting `serde_yaml` chooses to emit.
        let plan = render(
            "kind: skill\nname: s\ndescription: Base.\ntriggers: [\"foo\", \"bar\"]\n",
            "b\n",
        );
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        let map = parse_frontmatter(&text);
        assert_eq!(
            map.get("description").and_then(|v| v.as_str()),
            Some("Base. Triggers: foo, bar"),
            "{text}"
        );
    }

    #[test]
    fn skill_description_needing_quotes_round_trips_through_yaml() {
        // `": "` would be misread as a mapping separator and `#` would
        // start a comment if either were emitted unquoted; `frontmatter`
        // must quote this description so it reparses to the exact string.
        let plan = render(
            "kind: skill\nname: s\ndescription: \"Use when: sorting #1 items\"\n",
            "b\n",
        );
        let text = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        let map = parse_frontmatter(&text);
        assert_eq!(
            map.get("description").and_then(|v| v.as_str()),
            Some("Use when: sorting #1 items"),
            "{text}"
        );
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
    fn hook_with_unknown_event_warns_and_renders_nothing() {
        let plan = render(
            "kind: hook\nname: h\ndescription: d\nevent: on_full_moon\naction:\n  type: command\n  command: \"echo hi\"\n",
            "",
        );
        assert!(plan.files.is_empty());
        assert!(plan.merges.is_empty());
        assert_eq!(plan.warnings, vec!["unknown hook event 'on_full_moon'"]);
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
        assert_eq!(
            m.value,
            serde_json::json!({"type": "http", "url": "http://127.0.0.1:4180/mcp", "headers": {"Authorization": "Bearer ${T}"}})
        );
        let stdio = render(
            "kind: mcp_server\nname: jira\ndescription: d\ntransport: stdio\ncommand: npx\nargs: [\"-y\", \"jira-mcp\"]\nenv: { JIRA_TOKEN: \"${JIRA_TOKEN}\" }\n",
            "",
        );
        assert_eq!(
            stdio.merges[0].value,
            serde_json::json!({"type": "stdio", "command": "npx", "args": ["-y", "jira-mcp"], "env": {"JIRA_TOKEN": "${JIRA_TOKEN}"}})
        );
    }

    #[test]
    fn plugin_ref_renders_subset_merge_on_installed_plugins() {
        let pinned = render(
            "kind: plugin_ref\nname: superpowers\ndescription: d\nharness: claude\nmarketplace: { name: superpowers-marketplace, source: github, repo: obra/superpowers-marketplace }\nplugin: superpowers\nversion: \"6.3.0\"\n",
            "",
        );
        let m = &pinned.merges[0];
        assert_eq!(m.file, PLUGINS_PATH);
        assert_eq!(
            m.json_path,
            vec!["plugins", "superpowers@superpowers-marketplace"]
        );
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
        let plan = render(
            "kind: skill\nname: s\ndescription: d\ntargets:\n  claude:\n    enabled: false\n",
            "b",
        );
        assert!(plan.files.is_empty());
        assert_eq!(
            plan.warnings,
            vec!["disabled for claude by targets.claude.enabled"]
        );
    }

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
        assert_eq!(
            snap.configs[SETTINGS_PATH]["hooks"]["Stop"][0]["hooks"][0]["command"],
            "x"
        );
        assert_eq!(
            snap.configs[CLAUDE_JSON_PATH]["mcpServers"]["claude-fleet"]["type"],
            "http"
        );
        assert_eq!(
            snap.configs[PLUGINS_PATH]["plugins"]["superpowers@superpowers-marketplace"][0]
                ["version"],
            "6.3.0"
        );
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
        assert!(
            !s.contains('\''),
            "no single quotes: the whole script is passed through shell::quote"
        );
    }

    #[test]
    fn hook_names_are_stable() {
        assert_eq!(
            hook_asset_name("PreToolUse", Some("Bash")),
            "before-tool-bash"
        );
        assert_eq!(hook_asset_name("Stop", None), "stop");
        assert_eq!(hook_asset_name("Stop", Some("")), "stop");
        assert_eq!(
            hook_asset_name("PostToolUse", Some("EnterWorktree|ExitWorktree")),
            "after-tool-enterworktree-exitworktree"
        );
    }

    /// Runs the real `scan_script()` under `bash -lc` (through `shell::quote`,
    /// same as the real caller) against a temp `$HOME` with one real skill
    /// file plus an existing-but-empty `.claude/agents` dir, and feeds the
    /// output back through `parse_scan`. Regression test for the `-exec ...
    /// +` vs `xargs` fix: an empty scanned directory must not produce a
    /// bogus `~/-` entry (or hang), and only the one real file is reported.
    #[test]
    fn scan_script_runs_under_bash_and_parses_cleanly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();
        let skill_dir = home.join(".claude/skills/worktree");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            b"---\nname: worktree\n---\nbody\n",
        )
        .unwrap();
        // Exists but has no files: the case that hung / produced a bogus
        // `<hash>  -` line under `xargs -0` with zero input paths.
        std::fs::create_dir_all(home.join(".claude/agents")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), b"{}").unwrap();

        // `bash -lc <script>` via `std::process::Command` passes `script` as
        // one argv element straight to bash, with no intervening shell to
        // re-parse it — unlike the eventual remote caller (Task 8), which
        // embeds this same script, `shell::quote`d, inside an outer `ssh ...
        // bash -lc <quoted>` command line. Quoting here would double-quote:
        // the literal quote characters would become part of the script text
        // bash executes.
        let script = Claude.scan_script().unwrap();
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
        let snap = Claude.parse_scan(&stdout).unwrap();
        assert_eq!(
            snap.files.keys().collect::<Vec<_>>(),
            vec!["~/.claude/skills/worktree/SKILL.md"]
        );
        assert!(!snap.files.contains_key("~/-"));
        assert_eq!(snap.configs[SETTINGS_PATH], serde_json::json!({}));
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
