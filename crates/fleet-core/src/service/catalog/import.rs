//! Import a Claude Code config directory into the catalog as IR assets.

use super::harness::claude::{hook_asset_name, unmap_event, unmap_tier, unmap_tool};
use super::model::{
    is_valid_install_name, is_valid_name, Asset, AssetSpec, Header, HookAction, HookMatch, Kind,
    Marketplace, Problem, Resource, Source, TargetOverride,
};
use super::repo::{asset_path, write_asset};
use super::E_ASSET_EXISTS;
use crate::ipc_error::codes::E_INVALID;
use crate::ipc_error::IpcError;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportReport {
    pub created: Vec<(String, String)>,
    pub problems: Vec<Problem>,
    /// Non-blocking notices: the asset was still created, but under a
    /// slugified name because its host identifier couldn't be used as
    /// `install_as` (see `install_as_for`). Never populated for hooks or
    /// plugin refs, which don't carry `install_as` at all.
    #[serde(default)]
    pub warnings: Vec<Problem>,
    pub flagged_secrets: Vec<String>,
    pub dry_run: bool,
}

pub struct ImportSources {
    pub claude_dir: PathBuf,
    pub claude_json: PathBuf,
}

impl ImportSources {
    pub fn for_local() -> Result<Self, IpcError> {
        let home = std::env::var("HOME")
            .map_err(|_| IpcError::new(crate::ipc_error::codes::E_IO, "HOME not set"))?;
        Ok(Self {
            claude_dir: Path::new(&home).join(".claude"),
            claude_json: Path::new(&home).join(".claude.json"),
        })
    }
}

/// Split `---\n…\n---\n` frontmatter from a markdown file.
pub fn parse_frontmatter(text: &str) -> (serde_yaml::Mapping, String) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (serde_yaml::Mapping::new(), text.to_string());
    };
    let Some(end) = rest.find("\n---\n") else {
        return (serde_yaml::Mapping::new(), text.to_string());
    };
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

/// Turn an arbitrary identifier (a skill folder name, an MCP server key, a
/// plugin id, ...) into a valid catalog asset name (`is_valid_name`):
/// lowercase, every run of characters outside `[a-z0-9]` collapsed to a
/// single `-`, leading/trailing `-` trimmed. An input with no `[a-z0-9]`
/// characters at all (including the empty string) slugifies to the empty
/// string, which is not a valid name — that (and only that) case is
/// prefixed with `x`.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_dash = false;
    for c in name.chars() {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            out.push(lower);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "x".to_string()
    } else {
        debug_assert!(
            is_valid_name(trimmed),
            "slugify produced an invalid name from {name:?}: {trimmed:?}"
        );
        trimmed.to_string()
    }
}

/// (Kind, slug) -> the original identifier that produced it, so a second,
/// *different* original that slugifies to the same name can be told apart
/// from a re-import of the same one.
type SlugMap = BTreeMap<(Kind, String), String>;

/// Resolve `original` to a valid catalog name for `kind` via `slugify`. Two
/// different `original` identifiers landing on the same slug become a
/// collision problem (for the second one) instead of one silently
/// overwriting the other's asset file.
fn slug_or_problem(
    slugs: &mut SlugMap,
    kind: Kind,
    original: &str,
    path: &Path,
    problems: &mut Vec<Problem>,
) -> Option<String> {
    let slug = slugify(original);
    match slugs.get(&(kind, slug.clone())) {
        Some(prev) if prev != original => {
            problems.push(Problem {
                path: path.to_string_lossy().to_string(),
                message: format!("{} {slug}: name collides with {prev}", kind.as_str()),
            });
            None
        }
        _ => {
            slugs.insert((kind, slug.clone()), original.to_string());
            Some(slug)
        }
    }
}

/// Whether `original` should become the asset's `install_as`: `Some(original)`
/// when the catalog slug diverges from the host identifier and that
/// identifier is itself a valid install name; `None` when they match
/// (nothing to record) or `original` can't be used as-is, in which case the
/// caller records a warning instead (see `apply_install_as`).
fn install_as_for(original: &str, slug: &str) -> Option<String> {
    if original != slug && is_valid_install_name(original) {
        Some(original.to_string())
    } else {
        None
    }
}

/// Set `install_as` on `h` for skills, agents and MCP servers when `original`
/// (the host identifier) diverges from the catalog slug, or record a warning
/// when `original` isn't a valid install name — the asset is still created
/// under the slug, but the original stays unmanaged. Hooks and plugin refs
/// never call this: their identity isn't slugified from an arbitrary host
/// identifier, so `install_as` never applies to them.
fn apply_install_as(
    h: &mut Header,
    kind: Kind,
    original: &str,
    slug: &str,
    path: &Path,
    warnings: &mut Vec<Problem>,
) {
    match install_as_for(original, slug) {
        Some(install_as) => h.install_as = Some(install_as),
        None if original != slug => warnings.push(Problem {
            path: path.to_string_lossy().to_string(),
            message: format!(
                "{} {slug}: installs under a new name; {original} stays unmanaged",
                kind.as_str()
            ),
        }),
        None => {}
    }
}

/// Split a Claude tool list (`Read, Grep` or a YAML sequence) into
/// (neutral names, unknown Claude names).
fn split_tools(v: Option<&serde_yaml::Value>) -> (Vec<String>, Vec<String>) {
    let items: Vec<String> = match v {
        Some(serde_yaml::Value::String(s)) => s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
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

fn header(
    kind: Kind,
    name: &str,
    description: String,
    host: &str,
    original: &Path,
    symlink: Option<PathBuf>,
) -> Header {
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
        // Left unset here; `apply_install_as` fills it in afterwards for
        // skills, agents and MCP servers when the host identifier warrants
        // it. Hooks and plugin refs never call `apply_install_as`, so it
        // stays `None` for them.
        install_as: None,
        targets: BTreeMap::new(),
    }
}

fn claude_override(
    extra: BTreeMap<String, Value>,
    model: Option<String>,
) -> BTreeMap<String, TargetOverride> {
    let mut t = BTreeMap::new();
    if !extra.is_empty() || model.is_some() {
        t.insert(
            "claude".to_string(),
            TargetOverride {
                enabled: true,
                model,
                render_as: None,
                extra,
            },
        );
    }
    t
}

const KNOWN_SKILL_KEYS: &[&str] = &["name", "description", "allowed-tools"];
const KNOWN_AGENT_KEYS: &[&str] = &["name", "description", "tools", "model"];

fn import_skill(
    dir: &Path,
    original: &str,
    name: &str,
    host: &str,
    warnings: &mut Vec<Problem>,
) -> Result<Asset, String> {
    let symlink = std::fs::read_link(dir).ok();
    let skill_md = dir.join("SKILL.md");
    let text =
        std::fs::read_to_string(&skill_md).map_err(|e| format!("{}: {e}", skill_md.display()))?;
    let (fm, body) = parse_frontmatter(&text);
    let (allowed_tools, unknown) = split_tools(fm.get("allowed-tools"));
    let mut extra: BTreeMap<String, Value> = fm
        .iter()
        .filter_map(|(k, v)| {
            k.as_str()
                .filter(|k| !KNOWN_SKILL_KEYS.contains(k))
                .map(|k| (k.to_string(), yaml_to_json(v)))
        })
        .collect();
    if !unknown.is_empty() {
        extra.insert("tools".into(), serde_json::json!(unknown));
    }
    let mut resources = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&d)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for p in entries {
            // `symlink_metadata` does not follow the link, so a symlink here
            // (including one that cycles back into an ancestor directory) is
            // detected and skipped rather than walked, mirroring repo.rs's
            // `read_resources`.
            let meta = match std::fs::symlink_metadata(&p) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else if p != skill_md {
                let rel = p
                    .strip_prefix(dir)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                resources.push(Resource {
                    rel_path: format!("resources/{rel}"),
                    bytes: std::fs::read(&p).map_err(|e| e.to_string())?,
                });
            }
        }
    }
    let mut h = header(
        Kind::Skill,
        name,
        str_of(&fm, "description").unwrap_or_default(),
        host,
        dir,
        symlink,
    );
    apply_install_as(&mut h, Kind::Skill, original, name, dir, warnings);
    h.targets = claude_override(extra, None);
    Ok(Asset {
        header: h,
        spec: AssetSpec::Skill {
            allowed_tools,
            user_invocable: true,
            triggers: vec![],
        },
        body,
        resources,
    })
}

fn import_agent(
    file: &Path,
    original: &str,
    name: &str,
    host: &str,
    warnings: &mut Vec<Problem>,
) -> Result<Asset, String> {
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
        .filter_map(|(k, v)| {
            k.as_str()
                .filter(|k| !KNOWN_AGENT_KEYS.contains(k))
                .map(|k| (k.to_string(), yaml_to_json(v)))
        })
        .collect();
    if !unknown.is_empty() {
        extra.insert("tools".into(), serde_json::json!(unknown));
    }
    let mut h = header(
        Kind::Agent,
        name,
        str_of(&fm, "description").unwrap_or_default(),
        host,
        file,
        None,
    );
    apply_install_as(&mut h, Kind::Agent, original, name, file, warnings);
    h.targets = claude_override(extra, explicit);
    Ok(Asset {
        header: h,
        spec: AssetSpec::Agent { tools, model },
        body,
        resources: vec![],
    })
}

/// Replace any occurrence of `fleet_token` in `value` with the placeholder.
fn scrub_token(value: &mut String, fleet_token: Option<&str>) {
    if let Some(tok) = fleet_token {
        if value.contains(tok) {
            *value = value.replace(tok, "${FLEET_MCP_TOKEN}");
        }
    }
}

/// Scrub `fleet_token` from every value in a header/env map.
fn scrub_headers(headers: &mut BTreeMap<String, String>, fleet_token: Option<&str>) {
    for v in headers.values_mut() {
        scrub_token(v, fleet_token);
    }
}

fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["token", "secret", "key", "password", "authorization"]
        .iter()
        .any(|s| k.contains(s))
}

#[allow(clippy::too_many_arguments)]
fn import_hooks(
    settings: &Value,
    host: &str,
    path: &Path,
    fleet_token: Option<&str>,
    taken: &mut Vec<String>,
    flagged: &mut Vec<String>,
    problems: &mut Vec<Problem>,
) -> Vec<Asset> {
    let mut out = Vec::new();
    let Some(hooks) = settings.get("hooks").and_then(Value::as_object) else {
        return out;
    };
    for (claude_event, entries) in hooks {
        let Some(event) = unmap_event(claude_event) else {
            continue;
        };
        for entry in entries.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let matcher = entry
                .get("matcher")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty());
            let hooks_arr = entry.get("hooks").and_then(Value::as_array);
            let count = hooks_arr.map(|a| a.len()).unwrap_or(0);
            // Only the first hook of an entry is imported in v1; extra
            // entries would need distinct asset names. An entry with no
            // hooks at all yields no asset and must not consume a name.
            let Some(h) = hooks_arr.and_then(|a| a.first()) else {
                continue;
            };

            let base = hook_asset_name(claude_event, matcher);
            // `hook_asset_name` already produces a valid catalog name
            // (lowercased event + sanitised matcher joined by `-`); no need
            // to run it through `slugify` a second time.
            debug_assert!(
                is_valid_name(&base),
                "hook_asset_name produced an invalid name: {base:?}"
            );
            let mut name = base.clone();
            let mut n = 2;
            while taken.contains(&name) {
                name = format!("{base}-{n}");
                n += 1;
            }
            taken.push(name.clone());

            if count > 1 {
                problems.push(Problem {
                    path: path.to_string_lossy().to_string(),
                    message: format!(
                        "{claude_event}/{}: only the first of {count} hooks was imported",
                        matcher.unwrap_or("-")
                    ),
                });
            }

            let kind = h
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("command")
                .to_string();
            let mut headers: BTreeMap<String, String> = h
                .get("headers")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            scrub_headers(&mut headers, fleet_token);
            for (k, v) in &headers {
                if looks_secret(k) && !v.contains("${") {
                    flagged.push(format!(
                        "hook {name}: {k} holds a literal secret; replace with a ${{PLACEHOLDER}}"
                    ));
                }
            }
            let mut url = h.get("url").and_then(Value::as_str).map(String::from);
            if let Some(u) = &mut url {
                scrub_token(u, fleet_token);
            }
            let extra: BTreeMap<String, Value> = h
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter(|(k, _)| {
                            !["type", "command", "url", "headers", "timeout"].contains(&k.as_str())
                        })
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let mut hd = header(
                Kind::Hook,
                &name,
                format!(
                    "Imported {claude_event} hook{}",
                    matcher.map(|m| format!(" for {m}")).unwrap_or_default()
                ),
                host,
                path,
                None,
            );
            hd.targets = claude_override(extra, None);
            let tool = matcher.map(|m| {
                unmap_tool(m)
                    .map(String::from)
                    .unwrap_or_else(|| m.to_string())
            });
            out.push(Asset {
                header: hd,
                spec: AssetSpec::Hook {
                    event: event.to_string(),
                    r#match: tool.map(|t| HookMatch { tool: t }),
                    action: HookAction {
                        kind,
                        command: h.get("command").and_then(Value::as_str).map(String::from),
                        url,
                        headers,
                        timeout_s: h.get("timeout").and_then(Value::as_u64),
                    },
                },
                body: String::new(),
                resources: vec![],
            });
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn import_mcp(
    claude_json: &Value,
    host: &str,
    path: &Path,
    fleet_token: Option<&str>,
    flagged: &mut Vec<String>,
    slugs: &mut SlugMap,
    problems: &mut Vec<Problem>,
    warnings: &mut Vec<Problem>,
) -> Vec<Asset> {
    let mut out = Vec::new();
    let Some(servers) = claude_json.get("mcpServers").and_then(Value::as_object) else {
        return out;
    };
    for (name, s) in servers {
        let Some(slug) = slug_or_problem(slugs, Kind::McpServer, name, path, problems) else {
            continue;
        };
        let transport = match s.get("type").and_then(Value::as_str) {
            Some("http") | Some("sse") => "http",
            _ => "stdio",
        };
        let mut headers: BTreeMap<String, String> = s
            .get("headers")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|x| (k.clone(), x.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        scrub_headers(&mut headers, fleet_token);
        let mut env: BTreeMap<String, String> = s
            .get("env")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|x| (k.clone(), x.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        scrub_headers(&mut env, fleet_token);
        let mut url = s.get("url").and_then(Value::as_str).map(String::from);
        if let Some(u) = &mut url {
            scrub_token(u, fleet_token);
        }
        for (k, v) in headers.iter().chain(env.iter()) {
            if looks_secret(k) && !v.contains("${") {
                flagged.push(format!(
                    "mcp_server {name}: {k} holds a literal secret; replace with a ${{PLACEHOLDER}}"
                ));
            }
        }
        let extra: BTreeMap<String, Value> = s
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(k, _)| {
                        !["type", "url", "headers", "command", "args", "env"].contains(&k.as_str())
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut hd = header(
            Kind::McpServer,
            &slug,
            format!("Imported MCP server {name}"),
            host,
            path,
            None,
        );
        apply_install_as(&mut hd, Kind::McpServer, name, &slug, path, warnings);
        hd.targets = claude_override(extra, None);
        out.push(Asset {
            header: hd,
            spec: AssetSpec::McpServer {
                transport: transport.to_string(),
                url,
                headers,
                command: s.get("command").and_then(Value::as_str).map(String::from),
                args: s
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                env,
            },
            body: String::new(),
            resources: vec![],
        });
    }
    out
}

fn import_plugins(
    installed: &Value,
    known: &Value,
    host: &str,
    path: &Path,
    problems: &mut Vec<Problem>,
    slugs: &mut SlugMap,
) -> Vec<Asset> {
    let mut out = Vec::new();
    let Some(plugins) = installed.get("plugins").and_then(Value::as_object) else {
        return out;
    };
    for (key, records) in plugins {
        let Some((plugin, market)) = key.split_once('@') else {
            continue;
        };
        // Scan every record (not just the first) for an installed version;
        // a plugin with none recorded is reported, not defaulted to latest.
        let version = records
            .as_array()
            .and_then(|arr| arr.iter().find_map(|r| r.get("version")?.as_str()));
        let Some(version) = version else {
            problems.push(Problem {
                path: path.to_string_lossy().to_string(),
                message: format!("{plugin}@{market}: no installed version recorded"),
            });
            continue;
        };
        let version = version.to_string();
        // The catalog asset name (identity/filename) is slugified; the
        // `plugin` field inside the spec below stays the real plugin id so
        // `render_plugin`'s merge target (`plugins.<plugin>@<marketplace>`)
        // still matches what is actually installed on the host.
        let Some(slug) = slug_or_problem(slugs, Kind::PluginRef, plugin, path, problems) else {
            continue;
        };
        let src = known.get(market).and_then(|m| m.get("source"));
        let marketplace = Marketplace {
            name: market.to_string(),
            source: src
                .and_then(|s| s.get("source"))
                .and_then(Value::as_str)
                .unwrap_or("github")
                .to_string(),
            repo: src
                .and_then(|s| s.get("repo"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        out.push(Asset {
            header: header(
                Kind::PluginRef,
                &slug,
                format!("Imported plugin {plugin} from {market}"),
                host,
                path,
                None,
            ),
            spec: AssetSpec::PluginRef {
                harness: "claude".into(),
                marketplace,
                plugin: plugin.to_string(),
                version,
            },
            body: String::new(),
            resources: vec![],
        });
    }
    out
}

fn read_json(p: &Path) -> Value {
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
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
    let mut report = ImportReport {
        created: vec![],
        problems: vec![],
        warnings: vec![],
        flagged_secrets: vec![],
        dry_run,
    };
    let mut assets: Vec<Asset> = Vec::new();
    // Shared across every kind whose identity comes from an arbitrary
    // on-disk/JSON name (skills, agents, MCP servers, plugins) so a
    // post-slug collision between two different originals is caught no
    // matter which pass introduced each half of the pair.
    let mut slugs: SlugMap = BTreeMap::new();

    let skills_dir = src.claude_dir.join("skills");
    if skills_dir.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&skills_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for p in entries {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if !p.is_dir() {
                if std::fs::symlink_metadata(&p)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    report.problems.push(Problem {
                        path: p.to_string_lossy().to_string(),
                        message: "broken symlink".into(),
                    });
                }
                continue;
            }
            if !p.join("SKILL.md").is_file() {
                continue;
            }
            let Some(slug) =
                slug_or_problem(&mut slugs, Kind::Skill, &name, &p, &mut report.problems)
            else {
                continue;
            };
            match import_skill(&p, &name, &slug, host, &mut report.warnings) {
                Ok(a) => assets.push(a),
                Err(message) => report.problems.push(Problem {
                    path: p.to_string_lossy().to_string(),
                    message,
                }),
            }
        }
    }
    let agents_dir = src.claude_dir.join("agents");
    if agents_dir.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&agents_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for p in entries {
            if p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let name = p
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let Some(slug) =
                slug_or_problem(&mut slugs, Kind::Agent, &name, &p, &mut report.problems)
            else {
                continue;
            };
            match import_agent(&p, &name, &slug, host, &mut report.warnings) {
                Ok(a) => assets.push(a),
                Err(message) => report.problems.push(Problem {
                    path: p.to_string_lossy().to_string(),
                    message,
                }),
            }
        }
    }
    let settings_path = src.claude_dir.join("settings.json");
    let mut taken = Vec::new();
    assets.extend(import_hooks(
        &read_json(&settings_path),
        host,
        &settings_path,
        fleet_token,
        &mut taken,
        &mut report.flagged_secrets,
        &mut report.problems,
    ));
    assets.extend(import_mcp(
        &read_json(&src.claude_json),
        host,
        &src.claude_json,
        fleet_token,
        &mut report.flagged_secrets,
        &mut slugs,
        &mut report.problems,
        &mut report.warnings,
    ));
    let installed_path = src.claude_dir.join("plugins/installed_plugins.json");
    let known_path = src.claude_dir.join("plugins/known_marketplaces.json");
    assets.extend(import_plugins(
        &read_json(&installed_path),
        &read_json(&known_path),
        host,
        &installed_path,
        &mut report.problems,
        &mut slugs,
    ));

    for a in assets {
        let kind = a.kind();
        let problems = a.validate();
        if !problems.is_empty() {
            report.problems.push(Problem {
                path: format!("{}/{}", kind.dir(), a.header.name),
                message: problems.join("; "),
            });
            continue;
        }
        if asset_path(repo_root, kind, &a.header.name).exists() {
            report.problems.push(Problem {
                path: format!("{}/{}", kind.dir(), a.header.name),
                message: format!(
                    "{} {} already exists in the catalog",
                    kind.as_str(),
                    a.header.name
                ),
            });
            continue;
        }
        if !dry_run {
            if let Err(e) = write_asset(repo_root, &a, false) {
                // A per-asset write fault (name already taken, or — e.g. a
                // resource filename with a space or non-ASCII character —
                // an invalid resource path) is this asset's problem, not a
                // reason to abort the rest of the run; anything else (I/O,
                // serialization, ...) is unexpected and still propagates.
                if e.code == E_ASSET_EXISTS || e.code == E_INVALID {
                    report.problems.push(Problem {
                        path: format!("{}/{}", kind.dir(), a.header.name),
                        message: e.message,
                    });
                } else {
                    return Err(e);
                }
                continue;
            }
        }
        report
            .created
            .push((kind.as_str().to_string(), a.header.name.clone()));
    }
    Ok(report)
}

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
        w(
            "linked/skill-x/SKILL.md",
            "---\nname: skill-x\ndescription: Linked.\n---\nlinked body\n",
        );
        std::os::unix::fs::symlink(
            base.join("linked/skill-x"),
            claude_dir.join("skills/skill-x"),
        )
        .unwrap();
        std::os::unix::fs::symlink(base.join("nowhere"), claude_dir.join("skills/broken")).unwrap();
        w("home/.claude/agents/pm-qa.md", "---\nname: pm-qa\ndescription: QA lens.\ntools: Read, Grep, Weird\nmodel: opus\ncolor: blue\n---\nYou are QA.\n");
        w(
            "home/.claude/agents/odd.md",
            "---\nname: odd\ndescription: Odd model.\nmodel: claude-haiku-4-5-20251001\n---\np\n",
        );
        w(
            "home/.claude/settings.json",
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"exec rtk hook claude"}]}],"Stop":[{"hooks":[{"type":"command","command":"node stop.mjs","timeout":20}]},{"matcher":"","hooks":[{"type":"http","url":"http://127.0.0.1:4180/hook","timeout":5,"headers":{"Authorization":"Bearer SECRET123"}}]}]}}"#,
        );
        w(
            "home/.claude.json",
            r#"{"mcpServers":{"claude-fleet":{"type":"http","url":"http://127.0.0.1:4180/mcp","headers":{"Authorization":"Bearer SECRET123"}},"jira":{"type":"stdio","command":"npx","args":["-y","jira"],"env":{"JIRA_TOKEN":"abc"}}},"other":1}"#,
        );
        w(
            "home/.claude/plugins/installed_plugins.json",
            r#"{"version":2,"plugins":{"superpowers@superpowers-marketplace":[{"scope":"user","version":"6.3.0"}]}}"#,
        );
        w(
            "home/.claude/plugins/known_marketplaces.json",
            r#"{"superpowers-marketplace":{"source":{"source":"github","repo":"obra/superpowers-marketplace"}}}"#,
        );
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        (
            ImportSources {
                claude_dir,
                claude_json: base.join("home/.claude.json"),
            },
            repo,
        )
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
        assert!(rep
            .problems
            .iter()
            .any(|p| p.path.ends_with("skills/broken")));
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
        match &skill.spec {
            AssetSpec::Skill { allowed_tools, .. } => {
                assert_eq!(allowed_tools, &vec!["bash".to_string(), "read".to_string()])
            }
            _ => panic!(),
        }
        assert_eq!(
            skill.header.targets["claude"].extra["disable-model-invocation"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(skill.resources[0].rel_path, "resources/scripts/go.sh");
        assert_eq!(
            skill
                .header
                .source
                .as_ref()
                .unwrap()
                .imported_from
                .as_deref(),
            Some("local")
        );

        let linked = cat.find(Kind::Skill, "skill-x").unwrap();
        assert_eq!(linked.body, "linked body\n");
        assert!(linked
            .header
            .source
            .as_ref()
            .unwrap()
            .symlink_target
            .as_deref()
            .unwrap()
            .ends_with("linked/skill-x"));

        let agent = cat.find(Kind::Agent, "pm-qa").unwrap();
        match &agent.spec {
            AssetSpec::Agent { tools, model } => {
                assert_eq!(tools, &vec!["read".to_string(), "grep".to_string()]);
                assert_eq!(model, "strong");
            }
            _ => panic!(),
        }
        assert_eq!(
            agent.header.targets["claude"].extra["tools"],
            serde_json::json!(["Weird"])
        );
        assert_eq!(
            agent.header.targets["claude"].extra["color"],
            serde_json::json!("blue")
        );
        let odd = cat.find(Kind::Agent, "odd").unwrap();
        match &odd.spec {
            AssetSpec::Agent { model, .. } => assert_eq!(model, "default"),
            _ => panic!(),
        }
        assert_eq!(
            odd.header.targets["claude"].model.as_deref(),
            Some("claude-haiku-4-5-20251001")
        );

        let rtk = cat.find(Kind::Hook, "before-tool-bash").unwrap();
        match &rtk.spec {
            AssetSpec::Hook {
                event,
                r#match,
                action,
            } => {
                assert_eq!(event, "before_tool");
                assert_eq!(r#match.as_ref().unwrap().tool, "bash");
                assert_eq!(action.command.as_deref(), Some("exec rtk hook claude"));
            }
            _ => panic!(),
        }
        assert!(cat.find(Kind::Hook, "stop").is_some());
        let stop2 = cat.find(Kind::Hook, "stop-2").unwrap();
        match &stop2.spec {
            AssetSpec::Hook { action, .. } => {
                assert_eq!(action.headers["Authorization"], "Bearer ${FLEET_MCP_TOKEN}");
                assert_eq!(action.timeout_s, Some(5));
            }
            _ => panic!(),
        }

        let fleet = cat.find(Kind::McpServer, "claude-fleet").unwrap();
        match &fleet.spec {
            AssetSpec::McpServer { headers, .. } => {
                assert_eq!(headers["Authorization"], "Bearer ${FLEET_MCP_TOKEN}")
            }
            _ => panic!(),
        }
        let jira = cat.find(Kind::McpServer, "jira").unwrap();
        match &jira.spec {
            AssetSpec::McpServer {
                transport,
                args,
                env,
                ..
            } => {
                assert_eq!(transport, "stdio");
                assert_eq!(args, &vec!["-y".to_string(), "jira".to_string()]);
                assert_eq!(env["JIRA_TOKEN"], "abc");
            }
            _ => panic!(),
        }
        assert!(
            rep.flagged_secrets
                .iter()
                .any(|s| s.contains("jira") && s.contains("JIRA_TOKEN")),
            "{:?}",
            rep.flagged_secrets
        );

        let plugin = cat.find(Kind::PluginRef, "superpowers").unwrap();
        match &plugin.spec {
            AssetSpec::PluginRef {
                marketplace,
                version,
                ..
            } => {
                assert_eq!(marketplace.repo, "obra/superpowers-marketplace");
                assert_eq!(version, "6.3.0");
            }
            _ => panic!(),
        }

        // Re-import collides on everything and creates nothing new. The
        // broken symlink is a standing fault independent of catalog state,
        // so it is reported again too on the second scan; check the
        // collision count precisely instead of requiring every problem to
        // be a collision.
        let again = import_claude(&src, &repo, "local", Some("SECRET123"), false).unwrap();
        assert!(again.created.is_empty());
        assert_eq!(
            again
                .problems
                .iter()
                .filter(|p| p.message.contains("already exists"))
                .count(),
            rep.created.len()
        );
    }

    #[test]
    fn import_then_render_reproduces_skill_and_agent() {
        let (src, repo) = fixture("rt");
        import_claude(&src, &repo, "local", None, false).unwrap();
        let cat = load_dir(&repo).unwrap();
        let claude = crate::service::catalog::harness::claude::Claude;
        use crate::service::catalog::harness::Harness;
        let plan = claude
            .render(cat.find(Kind::Skill, "worktree").unwrap())
            .unwrap();
        let rendered = String::from_utf8(
            plan.files
                .iter()
                .find(|f| f.path.ends_with("SKILL.md"))
                .unwrap()
                .bytes
                .clone(),
        )
        .unwrap();
        assert_eq!(
            rendered,
            fs::read_to_string(src.claude_dir.join("skills/worktree/SKILL.md")).unwrap()
        );
        let plan = claude
            .render(cat.find(Kind::Agent, "pm-qa").unwrap())
            .unwrap();
        let rendered = String::from_utf8(plan.files[0].bytes.clone()).unwrap();
        // Field order is canonical (name, description, tools, model, extras); content is preserved.
        assert!(rendered.contains("tools: Read, Grep, Weird"), "{rendered}");
        assert!(rendered.contains("model: opus"));
        assert!(rendered.contains("color: blue"));
        assert!(rendered.ends_with("---\nYou are QA.\n"));
    }

    #[test]
    fn hook_header_secret_is_flagged() {
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "http",
                        "url": "http://h/hook",
                        "headers": { "Authorization": "Bearer OTHER" }
                    }]
                }]
            }
        });
        let mut taken = Vec::new();
        let mut flagged = Vec::new();
        let mut problems = Vec::new();
        let assets = import_hooks(
            &settings,
            "local",
            Path::new("settings.json"),
            Some("SECRET123"),
            &mut taken,
            &mut flagged,
            &mut problems,
        );
        assert_eq!(assets.len(), 1);
        assert!(problems.is_empty());
        assert!(
            flagged
                .iter()
                .any(|s| s.contains("hook") && s.contains("Authorization")),
            "{flagged:?}"
        );
    }

    #[test]
    fn fleet_token_is_scrubbed_from_env_and_url() {
        let claude_json = serde_json::json!({
            "mcpServers": {
                "svc": { "type": "stdio", "command": "x", "env": { "FLEET_HDR": "x SECRET123 y" } }
            }
        });
        let mut flagged = Vec::new();
        let mut slugs = SlugMap::new();
        let mut problems = Vec::new();
        let mut warnings = Vec::new();
        let assets = import_mcp(
            &claude_json,
            "local",
            Path::new(".claude.json"),
            Some("SECRET123"),
            &mut flagged,
            &mut slugs,
            &mut problems,
            &mut warnings,
        );
        let AssetSpec::McpServer { env, .. } = &assets[0].spec else {
            panic!()
        };
        assert_eq!(env["FLEET_HDR"], "x ${FLEET_MCP_TOKEN} y");

        let settings = serde_json::json!({
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "http", "url": "http://h/?t=SECRET123" }] }]
            }
        });
        let mut taken = Vec::new();
        let mut flagged2 = Vec::new();
        let mut problems = Vec::new();
        let assets = import_hooks(
            &settings,
            "local",
            Path::new("settings.json"),
            Some("SECRET123"),
            &mut taken,
            &mut flagged2,
            &mut problems,
        );
        let AssetSpec::Hook { action, .. } = &assets[0].spec else {
            panic!()
        };
        assert_eq!(
            action.url.as_deref(),
            Some("http://h/?t=${FLEET_MCP_TOKEN}")
        );
    }

    #[test]
    fn multi_hook_entry_imports_first_and_reports_problem() {
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [{
                    "hooks": [
                        { "type": "command", "command": "a" },
                        { "type": "command", "command": "b" }
                    ]
                }]
            }
        });
        let mut taken = Vec::new();
        let mut flagged = Vec::new();
        let mut problems = Vec::new();
        let assets = import_hooks(
            &settings,
            "local",
            Path::new("settings.json"),
            None,
            &mut taken,
            &mut flagged,
            &mut problems,
        );
        assert_eq!(assets.len(), 1);
        match &assets[0].spec {
            AssetSpec::Hook { action, .. } => assert_eq!(action.command.as_deref(), Some("a")),
            _ => panic!(),
        }
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0]
                .message
                .contains("only the first of 2 hooks was imported"),
            "{:?}",
            problems[0]
        );
    }

    #[test]
    fn hooks_entry_with_no_hooks_does_not_consume_a_name() {
        let settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [] },
                    { "hooks": [{ "type": "command", "command": "real" }] }
                ]
            }
        });
        let mut taken = Vec::new();
        let mut flagged = Vec::new();
        let mut problems = Vec::new();
        let assets = import_hooks(
            &settings,
            "local",
            Path::new("settings.json"),
            None,
            &mut taken,
            &mut flagged,
            &mut problems,
        );
        assert_eq!(assets.len(), 1);
        // The empty entry did not reserve "stop"; the real one got it.
        assert_eq!(assets[0].header.name, "stop");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn plugin_with_no_recorded_version_is_reported_and_skipped() {
        let installed = serde_json::json!({ "plugins": { "foo@bar": [{"scope": "user"}] } });
        let known = serde_json::json!({});
        let mut problems = Vec::new();
        let mut slugs = SlugMap::new();
        let assets = import_plugins(
            &installed,
            &known,
            "local",
            Path::new("installed_plugins.json"),
            &mut problems,
            &mut slugs,
        );
        assert!(assets.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("foo@bar"));
        assert!(problems[0]
            .message
            .contains("no installed version recorded"));
    }

    #[test]
    fn plugin_version_found_on_a_later_record_is_used() {
        let installed = serde_json::json!({
            "plugins": { "foo@bar": [{"scope": "user"}, {"scope": "project", "version": "2.0.0"}] }
        });
        let known = serde_json::json!({});
        let mut problems = Vec::new();
        let mut slugs = SlugMap::new();
        let assets = import_plugins(
            &installed,
            &known,
            "local",
            Path::new("installed_plugins.json"),
            &mut problems,
            &mut slugs,
        );
        assert!(problems.is_empty(), "{problems:?}");
        match &assets[0].spec {
            AssetSpec::PluginRef { version, .. } => assert_eq!(version, "2.0.0"),
            _ => panic!(),
        }
    }

    #[test]
    fn import_skill_skips_symlinked_resources_and_terminates() {
        let base = std::env::temp_dir().join(format!("fleet-import-symres-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let skill_dir = base.join("skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\n---\nb\n",
        )
        .unwrap();
        fs::write(skill_dir.join("real.txt"), "hi\n").unwrap();
        // A symlink back up the tree: following it as a directory would
        // recurse forever. It must be skipped entirely, not walked.
        std::os::unix::fs::symlink("..", skill_dir.join("loop")).unwrap();

        let asset = import_skill(&skill_dir, "s", "s", "local", &mut Vec::new()).unwrap();
        assert_eq!(asset.resources.len(), 1);
        assert_eq!(asset.resources[0].rel_path, "resources/real.txt");
    }

    #[test]
    fn slugify_examples() {
        assert_eq!(
            slugify("plugin:episodic-memory:episodic-memory"),
            "plugin-episodic-memory-episodic-memory"
        );
        assert_eq!(slugify("Foo_Bar"), "foo-bar");
        assert_eq!(slugify("--x--"), "x");
        assert_eq!(slugify(""), "x");
        // Already-kebab names are left alone.
        assert_eq!(slugify("worktree"), "worktree");
        assert_eq!(
            slugify("plugin_superpowers-chrome_chrome"),
            "plugin-superpowers-chrome-chrome"
        );
    }

    #[test]
    fn import_mcp_slugifies_non_kebab_server_keys() {
        let claude_json = serde_json::json!({
            "mcpServers": {
                "plugin_superpowers-chrome_chrome": { "type": "stdio", "command": "x" }
            }
        });
        let mut flagged = Vec::new();
        let mut slugs = SlugMap::new();
        let mut problems = Vec::new();
        let mut warnings = Vec::new();
        let assets = import_mcp(
            &claude_json,
            "local",
            Path::new(".claude.json"),
            None,
            &mut flagged,
            &mut slugs,
            &mut problems,
            &mut warnings,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].header.name, "plugin-superpowers-chrome-chrome");
    }

    /// A resource whose on-disk filename `write_asset` rejects (a space, per
    /// `valid_resource_rel_path`) must become a per-asset problem, not abort
    /// the whole import — the other assets in the same run still land.
    #[test]
    fn write_asset_invalid_resource_name_becomes_a_problem_not_an_abort() {
        let base = std::env::temp_dir().join(format!("fleet-import-badres-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let claude_dir = base.join("home/.claude");
        let w = |rel: &str, c: &str| {
            let p = base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        };
        w(
            "home/.claude/skills/good/SKILL.md",
            "---\nname: good\ndescription: A perfectly ordinary skill.\n---\n# Body\n",
        );
        w(
            "home/.claude/skills/bad/SKILL.md",
            "---\nname: bad\ndescription: Has a resource filename write_asset rejects.\n---\n# Body\n",
        );
        w("home/.claude/skills/bad/notes (draft).txt", "notes\n");
        w(
            "home/.claude/agents/pm-qa.md",
            "---\nname: pm-qa\ndescription: An ordinary agent.\ntools: Read\n---\nYou are QA.\n",
        );
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        let src = ImportSources {
            claude_dir,
            claude_json: base.join("home/.claude.json"),
        };

        let rep = import_claude(&src, &repo, "local", None, false).unwrap();

        // The offending asset is reported, not created.
        assert!(
            !rep.created.iter().any(|(_, n)| n == "bad"),
            "{:?}",
            rep.created
        );
        let problem = rep
            .problems
            .iter()
            .find(|p| p.path == "skills/bad")
            .unwrap_or_else(|| panic!("bad skill reported as a problem: {:?}", rep.problems));
        assert!(
            problem.message.contains("invalid resource path"),
            "{:?}",
            problem
        );
        // The rest of the run completed: the other assets were created.
        assert!(rep.created.contains(&("skill".into(), "good".into())));
        assert!(rep.created.contains(&("agent".into(), "pm-qa".into())));
        let cat = load_dir(&repo).unwrap();
        assert!(cat.find(Kind::Skill, "good").is_some());
        assert!(cat.find(Kind::Agent, "pm-qa").is_some());
        assert!(cat.find(Kind::Skill, "bad").is_none());
    }

    #[test]
    fn install_as_for_examples() {
        assert_eq!(
            install_as_for("foo_bar", "foo-bar"),
            Some("foo_bar".to_string())
        );
        // Original equals the slug: nothing to record.
        assert_eq!(install_as_for("worktree", "worktree"), None);
        // Original is not itself a valid install name (a space): no
        // `install_as`; the caller records a warning instead.
        assert_eq!(install_as_for("PM Review", "pm-review"), None);
    }

    #[test]
    fn non_kebab_skill_dir_gets_install_as() {
        let base = std::env::temp_dir().join(format!(
            "fleet-import-install-as-skill-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let claude_dir = base.join("home/.claude");
        let w = |rel: &str, c: &str| {
            let p = base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        };
        w(
            "home/.claude/skills/foo_bar/SKILL.md",
            "---\nname: foo_bar\ndescription: d\n---\nb\n",
        );
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        let src = ImportSources {
            claude_dir,
            claude_json: base.join("home/.claude.json"),
        };

        let rep = import_claude(&src, &repo, "local", None, false).unwrap();
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        let cat = load_dir(&repo).unwrap();
        let skill = cat.find(Kind::Skill, "foo-bar").unwrap();
        assert_eq!(skill.header.install_as.as_deref(), Some("foo_bar"));
    }

    #[test]
    fn non_slug_safe_agent_name_warns_instead_of_install_as() {
        let base = std::env::temp_dir().join(format!(
            "fleet-import-install-as-agent-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let claude_dir = base.join("home/.claude");
        let w = |rel: &str, c: &str| {
            let p = base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        };
        w(
            "home/.claude/agents/PM Review.md",
            "---\nname: PM Review\ndescription: d\n---\nb\n",
        );
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        let src = ImportSources {
            claude_dir,
            claude_json: base.join("home/.claude.json"),
        };

        let rep = import_claude(&src, &repo, "local", None, false).unwrap();
        let cat = load_dir(&repo).unwrap();
        let agent = cat.find(Kind::Agent, "pm-review").unwrap();
        assert!(agent.header.install_as.is_none());
        assert!(rep.problems.is_empty(), "{:?}", rep.problems);
        assert_eq!(rep.warnings.len(), 1, "{:?}", rep.warnings);
        assert_eq!(
            rep.warnings[0].message,
            "agent pm-review: installs under a new name; PM Review stays unmanaged"
        );
    }

    #[test]
    fn mcp_server_key_gets_install_as() {
        let claude_json = serde_json::json!({
            "mcpServers": {
                "claude_ai_Docs": { "type": "stdio", "command": "x" }
            }
        });
        let mut flagged = Vec::new();
        let mut slugs = SlugMap::new();
        let mut problems = Vec::new();
        let mut warnings = Vec::new();
        let assets = import_mcp(
            &claude_json,
            "local",
            Path::new(".claude.json"),
            None,
            &mut flagged,
            &mut slugs,
            &mut problems,
            &mut warnings,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].header.name, "claude-ai-docs");
        assert_eq!(
            assets[0].header.install_as.as_deref(),
            Some("claude_ai_Docs")
        );
    }

    #[test]
    fn kebab_originals_get_no_install_as_and_no_warning() {
        let (src, repo) = fixture("kebab-install-as");
        let rep = import_claude(&src, &repo, "local", Some("SECRET123"), false).unwrap();
        let cat = load_dir(&repo).unwrap();
        let skill = cat.find(Kind::Skill, "worktree").unwrap();
        assert!(skill.header.install_as.is_none());
        let agent = cat.find(Kind::Agent, "pm-qa").unwrap();
        assert!(agent.header.install_as.is_none());
        let fleet = cat.find(Kind::McpServer, "claude-fleet").unwrap();
        assert!(fleet.header.install_as.is_none());
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    }

    #[test]
    fn hooks_and_plugin_refs_never_get_install_as() {
        let (src, repo) = fixture("hooks-plugins-install-as");
        import_claude(&src, &repo, "local", Some("SECRET123"), false).unwrap();
        let cat = load_dir(&repo).unwrap();
        let hook = cat.find(Kind::Hook, "before-tool-bash").unwrap();
        assert!(hook.header.install_as.is_none());
        let plugin = cat.find(Kind::PluginRef, "superpowers").unwrap();
        assert!(plugin.header.install_as.is_none());
    }

    #[test]
    fn slug_collision_between_two_skill_folders_is_reported() {
        let base =
            std::env::temp_dir().join(format!("fleet-import-collision-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let claude_dir = base.join("home/.claude");
        let w = |rel: &str, c: &str| {
            let p = base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        };
        // `Foo_Bar` and `foo-bar` both slugify to `foo-bar`: exactly one of
        // them becomes an asset, the other a collision problem — never a
        // silent overwrite of one by the other.
        w(
            "home/.claude/skills/Foo_Bar/SKILL.md",
            "---\nname: Foo_Bar\ndescription: d\n---\nb\n",
        );
        w(
            "home/.claude/skills/foo-bar/SKILL.md",
            "---\nname: foo-bar\ndescription: d\n---\nb\n",
        );
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("catalog.yaml"), "schema_version: 1\n").unwrap();
        let src = ImportSources {
            claude_dir,
            claude_json: base.join("home/.claude.json"),
        };

        let rep = import_claude(&src, &repo, "local", None, false).unwrap();

        assert_eq!(
            rep.created
                .iter()
                .filter(|(kind, name)| kind == "skill" && name == "foo-bar")
                .count(),
            1,
            "{:?}",
            rep.created
        );
        assert!(
            rep.problems
                .iter()
                .any(|p| p.message.contains("collides with")),
            "{:?}",
            rep.problems
        );
    }
}
