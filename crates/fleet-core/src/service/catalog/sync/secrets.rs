//! Secret resolution and `${NAME}` substitution into a rendered plan.
//!
//! Precedence is host override > global > built-ins (`resolve`). Values are
//! never logged and never included in an error message — only names are.
//! A rendered `.toml` file (a Codex subagent) is substituted as TOML, value
//! by value, so a secret can never break or rewrite the document.
//!
//! `inventory::compute_states` substitutes before it compares (a host
//! holding the real value must not read as drift), and `sync::plan_sync` /
//! `sync::apply_sync` resolve once per host before every await — `resolve`
//! takes the store lock internally.

use super::super::harness::{ConfigMerge, FileWrite, RenderPlan};
use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::mcp::SETTING_PORT;
use crate::service::catalog::model::find_placeholders;
use crate::store::Store;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// Built-in secret name resolving to this host's control-API bearer token
/// (absent when the host has not been provisioned — a reference to it is
/// then reported as missing rather than filled in).
pub const BUILTIN_TOKEN: &str = "FLEET_MCP_TOKEN";
/// Built-in secret name resolving to the control API's port (the
/// `mcp.port` setting, or `"4180"` if unset).
pub const BUILTIN_PORT: &str = "FLEET_MCP_PORT";

const DEFAULT_PORT: &str = "4180";

/// Whether `name` is a valid `${NAME}` secret name: non-empty, uppercase
/// letters/digits/underscore only — the same shape as an environment
/// variable, so it never needs escaping wherever it lands. Shared by the
/// Tauri command and the MCP tool so the rule cannot drift between them.
pub fn is_valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Resolve every secret value visible to `host_alias`: host override >
/// global > built-ins. Never logs a value.
pub fn resolve(
    store: &Mutex<Store>,
    host_alias: &str,
) -> Result<BTreeMap<String, String>, IpcError> {
    let s = lock(store)?;
    let mut values = BTreeMap::new();

    let port = s
        .get_setting(SETTING_PORT)?
        .unwrap_or_else(|| DEFAULT_PORT.to_string());
    values.insert(BUILTIN_PORT.to_string(), port);
    if let Some(row) = s.get_host_token(host_alias)? {
        values.insert(BUILTIN_TOKEN.to_string(), row.token);
    }

    for (name, value) in s.secret_values_for_host(host_alias)? {
        values.insert(name, value);
    }
    Ok(values)
}

/// A post-substitution `RenderPlan`: its files' bytes and merges' values now
/// contain real secret text, so unlike `RenderPlan` this type's `Debug`
/// impl is hand-written to print only file paths, `file:json/path` strings
/// for merges, and counts — never bytes or values. It also deliberately
/// does not derive `Serialize`, so it can't be accidentally sent to the
/// frontend or logged as JSON either. Access the real plan via `inner`/
/// `into_inner` only where the caller actually needs to write it to disk.
#[derive(Clone, Default, PartialEq)]
pub struct SecretPlan(RenderPlan);

impl SecretPlan {
    pub fn inner(&self) -> &RenderPlan {
        &self.0
    }
}

impl From<RenderPlan> for SecretPlan {
    fn from(plan: RenderPlan) -> Self {
        SecretPlan(plan)
    }
}

impl std::fmt::Debug for SecretPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let file_paths: Vec<&str> = self.0.files.iter().map(|fw| fw.path.as_str()).collect();
        let merge_paths: Vec<String> = self
            .0
            .merges
            .iter()
            .map(|m| format!("{}:{}", m.file, m.json_path.join("/")))
            .collect();
        f.debug_struct("SecretPlan")
            .field("files", &file_paths)
            .field("merges", &merge_paths)
            .field("placeholders_count", &self.0.placeholders.len())
            .field("warnings_count", &self.0.warnings.len())
            .finish()
    }
}

/// A `RenderPlan` with every known `${NAME}` placeholder substituted.
/// `Debug` is hand-written (delegating to `SecretPlan`'s redacted impl) so
/// that formatting a `Substituted` can never print a secret value.
#[derive(Clone, Default, PartialEq)]
pub struct Substituted {
    pub plan: SecretPlan,
    /// Names referenced by the plan with no known value; left verbatim as
    /// `${NAME}` in the output.
    pub missing: Vec<String>,
    /// Paths of files whose body actually changed as a result of
    /// substitution.
    pub secret_files: BTreeSet<String>,
    /// Config files whose merge value actually changed as a result of
    /// substitution.
    pub secret_merge_files: BTreeSet<String>,
    /// `${NAME}`s found in a TOML **key**, which is never substituted
    /// (`<file>: <NAME>`, once each, in first-seen order). The plan is
    /// blocked on these: a warning nothing reads would otherwise let the
    /// file install with the placeholder still in it.
    pub key_placeholders: Vec<String>,
}

impl std::fmt::Debug for Substituted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Substituted")
            .field("plan", &self.plan)
            .field("missing", &self.missing)
            .field("secret_files", &self.secret_files)
            .field("secret_merge_files", &self.secret_merge_files)
            .field("key_placeholders", &self.key_placeholders)
            .finish()
    }
}

/// Replace every `${NAME}` this function recognises (see
/// `model::find_placeholders`'s naming rule: `[A-Z0-9_]+`) in `text` with
/// its value from `values`. Anything else that merely looks like `${...}`
/// (lower-case, empty, unterminated) is left untouched, exactly as
/// `find_placeholders` would ignore it. Unknown names are left as `${NAME}`
/// and recorded in `missing` (once each, in first-seen order).
fn substitute_str(
    text: &str,
    values: &BTreeMap<String, String>,
    missing: &mut Vec<String>,
) -> (String, bool) {
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                let is_placeholder_name = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                if is_placeholder_name {
                    match values.get(name) {
                        Some(value) => {
                            out.push_str(value);
                            changed = true;
                        }
                        None => {
                            out.push_str("${");
                            out.push_str(name);
                            out.push('}');
                            if !missing.iter().any(|m| m == name) {
                                missing.push(name.to_string());
                            }
                        }
                    }
                } else {
                    out.push_str("${");
                    out.push_str(name);
                    out.push('}');
                }
                rest = &after[end + 1..];
            }
            None => {
                // Unterminated `${` — matches `find_placeholders`, which
                // stops scanning rather than matching a partial token.
                out.push_str("${");
                rest = after;
                break;
            }
        }
    }
    out.push_str(rest);
    (out, changed)
}

/// Recurse into every string leaf of a merge `value` (objects and arrays
/// included) and substitute placeholders in each.
fn substitute_value(
    value: &Value,
    values: &BTreeMap<String, String>,
    missing: &mut Vec<String>,
) -> (Value, bool) {
    match value {
        Value::String(s) => {
            let (out, changed) = substitute_str(s, values, missing);
            (Value::String(out), changed)
        }
        Value::Array(items) => {
            let mut changed = false;
            let out = items
                .iter()
                .map(|item| {
                    let (v, c) = substitute_value(item, values, missing);
                    changed |= c;
                    v
                })
                .collect();
            (Value::Array(out), changed)
        }
        Value::Object(map) => {
            let mut changed = false;
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                let (nv, c) = substitute_value(v, values, missing);
                changed |= c;
                out.insert(k.clone(), nv);
            }
            (Value::Object(out), changed)
        }
        other => (other.clone(), false),
    }
}

/// Recurse into every string value of a TOML value (tables and arrays
/// included) and substitute placeholders in each. Keys are left alone.
fn substitute_toml_value(
    value: &toml::Value,
    values: &BTreeMap<String, String>,
    missing: &mut Vec<String>,
) -> (toml::Value, bool) {
    match value {
        toml::Value::String(s) => {
            let (out, changed) = substitute_str(s, values, missing);
            (toml::Value::String(out), changed)
        }
        toml::Value::Array(items) => {
            let mut changed = false;
            let out = items
                .iter()
                .map(|item| {
                    let (v, c) = substitute_toml_value(item, values, missing);
                    changed |= c;
                    v
                })
                .collect();
            (toml::Value::Array(out), changed)
        }
        toml::Value::Table(table) => {
            let mut changed = false;
            let mut out = toml::Table::new();
            for (k, v) in table {
                let (nv, c) = substitute_toml_value(v, values, missing);
                changed |= c;
                out.insert(k.clone(), nv);
            }
            (toml::Value::Table(out), changed)
        }
        other => (other.clone(), false),
    }
}

/// Every `${NAME}` a key anywhere in `value` holds, once each. Keys are never
/// substituted, so each of these is a placeholder that would reach the host
/// verbatim.
///
/// In the order the PARSED document holds them, which a `toml::Table` sorts by
/// key — not the order they appear in the file.
fn toml_key_placeholders(value: &toml::Value, out: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for (k, v) in table {
                for name in find_placeholders(k) {
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
                toml_key_placeholders(v, out);
            }
        }
        toml::Value::Array(items) => {
            for v in items {
                toml_key_placeholders(v, out);
            }
        }
        _ => {}
    }
}

/// `${NAME}` substitution for a rendered TOML file (a Codex subagent under
/// `~/.codex/agents/`): the text is parsed, placeholders are replaced inside
/// string *values* only, and the document is re-serialized by the toml
/// crate, so a secret holding `"`, `\`, `'` or a newline stays one intact
/// string instead of breaking — or changing — the document the way a raw
/// text replace would. The text is returned untouched when nothing was
/// substituted. `None` when `text` is not a TOML document or the result
/// cannot be serialized; the caller then falls back to raw replacement.
/// `missing` is only extended on success. The third element is every
/// `${NAME}` a KEY held (keys are never substituted), which the caller
/// blocks on.
fn substitute_toml(
    text: &str,
    values: &BTreeMap<String, String>,
    missing: &mut Vec<String>,
) -> Option<(String, bool, Vec<String>)> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let doc = toml::Value::Table(table);
    let mut local_missing = Vec::new();
    let (out, changed) = substitute_toml_value(&doc, values, &mut local_missing);
    let text_out = if changed {
        let toml::Value::Table(t) = out else {
            return None;
        };
        toml::to_string_pretty(&t).ok()?
    } else {
        text.to_string()
    };
    for name in local_missing {
        if !missing.contains(&name) {
            missing.push(name);
        }
    }
    let mut in_keys = Vec::new();
    toml_key_placeholders(&doc, &mut in_keys);
    Some((text_out, changed, in_keys))
}

/// Substitute `${NAME}` in every UTF-8 file body and every string leaf of
/// every merge value in `plan`. A file whose path ends in `.toml` is
/// substituted as TOML (`substitute_toml`): only string values, re-serialized
/// — and if it does not parse, as raw text with a plan warning. Non-UTF-8
/// files are left untouched (a binary asset file can't contain a text
/// placeholder). A file/merge whose
/// content is unchanged by substitution is not listed in `secret_files` /
/// `secret_merge_files` even if the plan happened to reference a name that
/// resolved to an unchanged value textually (e.g. a value equal to its own
/// placeholder is still a "change" in this implementation, since it did
/// perform a substitution rather than passing text through untouched).
pub fn substitute(plan: &RenderPlan, values: &BTreeMap<String, String>) -> Substituted {
    let mut missing = Vec::new();
    let mut secret_files = BTreeSet::new();
    let mut secret_merge_files = BTreeSet::new();
    let mut key_placeholders = Vec::new();

    let mut warnings = plan.warnings.clone();

    let files = plan
        .files
        .iter()
        .map(|f| match std::str::from_utf8(&f.bytes) {
            Ok(text) => {
                let as_toml = if f.path.ends_with(".toml") && text.contains("${") {
                    let parsed = substitute_toml(text, values, &mut missing);
                    match &parsed {
                        None => {
                            tracing::warn!(
                                path = %f.path,
                                "rendered TOML file does not parse; secrets substituted as raw text"
                            );
                            warnings.push(format!(
                                "{}: not a valid TOML document; secrets substituted as raw text",
                                f.path
                            ));
                        }
                        Some((_, _, in_keys)) if !in_keys.is_empty() => {
                            warnings.push(format!(
                                "{}: a ${{NAME}} in a TOML key is not substituted ({})",
                                f.path,
                                in_keys.join(", ")
                            ));
                            for name in in_keys {
                                let entry = format!("{}: {name}", f.path);
                                if !key_placeholders.contains(&entry) {
                                    key_placeholders.push(entry);
                                }
                            }
                        }
                        Some(_) => {}
                    }
                    parsed.map(|(out, changed, _)| (out, changed))
                } else {
                    None
                };
                let (out, changed) = match as_toml {
                    Some(done) => done,
                    None => substitute_str(text, values, &mut missing),
                };
                if changed {
                    secret_files.insert(f.path.clone());
                }
                FileWrite {
                    path: f.path.clone(),
                    bytes: out.into_bytes(),
                }
            }
            Err(_) => f.clone(),
        })
        .collect();

    let merges = plan
        .merges
        .iter()
        .map(|m| {
            let (value, changed) = substitute_value(&m.value, values, &mut missing);
            if changed {
                secret_merge_files.insert(m.file.clone());
            }
            ConfigMerge {
                file: m.file.clone(),
                json_path: m.json_path.clone(),
                mode: m.mode,
                value,
            }
        })
        .collect();

    Substituted {
        plan: SecretPlan::from(RenderPlan {
            files,
            merges,
            placeholders: plan.placeholders.clone(),
            warnings,
        }),
        missing,
        secret_files,
        secret_merge_files,
        key_placeholders,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::catalog::harness::codex::Codex;
    use crate::service::catalog::harness::{Harness, MergeMode};
    use crate::service::catalog::model::Asset;
    use serde_json::json;

    fn store_with(setup: impl FnOnce(&Store)) -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        setup(&s);
        Mutex::new(s)
    }

    #[test]
    fn resolve_prefers_host_override_over_global_and_adds_token() {
        let store = store_with(|s| {
            s.set_secret("A", None, "global-a").unwrap();
            s.set_secret("A", Some("mefistos"), "host-a").unwrap();
            s.upsert_host_token("mefistos", "tok-mef").unwrap();
        });

        let mef = resolve(&store, "mefistos").unwrap();
        assert_eq!(mef["A"], "host-a");
        assert_eq!(mef[BUILTIN_TOKEN], "tok-mef");
        assert_eq!(mef[BUILTIN_PORT], "4180");

        let local = resolve(&store, "local").unwrap();
        assert_eq!(
            local["A"], "global-a",
            "no override for local, falls back to global"
        );
        assert!(
            !local.contains_key(BUILTIN_TOKEN),
            "no token row for local means the built-in isn't provided"
        );
    }

    #[test]
    fn resolve_reads_the_configured_port_setting() {
        let store = store_with(|s| {
            s.set_setting(SETTING_PORT, "9999").unwrap();
        });
        let values = resolve(&store, "local").unwrap();
        assert_eq!(values[BUILTIN_PORT], "9999");
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn substitute_replaces_in_file_bytes_and_nested_merge_values() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"token is ${TOKEN} for real".to_vec(),
        });
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/unchanged.md".into(),
            bytes: b"nothing to see here".to_vec(),
        });
        plan.merges.push(ConfigMerge {
            file: "~/.claude/settings.json".into(),
            json_path: vec!["mcpServers".into()],
            mode: MergeMode::Set,
            value: json!({
                "fleet": {
                    "headers": {"Authorization": "Bearer ${TOKEN}"},
                    "tags": ["a", "${MISSING_NAME}"],
                }
            }),
        });

        let out = substitute(&plan, &values(&[("TOKEN", "sekret")]));

        let skill_md = out
            .plan
            .inner()
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .unwrap();
        assert_eq!(
            std::str::from_utf8(&skill_md.bytes).unwrap(),
            "token is sekret for real"
        );
        assert!(out.secret_files.contains("~/.claude/skills/s/SKILL.md"));
        assert!(!out.secret_files.contains("~/.claude/skills/s/unchanged.md"));

        let merged = &out.plan.inner().merges[0].value;
        assert_eq!(merged["fleet"]["headers"]["Authorization"], "Bearer sekret");
        assert_eq!(merged["fleet"]["tags"][1], "${MISSING_NAME}");
        assert!(out.secret_merge_files.contains("~/.claude/settings.json"));

        assert_eq!(out.missing, vec!["MISSING_NAME".to_string()]);
    }

    #[test]
    fn substitute_leaves_non_utf8_files_untouched() {
        let mut plan = RenderPlan::default();
        let bytes = vec![0xff, 0xfe, 0x00, 0x01];
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/blob.bin".into(),
            bytes: bytes.clone(),
        });
        let out = substitute(&plan, &BTreeMap::new());
        assert_eq!(out.plan.inner().files[0].bytes, bytes);
        assert!(out.secret_files.is_empty());
    }

    #[test]
    fn substitute_ignores_non_placeholder_looking_braces() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"literal ${lowercase} and unterminated ${OOPS".to_vec(),
        });
        let out = substitute(&plan, &BTreeMap::new());
        assert_eq!(
            std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap(),
            "literal ${lowercase} and unterminated ${OOPS"
        );
        assert!(out.missing.is_empty());
        assert!(out.secret_files.is_empty());
    }

    #[test]
    fn substitute_replaces_adjacent_placeholders() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"${A}${B}!".to_vec(),
        });
        let out = substitute(&plan, &values(&[("A", "aa"), ("B", "bb")]));
        assert_eq!(
            std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap(),
            "aabb!"
        );
        assert!(out.missing.is_empty());
    }

    #[test]
    fn substitute_handles_a_literal_dollar_prefix() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"$${A} literally".to_vec(),
        });
        let out = substitute(&plan, &values(&[("A", "aa")]));
        assert_eq!(
            std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap(),
            "$aa literally",
            "the leading '$' is a literal character, not part of the placeholder"
        );
        assert!(out.missing.is_empty());
    }

    #[test]
    fn debug_of_substituted_never_prints_secret_values() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/skills/s/SKILL.md".into(),
            bytes: b"token is ${TOKEN}".to_vec(),
        });
        plan.merges.push(ConfigMerge {
            file: "~/.claude/settings.json".into(),
            json_path: vec!["mcpServers".into(), "fleet".into()],
            mode: MergeMode::Set,
            value: json!({"auth": "${TOKEN}"}),
        });

        let out = substitute(&plan, &values(&[("TOKEN", "sekret-value-123")]));
        // Sanity: the substitution actually happened.
        assert_eq!(
            std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap(),
            "token is sekret-value-123"
        );

        let debug = format!("{out:?}");
        assert!(
            !debug.contains("sekret-value-123"),
            "Debug output must never contain a substituted secret value: {debug}"
        );
        assert!(
            debug.contains("~/.claude/skills/s/SKILL.md"),
            "Debug output should still name the affected file: {debug}"
        );
    }

    #[test]
    fn substitute_matches_find_placeholders_naming_rule() {
        // Sanity check that our own placeholder scan agrees with the
        // shared `find_placeholders` used elsewhere for the same syntax.
        let text = "Bearer ${FLEET_MCP_TOKEN} and ${X_1} and ${lower}";
        let mut missing = Vec::new();
        substitute_str(text, &BTreeMap::new(), &mut missing);
        assert_eq!(missing, find_placeholders(text));
    }

    /// A secret holding `"`, `\`, `'` and a newline, substituted into a
    /// Codex subagent's TOML (through `targets.codex.extra`), leaves a
    /// document that parses and holds the exact secret value.
    #[test]
    fn substitute_writes_a_secret_into_codex_agent_toml_as_a_toml_string() {
        let mut agent = Asset::from_yaml(
            None,
            "kind: agent\nname: pm\ndescription: d\ntargets:\n  codex:\n    extra:\n      mcp_servers:\n        jira:\n          command: npx\n          env:\n            TOKEN: \"${TOKEN}\"\n",
        )
        .unwrap();
        agent.body = "Use ${TOKEN} carefully.\n".into();
        let plan = Codex.render(&agent).unwrap();
        assert_eq!(plan.files[0].path, "~/.codex/agents/pm.toml");
        let secret = "a\"b\\c'd\ne";

        let out = substitute(&plan, &values(&[("TOKEN", secret)]));

        let text = std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap();
        let doc: toml::Table = toml::from_str(text).expect("still a TOML document");
        assert_eq!(
            doc["mcp_servers"]["jira"]["env"]["TOKEN"].as_str(),
            Some(secret)
        );
        assert_eq!(
            doc["developer_instructions"].as_str(),
            Some(format!("Use {secret} carefully.\n").as_str())
        );
        assert_eq!(doc["name"].as_str(), Some("pm"));
        assert!(out.secret_files.contains("~/.codex/agents/pm.toml"));
        assert!(out.missing.is_empty());
        assert_eq!(out.plan.inner().warnings, plan.warnings);
    }

    /// A `${NAME}` in a TOML KEY is never substituted, so the file would
    /// install with the literal placeholder in it even when the secret
    /// resolves. It is reported as such, not as a missing secret — and
    /// `plan.rs` blocks the asset on it, so the warning is no longer the only
    /// trace (`a_placeholder_in_a_toml_key_blocks_the_asset`).
    #[test]
    fn substitute_names_every_placeholder_a_toml_key_holds() {
        let mut plan = RenderPlan::default();
        let text = "[servers.\"${TOKEN}\"]\ncommand = \"npx\"\n[env]\n\"${TOKEN}\" = \"x\"\n\"${OTHER}\" = \"${TOKEN}\"\n";
        plan.files.push(FileWrite {
            path: "~/.codex/agents/pm.toml".into(),
            bytes: text.as_bytes().to_vec(),
        });
        let out = substitute(
            &plan,
            &values(&[("TOKEN", "sekret-value-123"), ("OTHER", "o")]),
        );
        assert_eq!(
            out.key_placeholders,
            vec![
                // A `toml::Table` is sorted by key, so this is the parsed
                // document's order, not the file's.
                "~/.codex/agents/pm.toml: OTHER".to_string(),
                "~/.codex/agents/pm.toml: TOKEN".to_string(),
            ],
            "once each"
        );
        // Resolvable, so not missing — and the VALUE is still substituted.
        assert!(out.missing.is_empty());
        let warn = out.plan.inner().warnings.join(" | ");
        assert!(warn.contains("TOKEN") && warn.contains("OTHER"), "{warn}");
        let written = std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap();
        assert!(
            written.contains("\"${TOKEN}\" ="),
            "the key is left alone: {written}"
        );
    }

    #[test]
    fn substitute_in_toml_reports_missing_names_and_keeps_an_untouched_file_byte_exact() {
        let mut plan = RenderPlan::default();
        let text = "name = \"pm\"\n# ${NOT_A_VALUE}\ntoken = \"${GONE}\"\n";
        plan.files.push(FileWrite {
            path: "~/.codex/agents/pm.toml".into(),
            bytes: text.as_bytes().to_vec(),
        });
        let out = substitute(&plan, &BTreeMap::new());
        assert_eq!(out.plan.inner().files[0].bytes, text.as_bytes());
        assert_eq!(out.missing, vec!["GONE".to_string()]);
        assert!(out.secret_files.is_empty());
    }

    /// A `.toml` file that does not parse still gets its placeholders
    /// replaced (as raw text), and the plan says so.
    #[test]
    fn substitute_falls_back_to_raw_text_for_toml_that_does_not_parse() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.codex/agents/bad.toml".into(),
            bytes: b"not [ valid = ${A}".to_vec(),
        });
        let out = substitute(&plan, &values(&[("A", "x")]));
        assert_eq!(
            std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap(),
            "not [ valid = x"
        );
        assert_eq!(out.plan.inner().warnings.len(), 1);
        assert!(
            out.plan.inner().warnings[0].contains("~/.codex/agents/bad.toml"),
            "{:?}",
            out.plan.inner().warnings
        );
    }

    /// Everything that is not a `.toml` file keeps the raw text replace.
    #[test]
    fn substitute_keeps_raw_replacement_for_non_toml_files() {
        let mut plan = RenderPlan::default();
        plan.files.push(FileWrite {
            path: "~/.claude/agents/pm.md".into(),
            bytes: b"token = \"${T}\"\n".to_vec(),
        });
        let out = substitute(&plan, &values(&[("T", "a\"b")]));
        assert_eq!(
            std::str::from_utf8(&out.plan.inner().files[0].bytes).unwrap(),
            "token = \"a\"b\"\n"
        );
        assert!(out.plan.inner().warnings.is_empty());
    }
}
