//! Store bindings (spec *D. Stores*): what an instance binds each store
//! role of its definition to.
//!
//! * An `mcp` role binds a catalog `mcp_server` asset under a binding name,
//!   with env overrides that point the asset's `${NAME}` placeholders at
//!   other secret NAMES (`{"GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}"}`).
//!   A value is only ever a secret reference, never a literal, so a rule
//!   holds no secret. Each run renders the binding afresh
//!   ([`render_entry`]): the asset's entry, the overrides, then the secrets
//!   the catalog's own resolver gives the rule's host.
//! * A `fleet` role binds a namespace of `agent_records`: `rule` (the rule's
//!   own, `rule:<id>`) or `shared:<name>`, which every rule binding the same
//!   name reads and writes.
//!
//! [`check`] is PURE: the bindings against the definition and the catalog.
//! [`mcp_config`] assembles a run's `--mcp-config` and the MCP tools the run
//! may call: each role's `read_tools`, never a `write_tools` entry.

use super::definition::{Definition, StoreKind, StoreRole};
use crate::service::catalog::model::{find_placeholders, AssetSpec};
use crate::service::catalog::sync::secrets::{is_valid_secret_name, substitute_str};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// The binding name of the built-in read server over the fleet store.
pub const RECORDS_SERVER: &str = "fleet-records";

/// One role's binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub role: String,
    /// mcp: the catalog `mcp_server` asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// mcp: the name the run knows the server by; the role when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// mcp: placeholder → `${SECRET_NAME}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// fleet: `rule` or `shared:<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

impl Binding {
    /// The name an mcp binding's server has in the run.
    pub fn server_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.role)
    }
}

/// PURE: a rule's `bindings` column.
pub fn parse(json: &str) -> Result<Vec<Binding>, String> {
    serde_json::from_str(json).map_err(|e| format!("bindings: {e}"))
}

fn binding_name_ok(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase())
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn shared_name_ok(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase() || f.is_ascii_digit())
        && s.len() <= 60
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// PURE: the secret an override value names: exactly `${NAME}`.
pub fn secret_ref(value: &str) -> Option<&str> {
    value
        .strip_prefix("${")
        .and_then(|v| v.strip_suffix('}'))
        .filter(|n| is_valid_secret_name(n))
}

/// The strings of an `mcp_server` asset that may hold placeholders.
fn server_strings(spec: &AssetSpec) -> Vec<&str> {
    match spec {
        AssetSpec::McpServer {
            url,
            headers,
            command,
            args,
            env,
            ..
        } => url
            .iter()
            .chain(command.iter())
            .map(String::as_str)
            .chain(headers.values().map(String::as_str))
            .chain(args.iter().map(String::as_str))
            .chain(env.values().map(String::as_str))
            .collect(),
        _ => Vec::new(),
    }
}

/// PURE: an `mcp_server` asset's placeholders, in first-seen order.
pub fn server_placeholders(spec: &AssetSpec) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in server_strings(spec) {
        for p in find_placeholders(s) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// PURE: every problem of `bindings` for `def`. `server` looks an asset up
/// in the catalog: its spec when it is an `mcp_server`.
pub fn check(
    def: &Definition,
    bindings: &[Binding],
    server: &dyn Fn(&str) -> Option<AssetSpec>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen_roles: Vec<&str> = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for b in bindings {
        let at = format!("bindings › {}", b.role);
        let Some(role) = def.stores.iter().find(|s| s.role == b.role) else {
            out.push(format!("{at}: the definition has no such store role"));
            continue;
        };
        if seen_roles.contains(&b.role.as_str()) {
            out.push(format!("{at}: bound twice"));
        }
        seen_roles.push(&b.role);
        match role.kind {
            StoreKind::Mcp => check_mcp(b, &at, server, &mut names, &mut out),
            StoreKind::Fleet => check_fleet(b, &at, &mut out),
        }
    }
    for r in def.stores.iter().filter(|r| r.required) {
        if !bindings.iter().any(|b| b.role == r.role) {
            out.push(format!("bindings › {}: required, and not bound", r.role));
        }
    }
    out
}

fn check_mcp<'a>(
    b: &'a Binding,
    at: &str,
    server: &dyn Fn(&str) -> Option<AssetSpec>,
    names: &mut Vec<&'a str>,
    out: &mut Vec<String>,
) {
    if b.namespace.is_some() {
        out.push(format!("{at}: an mcp role binds a server, not a namespace"));
    }
    let name = b.server_name();
    if !binding_name_ok(name) {
        out.push(format!(
            "{at}: binding name {name:?} is [a-z][a-z0-9_-]*, at most 40"
        ));
    } else if name == RECORDS_SERVER || name == super::diagnostics::SERVER {
        out.push(format!("{at}: {name} is fleet's own server"));
    }
    if names.contains(&name) {
        out.push(format!("{at}: binding name {name} is used twice"));
    }
    names.push(name);
    let Some(asset) = b.server.as_deref() else {
        out.push(format!("{at}: name the catalog MCP server"));
        return;
    };
    let Some(spec) = server(asset) else {
        out.push(format!("{at}: the catalog has no MCP server {asset:?}"));
        return;
    };
    let placeholders = server_placeholders(&spec);
    for (k, v) in &b.env {
        if !placeholders.contains(k) {
            out.push(format!(
                "{at}: {asset} has no ${{{k}}} to override ({})",
                if placeholders.is_empty() {
                    "it has none".to_string()
                } else {
                    placeholders.join(", ")
                }
            ));
        }
        if secret_ref(v).is_none() {
            out.push(format!(
                "{at}: {k} must name a secret as ${{NAME}}, never a value"
            ));
        }
    }
}

fn check_fleet(b: &Binding, at: &str, out: &mut Vec<String>) {
    if b.server.is_some() || b.name.is_some() || !b.env.is_empty() {
        out.push(format!("{at}: a fleet role binds a namespace only"));
    }
    match b.namespace.as_deref() {
        Some("rule") => {}
        Some(n) => match n.strip_prefix("shared:") {
            Some(s) if shared_name_ok(s) => {}
            _ => out.push(format!(
                "{at}: namespace {n:?} is rule or shared:<name> ([a-z0-9][a-z0-9_-]*, at most 60)"
            )),
        },
        None => out.push(format!("{at}: name the namespace: rule or shared:<name>")),
    }
}

/// PURE: the namespace each fleet role of rule `rule_id` reads and writes.
pub fn namespaces(rule_id: i64, bindings: &[Binding]) -> BTreeMap<String, String> {
    bindings
        .iter()
        .filter_map(|b| {
            let ns = b.namespace.as_deref()?;
            let ns = if ns == "rule" {
                format!("rule:{rule_id}")
            } else {
                ns.to_string()
            };
            Some((b.role.clone(), ns))
        })
        .collect()
}

/// PURE: an `mcp_server` asset rendered for one binding: its entry as
/// `claude --mcp-config` takes it, each placeholder replaced by the secret
/// the override names (or its own name), from `secrets`. Errors name the
/// missing secrets, never a value.
pub fn render_entry(
    spec: &AssetSpec,
    overrides: &BTreeMap<String, String>,
    secrets: &BTreeMap<String, String>,
) -> Result<Value, String> {
    let AssetSpec::McpServer {
        transport,
        url,
        headers,
        command,
        args,
        env,
    } = spec
    else {
        return Err("not an MCP server".into());
    };
    let mut values = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    for p in server_placeholders(spec) {
        let secret = overrides
            .get(&p)
            .and_then(|v| secret_ref(v))
            .unwrap_or(&p)
            .to_string();
        match secrets.get(&secret) {
            Some(v) => {
                values.insert(p, v.clone());
            }
            None => {
                if !missing.contains(&secret) {
                    missing.push(secret);
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!("missing secret(s): {}", missing.join(", ")));
    }
    let sub = |s: &str| substitute_str(s, &values, &mut Vec::new()).0;
    let mut obj = Map::new();
    obj.insert("type".into(), json!(transport));
    if transport == "http" {
        obj.insert("url".into(), json!(sub(url.as_deref().unwrap_or_default())));
        if !headers.is_empty() {
            let h: Map<String, Value> = headers
                .iter()
                .map(|(k, v)| (k.clone(), json!(sub(v))))
                .collect();
            obj.insert("headers".into(), Value::Object(h));
        }
    } else {
        obj.insert(
            "command".into(),
            json!(sub(command.as_deref().unwrap_or_default())),
        );
        if !args.is_empty() {
            let a: Vec<String> = args.iter().map(|a| sub(a)).collect();
            obj.insert("args".into(), json!(a));
        }
        if !env.is_empty() {
            let e: Map<String, Value> = env
                .iter()
                .map(|(k, v)| (k.clone(), json!(sub(v))))
                .collect();
            obj.insert("env".into(), Value::Object(e));
        }
    }
    Ok(Value::Object(obj))
}

/// PURE: the tools a run may call on a role's binding.
pub fn allowed_tools(role: &StoreRole, binding: &Binding) -> Vec<String> {
    role.read_tools
        .iter()
        .map(|t| format!("mcp__{}__{t}", binding.server_name()))
        .collect()
}

/// A run's MCP config, secrets in it: keep it off logs and command lines.
#[derive(Clone, PartialEq)]
pub struct RunMcp {
    /// `{"mcpServers": …}`; `None` when the run binds nothing.
    pub config: Option<String>,
    /// `mcp__<binding>__<tool>` for every read tool, and the records
    /// server's tools when the run has one.
    pub allowed_tools: Vec<String>,
}

impl std::fmt::Debug for RunMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunMcp")
            .field("config", &self.config.as_ref().map(|c| c.len()))
            .field("allowed_tools", &self.allowed_tools)
            .finish()
    }
}

/// PURE: a run's MCP servers: each mcp binding rendered, plus
/// `records` (the [`RECORDS_SERVER`] entry) when the rule binds a fleet
/// namespace, and `diagnostics` (`fleet-diagnostics`) when the definition
/// sets `run.diagnostics`. `server` looks an asset up as in [`check`].
pub fn mcp_config(
    def: &Definition,
    bindings: &[Binding],
    server: &dyn Fn(&str) -> Option<AssetSpec>,
    secrets: &BTreeMap<String, String>,
    records: Option<Value>,
    diagnostics: Option<Value>,
) -> Result<RunMcp, String> {
    let mut servers = Map::new();
    let mut allowed = Vec::new();
    for b in bindings {
        let Some(role) = def.stores.iter().find(|s| s.role == b.role) else {
            return Err(format!("bindings › {}: no such store role", b.role));
        };
        if role.kind != StoreKind::Mcp {
            continue;
        }
        let asset = b.server.as_deref().unwrap_or_default();
        let spec = server(asset).ok_or_else(|| format!("no MCP server {asset:?}"))?;
        let entry = render_entry(&spec, &b.env, secrets)
            .map_err(|e| format!("bindings › {}: {e}", b.role))?;
        servers.insert(b.server_name().to_string(), entry);
        allowed.extend(allowed_tools(role, b));
    }
    if let Some(r) = records {
        servers.insert(RECORDS_SERVER.into(), r);
        allowed.extend(
            super::records::TOOLS
                .iter()
                .map(|t| format!("mcp__{RECORDS_SERVER}__{t}")),
        );
    }
    if let Some(d) = diagnostics {
        use super::diagnostics::{SERVER, TOOLS};
        servers.insert(SERVER.into(), d);
        allowed.extend(TOOLS.iter().map(|t| format!("mcp__{SERVER}__{t}")));
    }
    Ok(RunMcp {
        config: (!servers.is_empty()).then(|| json!({ "mcpServers": servers }).to_string()),
        allowed_tools: allowed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> Definition {
        super::super::definition::parse_value(&json!({
            "stores": [
                { "role": "logs", "kind": "mcp", "label": "Logs", "required": true,
                  "read_tools": ["search_logs"] },
                { "role": "kb", "kind": "mcp", "label": "KB", "read_tools": ["search_docs"],
                  "write_tools": ["add_known_error"] },
                { "role": "memory", "kind": "fleet", "label": "Memory" }
            ],
            "trigger": { "source": "manual" }
        }))
        .unwrap()
    }

    fn graylog() -> AssetSpec {
        AssetSpec::McpServer {
            transport: "stdio".into(),
            url: None,
            headers: BTreeMap::new(),
            command: Some("node".into()),
            args: vec!["${SUPPORT_AGENT_DIR}/dist/mcp/graylog/index.js".into()],
            env: [
                (
                    "GRAYLOG_BASE_URL".to_string(),
                    "${GRAYLOG_BASE_URL}".to_string(),
                ),
                (
                    "GRAYLOG_API_TOKEN".to_string(),
                    "${GRAYLOG_API_TOKEN}".to_string(),
                ),
            ]
            .into(),
        }
    }

    fn lookup(name: &str) -> Option<AssetSpec> {
        (name == "graylog").then(graylog)
    }

    fn b(v: Value) -> Binding {
        serde_json::from_value(v).unwrap()
    }

    fn prod() -> Binding {
        b(
            json!({ "role": "logs", "server": "graylog", "name": "logs-prod",
            "env": { "GRAYLOG_BASE_URL": "${GRAYLOG_PROD_URL}",
                     "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } }),
        )
    }

    #[test]
    fn good_bindings_have_no_problem() {
        let bs = vec![
            prod(),
            b(json!({ "role": "memory", "namespace": "shared:l2-fingerprints" })),
        ];
        assert_eq!(check(&def(), &bs, &lookup), Vec::<String>::new());
        assert_eq!(
            namespaces(7, &bs),
            [("memory".to_string(), "shared:l2-fingerprints".to_string())].into()
        );
        let own = vec![b(json!({ "role": "memory", "namespace": "rule" }))];
        assert_eq!(namespaces(7, &own)["memory"], "rule:7");
    }

    #[test]
    fn bad_bindings_are_named() {
        let cases = [
            (
                json!({ "role": "nope", "namespace": "rule" }),
                "no such store role",
            ),
            (
                json!({ "role": "logs", "server": "jira" }),
                "no MCP server \"jira\"",
            ),
            (json!({ "role": "logs" }), "name the catalog MCP server"),
            (
                json!({ "role": "logs", "server": "graylog", "env": { "GRAYLOG_API_TOKEN": "s3cret" } }),
                "never a value",
            ),
            (
                json!({ "role": "logs", "server": "graylog", "env": { "OTHER": "${X}" } }),
                "has no ${OTHER}",
            ),
            (
                json!({ "role": "logs", "server": "graylog", "name": "fleet-records" }),
                "fleet's own server",
            ),
            (
                json!({ "role": "logs", "server": "graylog", "name": "Logs" }),
                "binding name",
            ),
            (
                json!({ "role": "logs", "namespace": "rule", "server": "graylog" }),
                "not a namespace",
            ),
            (
                json!({ "role": "memory", "namespace": "shared:" }),
                "is rule or shared",
            ),
            (
                json!({ "role": "memory", "namespace": "rule:9" }),
                "is rule or shared",
            ),
            (json!({ "role": "memory" }), "name the namespace"),
            (
                json!({ "role": "memory", "namespace": "rule", "server": "graylog" }),
                "namespace only",
            ),
        ];
        for (v, want) in cases {
            let mut bs = vec![b(v.clone())];
            if v["role"] != "logs" {
                bs.push(prod());
            }
            let p = check(&def(), &bs, &lookup).join("; ");
            assert!(p.contains(want), "{v}: {p}");
        }
        let p = check(&def(), &[], &lookup).join("; ");
        assert!(p.contains("logs: required, and not bound"), "{p}");
        let twice = vec![prod(), prod()];
        let p = check(&def(), &twice, &lookup).join("; ");
        assert!(p.contains("bound twice") && p.contains("used twice"), "{p}");
    }

    #[test]
    fn a_binding_renders_with_its_own_secrets() {
        let secrets: BTreeMap<String, String> = [
            ("GRAYLOG_PROD_URL", "https://g.prod"),
            ("GRAYLOG_PROD_TOKEN", "t0k${GRAYLOG_BASE_URL}"),
            ("SUPPORT_AGENT_DIR", "/opt/sa"),
            ("GRAYLOG_BASE_URL", "https://g.default"),
            ("GRAYLOG_API_TOKEN", "t-default"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let e = render_entry(&graylog(), &prod().env, &secrets).unwrap();
        assert_eq!(
            e,
            json!({ "type": "stdio", "command": "node",
                "args": ["/opt/sa/dist/mcp/graylog/index.js"],
                "env": { "GRAYLOG_BASE_URL": "https://g.prod",
                         "GRAYLOG_API_TOKEN": "t0k${GRAYLOG_BASE_URL}" } }),
            "a secret's own text is never substituted again"
        );
        let default = render_entry(&graylog(), &BTreeMap::new(), &secrets).unwrap();
        assert_eq!(default["env"]["GRAYLOG_BASE_URL"], "https://g.default");
        let mut few = secrets.clone();
        few.remove("GRAYLOG_PROD_TOKEN");
        let err = render_entry(&graylog(), &prod().env, &few).unwrap_err();
        assert_eq!(err, "missing secret(s): GRAYLOG_PROD_TOKEN");
    }

    #[test]
    fn a_run_offers_read_tools_only_and_the_records_server() {
        let secrets: BTreeMap<String, String> = [
            ("GRAYLOG_PROD_URL", "u"),
            ("GRAYLOG_PROD_TOKEN", "t"),
            ("SUPPORT_AGENT_DIR", "/d"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let kb = b(json!({ "role": "kb", "server": "graylog", "env": {
            "GRAYLOG_BASE_URL": "${GRAYLOG_PROD_URL}", "GRAYLOG_API_TOKEN": "${GRAYLOG_PROD_TOKEN}" } }));
        let bs = vec![
            prod(),
            kb,
            b(json!({ "role": "memory", "namespace": "rule" })),
        ];
        let run = mcp_config(
            &def(),
            &bs,
            &lookup,
            &secrets,
            Some(json!({ "type": "http", "url": "u" })),
            Some(json!({ "type": "http", "url": "d" })),
        )
        .unwrap();
        assert_eq!(
            run.allowed_tools,
            [
                "mcp__logs-prod__search_logs",
                "mcp__kb__search_docs",
                "mcp__fleet-records__get",
                "mcp__fleet-records__find",
                "mcp__fleet-diagnostics__hub_log",
                "mcp__fleet-diagnostics__errors",
                "mcp__fleet-diagnostics__timeline",
                "mcp__fleet-diagnostics__fleet",
                "mcp__fleet-diagnostics__host_log",
                "mcp__fleet-diagnostics__session",
                "mcp__fleet-diagnostics__client_log",
                "mcp__fleet-diagnostics__analyses",
                "mcp__fleet-diagnostics__proposals"
            ]
        );
        let cfg: Value = serde_json::from_str(run.config.as_deref().unwrap()).unwrap();
        let names: Vec<&String> = cfg["mcpServers"].as_object().unwrap().keys().collect();
        assert_eq!(
            names,
            ["fleet-diagnostics", "fleet-records", "kb", "logs-prod"]
        );
        assert!(
            !format!("{run:?}").contains("/d/dist"),
            "Debug prints no config"
        );
        let none = mcp_config(&def(), &[], &lookup, &secrets, None, None).unwrap();
        assert_eq!((none.config, none.allowed_tools.len()), (None, 0));
    }
}
