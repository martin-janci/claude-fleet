//! How the router's tools are *presented* to a client: scoped to what the
//! caller may actually call, with the schema noise stripped and the MCP
//! behaviour hints attached.
//!
//! The served tool list is the client's context window — a tool definition is
//! paid for by every request the client makes, whether or not it calls the
//! tool. Three rules apply here, all of them derived from
//! [`guard::TOOL_POLICIES`] so nothing is hand-maintained twice:
//!
//! 1. **Scope** ([`visible_to`]): a caller is served only the tools its token
//!    may call. The predicates are the same ones `enforce_mode` /
//!    `enforce_admin` apply on the call itself, so the list can never offer
//!    what the call would refuse. Before this, a `readonly` token was served
//!    ~30 tools it could only be refused, and a paired client the fleet-admin
//!    surface — context spent on `E_FORBIDDEN`, and a worse tool choice.
//! 2. **Slimming** ([`slim_schema`]): `schemars` emits `$schema`, `title` and
//!    numeric `format`s on every object, `"default": null` on every optional
//!    field, and keeps the hard-wrapped newlines of the doc comments. None of
//!    it changes what the model may send; together it measured ~12% of this
//!    server's definition budget.
//! 3. **Hints** ([`annotations_for`]): `readOnlyHint` / `destructiveHint` let
//!    a client auto-approve reads and gate writes instead of asking about
//!    every call.

use super::guard;
use super::Caller;
use super::TokenMode;
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Map, Value};
use std::sync::Arc;

/// Numeric `format`s that tell a model nothing it can act on: JSON has one
/// number type, and the server validates ranges itself. Semantic formats
/// (`date-time`, `uri`, …) are kept — those do change what a caller sends.
const NOISE_FORMATS: &[&str] = &[
    "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "float",
    "double",
];

/// Subschema positions whose value is a single schema object.
const SUBSCHEMA: &[&str] = &[
    "items",
    "additionalProperties",
    "not",
    "if",
    "then",
    "else",
    "contains",
    "propertyNames",
];
/// Subschema positions whose value is an array of schema objects.
const SUBSCHEMA_LIST: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];
/// Positions whose value is a MAP of name → schema. The map's own keys are
/// property names, never schema keywords, so nothing is stripped from it —
/// only from the schemas inside. (A parameter actually named `title` would
/// otherwise be deleted.)
const SUBSCHEMA_MAP: &[&str] = &["properties", "definitions", "$defs", "patternProperties"];

/// True when `caller` may call `tool` — the union of the two gates
/// `call_tool` enforces (`enforce_mode`, `enforce_admin`). Kept next to them
/// in spirit: if either gate changes, this must change with it, and
/// `tools::tests` checks the two agree for every router tool.
pub(super) fn visible_to(caller: &Caller, tool: &str) -> bool {
    // An updater token lists nothing: its only door is `/update/*`.
    if caller.mode == TokenMode::Updater {
        return false;
    }
    let peer_tool = tool == crate::mcp::auth::PEER_TOOL;
    if caller.mode == TokenMode::Peer || peer_tool {
        return caller.mode == TokenMode::Peer && peer_tool;
    }
    if !guard::mode_allows_tool(caller.mode, tool) {
        return false;
    }
    // Refused to every per-host token at the central gate (`enforce_admin`);
    // listing it would cost every host's Claude a definition it can never use.
    if caller.host_alias.is_some() && guard::NOT_FOR_HOST_TOKENS.contains(&tool) {
        return false;
    }
    guard::access_allows(caller, tool)
}

/// MCP behaviour hints for one tool, from its policy row. Only the hint that
/// says something is emitted: a read is marked `readOnlyHint`, a
/// confirmation-gated mutation `destructiveHint`, and everything else carries
/// no annotations rather than a row of defaults.
pub(super) fn annotations_for(tool: &str) -> Option<ToolAnnotations> {
    if guard::is_readonly_tool(tool) {
        Some(ToolAnnotations::new().read_only(true))
    } else if guard::needs_confirmation(tool) {
        Some(ToolAnnotations::new().destructive(true))
    } else {
        None
    }
}

/// Collapse every run of whitespace to a single space. Doc comments are
/// hard-wrapped for the Rust source; the wrapping is padding on the wire.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = false;
            out.push(c);
        }
    }
    out
}

/// Strip the keywords that cost tokens and change nothing, in place, over a
/// JSON Schema object and every subschema under it.
pub(super) fn slim_schema(schema: &mut Map<String, Value>) {
    schema.remove("$schema");
    schema.remove("title");
    if schema
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|f| NOISE_FORMATS.contains(&f))
    {
        schema.remove("format");
    }
    // `"default": null` on an `Option<T>` restates the type's own nullability.
    if schema.get("default").is_some_and(Value::is_null) {
        schema.remove("default");
    }
    // `"minimum": 0` is what schemars says for every unsigned integer; a
    // negative number fails deserialization with a clear error anyway.
    if schema.get("minimum").and_then(Value::as_u64) == Some(0) && is_integer(schema) {
        schema.remove("minimum");
    }
    drop_optional_null(schema);
    if let Some(Value::String(d)) = schema.get_mut("description") {
        *d = collapse_ws(d);
    }
    for key in SUBSCHEMA_MAP {
        if let Some(Value::Object(children)) = schema.get_mut(*key) {
            for child in children.values_mut() {
                if let Value::Object(o) = child {
                    slim_schema(o);
                }
            }
        }
    }
    for key in SUBSCHEMA {
        if let Some(Value::Object(o)) = schema.get_mut(*key) {
            slim_schema(o);
        }
    }
    for key in SUBSCHEMA_LIST {
        if let Some(Value::Array(items)) = schema.get_mut(*key) {
            for item in items.iter_mut() {
                if let Value::Object(o) = item {
                    slim_schema(o);
                }
            }
        }
    }
}

fn is_integer(schema: &Map<String, Value>) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == "integer",
        Some(Value::Array(ts)) => ts.iter().any(|t| t == "integer"),
        _ => false,
    }
}

/// An `Option<T>` parameter is emitted as `"type": ["T", "null"]` (or an
/// `anyOf` of `T` and `{"type": "null"}`), yet it is also left out of
/// `required`: "may be omitted" already says everything the model needs, and
/// omitting it is what a model does. `null` still deserializes to `None`, so
/// a client that sends it is not refused. Only properties NOT in `required`
/// are touched: a required nullable field keeps its `null`. On this server it
/// was ~380 properties, ~3.4 KB of every client's definitions (2026-10-10).
fn drop_optional_null(schema: &mut Map<String, Value>) {
    let required: Vec<String> = match schema.get("required") {
        Some(Value::Array(r)) => r
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        _ => Vec::new(),
    };
    let Some(Value::Object(props)) = schema.get_mut("properties") else {
        return;
    };
    for (name, prop) in props.iter_mut() {
        if required.contains(name) {
            continue;
        }
        let Value::Object(prop) = prop else { continue };
        if let Some(Value::Array(types)) = prop.get_mut("type") {
            types.retain(|t| t != "null");
            if types.len() == 1 {
                let only = types.remove(0);
                prop.insert("type".into(), only);
            }
        }
        if let Some(Value::Array(variants)) = prop.get_mut("enum") {
            variants.retain(|v| !v.is_null());
        }
        let is_null = |v: &Value| {
            v.get("type").is_some_and(|t| t == "null")
                && v.as_object().is_some_and(|o| o.len() == 1)
        };
        if let Some(Value::Array(any)) = prop.get("anyOf") {
            if any.len() == 2 && any.iter().any(is_null) {
                let keep = any.iter().find(|v| !is_null(v)).cloned();
                if let Some(Value::Object(inner)) = keep {
                    prop.remove("anyOf");
                    for (k, v) in inner {
                        prop.entry(k).or_insert(v);
                    }
                }
            }
        }
    }
}

/// One router tool as it goes on the wire: slimmed schema, collapsed
/// description, policy-derived annotations.
pub(super) fn present(mut tool: Tool) -> Tool {
    let mut schema = (*tool.input_schema).clone();
    slim_schema(&mut schema);
    tool.input_schema = Arc::new(schema);
    if let Some(desc) = tool.description.take() {
        tool.description = Some(collapse_ws(&desc).into());
    }
    tool.annotations = annotations_for(&tool.name);
    tool
}
