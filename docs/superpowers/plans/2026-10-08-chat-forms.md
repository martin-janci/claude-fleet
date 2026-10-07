# Chat Forms (part 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An agent calls the control API's new `ask` tool with a `fleet.form/1` form; the form appears as a step-by-step card in the desktop's Conversation panel (and is answerable through a hub); the person's answers return to the agent as the tool's result, with secrets written to 0600 files on the session's host.

**Architecture:** A pure spec model + validator + answer checker (`pages/forms.rs`), a `form_requests` table with a store `Notify` (the `wait_for_reply` pattern), a service layer (`service/forms.rs`) that waits, answers, writes secrets with `provision::write_host_file_secret` and sweeps them on the reconcile tick, one MCP tool `ask` for both sides, four desktop commands routed to it on a hub, and a Svelte card driven by `pending_form` on the session row (no new event kind).

**Tech Stack:** Rust (fleet-core, rmcp, rusqlite, tokio), Tauri 2 (src-tauri), Svelte 5 runes, Vitest, bash (hub-e2e).

**Spec:** `docs/superpowers/specs/2026-10-07-chat-forms-design.md`

## Global Constraints

- **Validation ladder (CLAUDE.md):** after every edit `cargo fleet-fast-check` (Rust) / `npx svelte-check` (frontend); per task `cargo fleet-lint` + `cargo fleet-test -- <filter>` / `npx vitest run <file>`; before the last commit `scripts/verify.sh full`. Never `cargo build` to check compilation, never `-p <crate>` in the inner loop (except the `REGEN_*` commands quoted below, which are the repo's own). `pnpm` scripts are not on PATH here: use `npx vitest run`, `npx svelte-check`.
- **Shell quoting:** every value in a remote command string goes through `crate::shell::quote`. A secret value never enters argv or a script string.
- **Store lock:** never hold the `Store` mutex guard across an `.await`. Take what you need under a short lock, drop it, then do I/O.
- **Errors:** `IpcError` with `codes::*`. The not-found code is `E_NOTFOUND`. New codes: `E_NOT_A_SESSION`, `E_HOST_WRITE`.
- **Limits (spec §2):** ≤ 12 steps, ≤ 40 fields, ≤ 50 options, ≤ 16 KiB spec JSON, `title` ≤ 120, `label` ≤ 200, `intro`/`help`/`why`/`note` ≤ 500, `name` matches `[a-z][a-z0-9_]*` and ≤ 40, `text.max_len` default 500 cap 2000, `textarea.max_len` default 5000 cap 20000.
- **Lifetimes:** wait default and max 600 s; a pending form expires after 24 h; decided rows are deleted after 7 days.
- **Secret path:** `~/.cache/claude-fleet/forms/<form_id>/<field>`; `form_id` = `f_` + 16 characters of `[0-9A-Za-z]`.
- **Result note, verbatim:** `Delete each secret file once you have used it.`
- **Migration number:** take it from `origin/main` (`git ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1`); this plan assumes 112. If main moved, use the next free number everywhere this plan says 112.
- **Contract:** `CONTRACT_REVISION`, `MIN_HUB_CONTRACT`, `MAX_HUB_CONTRACT` all go 8 → 9 in the same commit (the desktop now routes to a hub tool that did not exist before).
- **One writer per file per task.** Tasks touching the same file are ordered; do not run them in parallel.
- Subagents never run `git pull/push/rebase/checkout/stash`; commit only in this worktree.

---

## File map

| File | Task | Responsibility |
|---|---|---|
| `crates/fleet-core/src/pages/forms.rs` (new) | 1 | `fleet.form/1` model, spec validator, answer checker |
| `crates/fleet-core/src/pages/forms_tests.rs` (new) | 1 | its tests, the shared examples, the schema doc |
| `crates/fleet-core/src/pages/mod.rs` | 1 | `pub mod forms;` |
| `docs/form-examples/specs.json`, `docs/form-examples/answers.json` (new) | 1 | shared cases, run by Rust and TS |
| `docs/form-spec.schema.json` (generated) | 1 | JSON Schema |
| `crates/fleet-core/migrations/112_form_requests.sql` (new) | 2 | the table |
| `crates/fleet-core/src/store/schema.rs` | 2 | register 112 |
| `crates/fleet-core/src/store/forms.rs` (new) | 2 | rows, queries, notify, row_version bump |
| `crates/fleet-core/src/store/mod.rs` | 2 | `mod forms;`, re-exports, `form_notify` field |
| `crates/fleet-core/src/store/rows.rs` | 2 | `pending_form` on `SessionRow` |
| `crates/fleet-core/src/mcp/tools/views.rs` | 2 | phone field |
| `src-tauri/src/backend/hub_contract.golden.json`, `tests_contract.rs` | 2 | wire names |
| `crates/fleet-core/src/service/forms.rs` (new) | 3 | open / wait / cancel / get / list / answer / decline / tick / sweep |
| `crates/fleet-core/src/service/mod.rs` | 3 | `pub mod forms;` |
| `crates/fleet-core/src/service/attention.rs` | 3 | waiting on a pending form |
| `crates/fleet-core/src/service/sessions/lifecycle.rs` | 3 | `record_kill` cancels |
| `crates/fleet-core/src/service/tick.rs` | 3 | expiry, purge, sweep |
| `crates/fleet-core/src/ipc_error.rs` | 3 | two codes |
| `crates/fleet-core/src/mcp/tools/params.rs`, `forms.rs` (new), `mod.rs`, `guard.rs`, `tests.rs` | 4 | the `ask` tool |
| `docs/control-api.md`, `docs/control-api-reference.md` | 4 | docs |
| `src-tauri/src/commands/forms.rs` (new), `commands/mod.rs`, `lib.rs`, `backend/verdicts.rs`, `backend/tests_routing.rs`, `crates/fleet-core/src/wire_contract.rs`, `backend/contract.rs` | 5 | desktop commands |
| `src/lib/forms/forms.ts`, `form_model.ts`, `form_model.test.ts` (new) | 6 | TS types, API, model |
| `src/lib/forms/FormWizard.svelte`, `FormCard.svelte`, `FormCard.test.ts` (new) | 7 | the UI |
| `src/lib/ConversationPanel.svelte`, `SessionRowItem.svelte`, `sessions.ts`, `attention.ts`, `share.ts`, `hub.ts`, `conversation.ts` + tests | 8 | integration |
| `docs/forms.md` (new), `skills/claude-fleet-control/SKILL.md`, `docs/status.md`, `scripts/hub-e2e.sh` | 9 | docs, e2e |

---

### Task 1: The form spec model, validator and answer checker

**Files:**
- Create: `crates/fleet-core/src/pages/forms.rs`
- Create: `crates/fleet-core/src/pages/forms_tests.rs`
- Modify: `crates/fleet-core/src/pages/mod.rs:17-28` (module list)
- Create: `docs/form-examples/specs.json`, `docs/form-examples/answers.json`
- Generate: `docs/form-spec.schema.json`

**Interfaces:**
- Produces (all `pub` in `fleet_core::pages::forms`):
  - `FormSpec { spec, title, intro: Option<String>, submit: Option<String>, steps: Vec<FormStep> }`
  - `FormStep { title, intro, when: Option<FieldCondition>, fields: Vec<FormField> }`
  - `FormField { name, kind: FieldType, label, help, required: bool, value: Option<Value>, when, placeholder, max_len: Option<u32>, min: Option<f64>, max: Option<f64>, integer: bool, options: Option<Vec<(String, String)>> }`
  - `enum FieldType { Text, Textarea, Number, Bool, Select, Multiselect, Secret }`
  - `FieldCondition { field, eq, one_of (serde "in"), truthy, all, any, not }`
  - `fn parse(spec: &serde_json::Value) -> Result<FormSpec, Vec<String>>`
  - `fn validate(form: &FormSpec) -> Vec<String>`
  - `fn check_answers(form: &FormSpec, values: &serde_json::Map<String, Value>) -> Result<Answers, Vec<FieldProblem>>`
  - `Answers { values: Map<String, Value>, secrets: BTreeMap<String, String> }`
  - `FieldProblem { field: String, problem: String }` (Serialize + Deserialize)
  - consts `FORM_SPEC_VERSION`, `MAX_STEPS`, `MAX_FIELDS`, `MAX_OPTIONS`, `MAX_SPEC_BYTES`, `MAX_TEXT`

- [ ] **Step 1: Write the shared examples**

`docs/form-examples/specs.json` (a valid spec has `"problems": []`):

```json
{
  "_about": "Shared fleet.form/1 cases. crates/fleet-core/src/pages/forms_tests.rs runs every case; problems are exact strings.",
  "cases": [
    {
      "name": "the spec's own example",
      "spec": {
        "spec": "fleet.form/1", "title": "New project", "submit": "Create",
        "steps": [
          { "title": "Basics", "fields": [
            { "name": "name", "type": "text", "label": "Name", "required": true, "placeholder": "my-app" },
            { "name": "kind", "type": "select", "label": "Kind", "options": [["web", "Web app"], ["cli", "CLI"]] },
            { "name": "db", "type": "bool", "label": "Needs a database", "value": false } ] },
          { "title": "Database", "when": { "field": "db", "truthy": true }, "fields": [
            { "name": "engine", "type": "select", "label": "Engine", "options": [["pg", "Postgres"], ["sqlite", "SQLite"]] },
            { "name": "db_pass", "type": "secret", "label": "Password" } ] } ] },
      "problems": []
    },
    {
      "name": "a condition on a later field",
      "spec": { "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "when": { "field": "b", "truthy": true }, "fields": [ { "name": "a", "type": "text", "label": "A" } ] },
        { "title": "B", "fields": [ { "name": "b", "type": "bool", "label": "B" } ] } ] },
      "problems": ["step 1 › when: `b` is not an earlier field"]
    },
    {
      "name": "a condition reading a secret",
      "spec": { "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [
          { "name": "pw", "type": "secret", "label": "Password" },
          { "name": "x", "type": "text", "label": "X", "when": { "field": "pw", "eq": "hunter2" } } ] } ] },
      "problems": ["step 1 › field 2 (x) › when: `pw` is a secret; a condition cannot read it"]
    },
    {
      "name": "a secret with a default",
      "spec": { "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [ { "name": "pw", "type": "secret", "label": "Password", "value": "x" } ] } ] },
      "problems": ["step 1 › field 1 (pw): a secret has no default value"]
    },
    {
      "name": "options on a text field, duplicate name, bad name",
      "spec": { "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [
          { "name": "a", "type": "text", "label": "A", "options": [["x", "X"]] },
          { "name": "a", "type": "bool", "label": "Again" },
          { "name": "Bad-Name", "type": "bool", "label": "B" } ] } ] },
      "problems": [
        "step 1 › field 1 (a): `options` is not for a text field",
        "step 1 › field 2 (a): another field has this name",
        "step 1 › field 3 (Bad-Name): a name is lowercase letters, digits and _, starting with a letter, at most 40 characters"
      ]
    },
    {
      "name": "two steps with one title, a select without options, truthy on text",
      "spec": { "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "Same", "fields": [ { "name": "a", "type": "text", "label": "A" } ] },
        { "title": "Same", "when": { "field": "a", "truthy": true }, "fields": [ { "name": "s", "type": "select", "label": "S" } ] } ] },
      "problems": [
        "step 2: another step has this title",
        "step 2 › when: `a`: truthy is for on/off fields; use eq or in",
        "step 2 › field 1 (s): a select field needs 1 to 50 options"
      ]
    },
    {
      "name": "the wrong version and an empty form",
      "spec": { "spec": "fleet.page/1", "title": "T", "steps": [] },
      "problems": ["spec must be \"fleet.form/1\"", "a form has 1 to 12 steps"]
    },
    {
      "name": "a default outside the bounds",
      "spec": { "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [ { "name": "n", "type": "number", "label": "N", "min": 1, "max": 5, "value": 9 } ] } ] },
      "problems": ["step 1 › field 1 (n): the default must be at most 5"]
    }
  ]
}
```

`docs/form-examples/answers.json` (run by Rust here and by TS in Task 6):

```json
{
  "_about": "Shared answer cases for fleet.form/1. Rust (forms_tests.rs) and TS (form_model.test.ts) must agree exactly.",
  "spec": {
    "spec": "fleet.form/1", "title": "New project",
    "steps": [
      { "title": "Basics", "fields": [
        { "name": "name", "type": "text", "label": "Name", "required": true, "max_len": 10 },
        { "name": "kind", "type": "select", "label": "Kind", "options": [["web", "Web app"], ["cli", "CLI"]] },
        { "name": "tags", "type": "multiselect", "label": "Tags", "options": [["a", "A"], ["b", "B"], ["c", "C"]] },
        { "name": "port", "type": "number", "label": "Port", "integer": true, "min": 1, "max": 65535 },
        { "name": "db", "type": "bool", "label": "Database" },
        { "name": "agree", "type": "bool", "label": "I agree", "required": true } ] },
      { "title": "Database", "when": { "field": "db", "truthy": true }, "fields": [
        { "name": "engine", "type": "select", "label": "Engine", "required": true, "options": [["pg", "Postgres"], ["sqlite", "SQLite"]] },
        { "name": "db_pass", "type": "secret", "label": "Password", "when": { "field": "engine", "eq": "pg" } } ] }
    ]
  },
  "cases": [
    {
      "name": "everything filled, the database step shown",
      "values": { "name": "my-app", "kind": "web", "tags": ["c", "a"], "port": 8080, "db": true, "agree": true, "engine": "pg", "db_pass": "s3cret" },
      "answers": { "name": "my-app", "kind": "web", "tags": ["a", "c"], "port": 8080, "db": true, "agree": true, "engine": "pg" },
      "secrets": ["db_pass"]
    },
    {
      "name": "the database step hidden drops its values",
      "values": { "name": "x", "db": false, "agree": true, "engine": "pg", "db_pass": "s3cret" },
      "answers": { "name": "x", "db": false, "agree": true },
      "secrets": []
    },
    {
      "name": "a blank required text and an unticked consent",
      "values": { "name": "   ", "agree": false },
      "problems": [ { "field": "name", "problem": "is required" }, { "field": "agree", "problem": "is required" } ]
    },
    {
      "name": "wrong shapes and bounds",
      "values": { "name": "far too long a name", "kind": "tui", "tags": ["a", "z"], "port": 80.5, "db": "yes", "agree": true },
      "problems": [
        { "field": "name", "problem": "is longer than 10 characters" },
        { "field": "kind", "problem": "must be one of the options" },
        { "field": "tags", "problem": "must be a list of the options" },
        { "field": "port", "problem": "must be a whole number" },
        { "field": "db", "problem": "must be on or off" }
      ]
    },
    {
      "name": "a number under its minimum, and a field the form does not have",
      "values": { "name": "x", "port": 0, "agree": true, "colour": "red" },
      "problems": [ { "field": "port", "problem": "must be at least 1" }, { "field": "colour", "problem": "is not a field of this form" } ]
    },
    {
      "name": "the database step shown but its required select empty",
      "values": { "name": "x", "db": true, "agree": true },
      "problems": [ { "field": "engine", "problem": "is required" } ]
    }
  ]
}
```

- [ ] **Step 2: Write the failing tests**

`crates/fleet-core/src/pages/forms_tests.rs`:

```rust
use super::forms::*;
use serde_json::{json, Map, Value};

fn cases(rel: &str) -> Value {
    serde_json::from_str(&crate::repo_files::read(rel)).expect(rel)
}

#[test]
fn every_shared_spec_case_reports_exactly_its_problems() {
    let doc = cases("docs/form-examples/specs.json");
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let want: Vec<String> = serde_json::from_value(case["problems"].clone()).unwrap();
        let got = match parse(&case["spec"]) {
            Ok(_) => vec![],
            Err(p) => p,
        };
        assert_eq!(got, want, "case {name:?}");
    }
}

#[test]
fn every_shared_answer_case_agrees() {
    let doc = cases("docs/form-examples/answers.json");
    let form = parse(&doc["spec"]).expect("the answers spec is valid");
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let values: Map<String, Value> = serde_json::from_value(case["values"].clone()).unwrap();
        match check_answers(&form, &values) {
            Ok(a) => {
                assert_eq!(Value::Object(a.values), case["answers"], "case {name:?}");
                let secrets: Vec<&str> = a.secrets.keys().map(String::as_str).collect();
                let want: Vec<&str> = case["secrets"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
                assert_eq!(secrets, want, "case {name:?}");
            }
            Err(p) => {
                assert_eq!(serde_json::to_value(&p).unwrap(), case["problems"], "case {name:?}");
            }
        }
    }
}

#[test]
fn a_secrets_value_is_kept_apart_from_the_answers() {
    let form = parse(&json!({ "spec": "fleet.form/1", "title": "T", "steps": [
        { "title": "A", "fields": [ { "name": "pw", "type": "secret", "label": "P", "required": true } ] } ] }))
    .unwrap();
    let values: Map<String, Value> = serde_json::from_value(json!({ "pw": "hunter2" })).unwrap();
    let a = check_answers(&form, &values).unwrap();
    assert!(a.values.is_empty(), "never in the answers: {:?}", a.values);
    assert_eq!(a.secrets.get("pw").map(String::as_str), Some("hunter2"));
}

#[test]
fn an_oversized_spec_is_refused_before_parsing() {
    let big = "x".repeat(MAX_SPEC_BYTES);
    let err = parse(&json!({ "spec": "fleet.form/1", "title": big, "steps": [] })).unwrap_err();
    assert!(err[0].contains("bytes, over the 16384 a form may have"), "{err:?}");
}

#[test]
fn unknown_keys_are_refused() {
    let err = parse(&json!({ "spec": "fleet.form/1", "title": "T", "colour": "red", "steps": [] })).unwrap_err();
    assert!(err[0].starts_with("not a fleet.form/1 spec:"), "{err:?}");
}

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

#[test]
fn form_docs_are_current() {
    let schema = rmcp::schemars::schema_for!(FormSpec);
    let text = serde_json::to_string_pretty(&schema).unwrap() + "\n";
    let path = repo_path("docs/form-spec.schema.json");
    if std::env::var("REGEN_FORM_DOCS").is_ok() {
        std::fs::write(&path, &text).expect("write");
        panic!("wrote docs/form-spec.schema.json — read the diff, then run again without REGEN_FORM_DOCS");
    }
    assert!(
        std::fs::read_to_string(&path).unwrap_or_default() == text,
        "\n\ndocs/form-spec.schema.json is out of date with crates/fleet-core/src/pages/forms.rs. Regenerate with:\n  \
REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current\n"
    );
}
```

In `crates/fleet-core/src/pages/mod.rs`, after `pub mod flows;` add `pub mod forms;`, and after `mod tests;` add:

```rust
#[cfg(test)]
mod forms_tests;
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo fleet-test -- pages::forms_tests`
Expected: compile error, `could not find forms in pages` (the module does not exist yet).

- [ ] **Step 4: Write the model and the checks**

`crates/fleet-core/src/pages/forms.rs`:

```rust
//! `fleet.form/1`: a form an agent asks a person to fill, step by step
//! (chat forms, `docs/superpowers/specs/2026-10-07-chat-forms-design.md`).
//! Data only. This module parses and validates a spec and checks a
//! person's answers against it; the TS twin is `src/lib/forms/form_model.ts`,
//! and both run `docs/form-examples/*.json`. `docs/form-spec.schema.json`
//! is generated from these types (`REGEN_FORM_DOCS=1`).

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The one form spec version this build reads.
pub const FORM_SPEC_VERSION: &str = "fleet.form/1";
pub const MAX_STEPS: usize = 12;
pub const MAX_FIELDS: usize = 40;
pub const MAX_OPTIONS: usize = 50;
pub const MAX_SPEC_BYTES: usize = 16 * 1024;
pub const MAX_TITLE: usize = 120;
pub const MAX_LABEL: usize = 200;
/// `intro`, `help`, and the tool's `why` / `note`.
pub const MAX_TEXT: usize = 500;
pub const MAX_NAME: usize = 40;
const MAX_SUBMIT: usize = 40;
const TEXT_LEN: (u32, u32) = (500, 2000);
const TEXTAREA_LEN: (u32, u32) = (5000, 20000);

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct FormSpec {
    /// Always "fleet.form/1".
    pub spec: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    /// The last step's button. Default "Submit".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<String>,
    pub steps: Vec<FormStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct FormStep {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<FieldCondition>,
    pub fields: Vec<FormField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Textarea,
    Number,
    Bool,
    Select,
    Multiselect,
    /// Written to a 0600 file on the session's host; the agent gets the path.
    Secret,
}

impl FieldType {
    fn word(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Textarea => "textarea",
            FieldType::Number => "number",
            FieldType::Bool => "bool",
            FieldType::Select => "select",
            FieldType::Multiselect => "multiselect",
            FieldType::Secret => "secret",
        }
    }
}

/// One field. Flat, like a page `Condition`: which keys a type may carry is
/// the validator's to say, not serde's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct FormField {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    /// The default. Never on a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<FieldCondition>,
    /// text, textarea.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// text (default 500, ≤ 2000), textarea (default 5000, ≤ 20000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<u32>,
    /// number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// number: whole numbers only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub integer: bool,
    /// select, multiselect: `[value, label]` pairs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<(String, String)>>,
}

/// When a step or a field is asked. Exactly one form: `{field, eq}`,
/// `{field, in}`, `{field, truthy}`, `{all}`, `{any}` or `{not}`. `field`
/// names an EARLIER field, never a secret.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct FieldCondition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq: Option<Value>,
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truthy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<FieldCondition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<FieldCondition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<Box<FieldCondition>>,
}

/// What a person answered: the visible, non-secret values (typed, in form
/// order), and each visible secret's value, kept apart so it is never
/// stored or returned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answers {
    pub values: Map<String, Value>,
    pub secrets: BTreeMap<String, String>,
}

/// One field's problem with an answer, in the words the card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldProblem {
    pub field: String,
    pub problem: String,
}

/// Parse and validate `spec`: the form, or every problem with where it is.
pub fn parse(spec: &Value) -> Result<FormSpec, Vec<String>> {
    let size = spec.to_string().len();
    if size > MAX_SPEC_BYTES {
        return Err(vec![format!(
            "the spec is {size} bytes, over the {MAX_SPEC_BYTES} a form may have"
        )]);
    }
    let form: FormSpec = serde_json::from_value(spec.clone())
        .map_err(|e| vec![format!("not a fleet.form/1 spec: {e}")])?;
    let problems = validate(&form);
    if problems.is_empty() {
        Ok(form)
    } else {
        Err(problems)
    }
}

struct Problems(Vec<String>);

impl Problems {
    fn bad(&mut self, at: &str, message: impl Into<String>) {
        let message = message.into();
        self.0.push(if at.is_empty() {
            message
        } else {
            format!("{at}: {message}")
        });
    }

    fn text(&mut self, at: &str, what: &str, text: &str, max: usize) {
        if text.trim().is_empty() {
            self.bad(at, format!("{what} must not be empty"));
        } else if text.chars().count() > max {
            self.bad(at, format!("{what} is longer than {max} characters"));
        }
    }
}

/// Every problem in `form`. Empty means valid.
pub fn validate(form: &FormSpec) -> Vec<String> {
    let mut v = Problems(Vec::new());
    if form.spec != FORM_SPEC_VERSION {
        v.bad("", format!("spec must be {FORM_SPEC_VERSION:?}"));
    }
    v.text("", "title", &form.title, MAX_TITLE);
    if let Some(t) = &form.intro {
        v.text("", "intro", t, MAX_TEXT);
    }
    if let Some(t) = &form.submit {
        v.text("", "submit", t, MAX_SUBMIT);
    }
    if form.steps.is_empty() || form.steps.len() > MAX_STEPS {
        v.bad("", format!("a form has 1 to {MAX_STEPS} steps"));
    }
    let total: usize = form.steps.iter().map(|s| s.fields.len()).sum();
    if total > MAX_FIELDS {
        v.bad("", format!("{total} fields, over the {MAX_FIELDS} a form may have"));
    }
    let mut titles = BTreeSet::new();
    let mut seen: BTreeMap<&str, &FormField> = BTreeMap::new();
    for (i, step) in form.steps.iter().enumerate() {
        let at = format!("step {}", i + 1);
        v.text(&at, "title", &step.title, MAX_TITLE);
        if !titles.insert(step.title.as_str()) {
            v.bad(&at, "another step has this title");
        }
        if let Some(t) = &step.intro {
            v.text(&at, "intro", t, MAX_TEXT);
        }
        if let Some(c) = &step.when {
            check_condition(&mut v, &format!("{at} › when"), c, &seen);
        }
        if step.fields.is_empty() {
            v.bad(&at, "a step has at least one field");
        }
        for (j, f) in step.fields.iter().enumerate() {
            let fat = format!("{at} › field {} ({})", j + 1, f.name);
            check_field(&mut v, &fat, f, &seen);
            if seen.insert(f.name.as_str(), f).is_some() {
                v.bad(&fat, "another field has this name");
            }
        }
    }
    v.0
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_NAME
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn check_field(v: &mut Problems, at: &str, f: &FormField, seen: &BTreeMap<&str, &FormField>) {
    use FieldType::*;
    if !valid_name(&f.name) {
        v.bad(
            at,
            format!("a name is lowercase letters, digits and _, starting with a letter, at most {MAX_NAME} characters"),
        );
    }
    v.text(at, "label", &f.label, MAX_LABEL);
    if let Some(t) = &f.help {
        v.text(at, "help", t, MAX_TEXT);
    }
    let word = f.kind.word();
    let not_for = |v: &mut Problems, key: &str| v.bad(at, format!("`{key}` is not for a {word} field"));
    if f.placeholder.is_some() && !matches!(f.kind, Text | Textarea) {
        not_for(v, "placeholder");
    }
    if let Some(p) = &f.placeholder {
        v.text(at, "placeholder", p, MAX_LABEL);
    }
    match (f.kind, f.max_len) {
        (Text, Some(n)) if n == 0 || n > TEXT_LEN.1 => {
            v.bad(at, format!("max_len is 1 to {}", TEXT_LEN.1))
        }
        (Textarea, Some(n)) if n == 0 || n > TEXTAREA_LEN.1 => {
            v.bad(at, format!("max_len is 1 to {}", TEXTAREA_LEN.1))
        }
        (Text | Textarea, _) | (_, None) => {}
        (_, Some(_)) => not_for(v, "max_len"),
    }
    if f.kind != Number {
        if f.min.is_some() {
            not_for(v, "min");
        }
        if f.max.is_some() {
            not_for(v, "max");
        }
        if f.integer {
            not_for(v, "integer");
        }
    } else if let (Some(lo), Some(hi)) = (f.min, f.max) {
        if lo > hi {
            v.bad(at, "min is above max");
        }
    }
    match (&f.options, f.kind) {
        (Some(opts), Select | Multiselect) => {
            if opts.is_empty() || opts.len() > MAX_OPTIONS {
                v.bad(at, format!("a {word} field needs 1 to {MAX_OPTIONS} options"));
            }
            let mut values = BTreeSet::new();
            for (value, label) in opts {
                if !values.insert(value.as_str()) {
                    v.bad(at, format!("option value {value:?} appears twice"));
                }
                if value.is_empty() {
                    v.bad(at, "an option value must not be empty");
                }
                v.text(at, "an option label", label, MAX_LABEL);
            }
        }
        (None, Select | Multiselect) => {
            v.bad(at, format!("a {word} field needs 1 to {MAX_OPTIONS} options"))
        }
        (Some(_), _) => not_for(v, "options"),
        (None, _) => {}
    }
    if let Some(d) = &f.value {
        if f.kind == Secret {
            v.bad(at, "a secret has no default value");
        } else if let Err(p) = check_value(f, d) {
            v.bad(at, format!("the default {p}"));
        }
    }
    if let Some(c) = &f.when {
        check_condition(v, &format!("{at} › when"), c, seen);
    }
}

fn check_condition(
    v: &mut Problems,
    at: &str,
    c: &FieldCondition,
    seen: &BTreeMap<&str, &FormField>,
) {
    let forms = [c.field.is_some(), c.all.is_some(), c.any.is_some(), c.not.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    if forms != 1 {
        v.bad(at, "a condition is exactly one of {field, …}, {all}, {any} or {not}");
        return;
    }
    if let Some(name) = &c.field {
        let tests = [c.eq.is_some(), c.one_of.is_some(), c.truthy.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if tests != 1 {
            v.bad(at, format!("`{name}`: say exactly one of eq, in or truthy"));
        }
        let Some(target) = seen.get(name.as_str()) else {
            v.bad(at, format!("`{name}` is not an earlier field"));
            return;
        };
        if target.kind == FieldType::Secret {
            v.bad(at, format!("`{name}` is a secret; a condition cannot read it"));
            return;
        }
        if c.truthy.is_some() && target.kind != FieldType::Bool {
            v.bad(at, format!("`{name}`: truthy is for on/off fields; use eq or in"));
        }
        if c.one_of.as_ref().is_some_and(Vec::is_empty) {
            v.bad(at, format!("`{name}`: `in` needs at least one value"));
        }
        for want in c.eq.iter().chain(c.one_of.iter().flatten()) {
            let ok = if target.kind == FieldType::Multiselect {
                want.as_str().is_some_and(|w| option_values(target).any(|o| o == w))
            } else {
                check_value(target, want).is_ok()
            };
            if !ok {
                v.bad(at, format!("`{name}` can never be {want}"));
            }
        }
    } else if c.eq.is_some() || c.one_of.is_some() || c.truthy.is_some() {
        v.bad(at, "eq, in and truthy need a field");
    }
    for (i, sub) in c.all.iter().chain(c.any.iter()).flatten().enumerate() {
        check_condition(v, &format!("{at} › condition {}", i + 1), sub, seen);
    }
    if c.all.as_ref().is_some_and(Vec::is_empty) || c.any.as_ref().is_some_and(Vec::is_empty) {
        v.bad(at, "all / any need at least one condition");
    }
    if let Some(sub) = &c.not {
        check_condition(v, &format!("{at} › not"), sub, seen);
    }
}

fn option_values(f: &FormField) -> impl Iterator<Item = &str> {
    f.options.iter().flatten().map(|(v, _)| v.as_str())
}

/// `v` as an answer to `f`, normalised (a multiselect in option order), or
/// what is wrong with it. The words are the card's: keep them in step with
/// `form_model.ts`.
fn check_value(f: &FormField, v: &Value) -> Result<Value, String> {
    use FieldType::*;
    match f.kind {
        Text | Textarea | Secret => {
            let s = v.as_str().ok_or("must be text")?;
            let cap = match f.kind {
                Text => f.max_len.unwrap_or(TEXT_LEN.0),
                Textarea => f.max_len.unwrap_or(TEXTAREA_LEN.0),
                _ => TEXT_LEN.1,
            } as usize;
            if s.chars().count() > cap {
                return Err(format!("is longer than {cap} characters"));
            }
            Ok(Value::String(s.to_string()))
        }
        Number => {
            let n = v.as_f64().ok_or("must be a number")?;
            if f.integer && n.fract() != 0.0 {
                return Err("must be a whole number".into());
            }
            if let Some(lo) = f.min.filter(|lo| n < *lo) {
                return Err(format!("must be at least {lo}"));
            }
            if let Some(hi) = f.max.filter(|hi| n > *hi) {
                return Err(format!("must be at most {hi}"));
            }
            Ok(v.clone())
        }
        Bool => v.as_bool().map(Value::Bool).ok_or_else(|| "must be on or off".into()),
        Select => match v.as_str() {
            Some(s) if option_values(f).any(|o| o == s) => Ok(v.clone()),
            _ => Err("must be one of the options".into()),
        },
        Multiselect => {
            let items = v.as_array().ok_or("must be a list of the options")?;
            let picked: BTreeSet<&str> = items.iter().filter_map(Value::as_str).collect();
            if picked.len() != items.len() || !picked.iter().all(|p| option_values(f).any(|o| o == *p)) {
                return Err("must be a list of the options".into());
            }
            Ok(Value::Array(
                option_values(f)
                    .filter(|o| picked.contains(o))
                    .map(|o| Value::String(o.to_string()))
                    .collect(),
            ))
        }
    }
}

/// Blank: no answer at all (whitespace text, an empty list, null).
fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Does `c` hold over the answers given so far? A hidden field has no
/// answer, so a condition on it reads it as absent.
pub fn holds(c: Option<&FieldCondition>, shown: &Map<String, Value>) -> bool {
    let Some(c) = c else { return true };
    if let Some(all) = &c.all {
        return all.iter().all(|x| holds(Some(x), shown));
    }
    if let Some(any) = &c.any {
        return any.iter().any(|x| holds(Some(x), shown));
    }
    if let Some(not) = &c.not {
        return !holds(Some(not), shown);
    }
    let Some(name) = &c.field else { return true };
    let got = shown.get(name);
    let matches = |want: &Value| match got {
        Some(Value::Array(items)) => items.iter().any(|i| same(i, want)),
        Some(g) => same(g, want),
        None => false,
    };
    if let Some(want) = &c.eq {
        return matches(want);
    }
    if let Some(list) = &c.one_of {
        return list.iter().any(matches);
    }
    if let Some(t) = c.truthy {
        return (got == Some(&Value::Bool(true))) == t;
    }
    true
}

/// Check a person's `values` against `form`: the accepted answers, or each
/// field's problem in form order (then names the form does not have).
/// Values of hidden steps and fields are dropped, not checked.
pub fn check_answers(
    form: &FormSpec,
    values: &Map<String, Value>,
) -> Result<Answers, Vec<FieldProblem>> {
    let mut problems = Vec::new();
    let mut out = Answers::default();
    let mut known = BTreeSet::new();
    let problem = |f: &FormField, p: String| FieldProblem {
        field: f.name.clone(),
        problem: p,
    };
    for step in &form.steps {
        for f in &step.fields {
            known.insert(f.name.as_str());
        }
        if !holds(step.when.as_ref(), &out.values) {
            continue;
        }
        for f in &step.fields {
            if !holds(f.when.as_ref(), &out.values) {
                continue;
            }
            let Some(given) = values.get(&f.name).filter(|v| !is_blank(v)) else {
                if f.required {
                    problems.push(problem(f, "is required".into()));
                }
                continue;
            };
            match check_value(f, given) {
                Err(p) => problems.push(problem(f, p)),
                Ok(Value::Bool(false)) if f.required => {
                    problems.push(problem(f, "is required".into()))
                }
                Ok(norm) if f.kind == FieldType::Secret => {
                    out.secrets
                        .insert(f.name.clone(), norm.as_str().unwrap_or_default().to_string());
                }
                Ok(norm) => {
                    out.values.insert(f.name.clone(), norm);
                }
            }
        }
    }
    for k in values.keys() {
        if !known.contains(k.as_str()) {
            problems.push(FieldProblem {
                field: k.clone(),
                problem: "is not a field of this form".into(),
            });
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}
```

Note on `out.values`: it is built in form order only if `serde_json` keeps insertion order. Check `crates/fleet-core/Cargo.toml` for `serde_json = { …, features = ["preserve_order"] }`; if it is absent, `Map` is sorted by key, the comparison in `every_shared_answer_case_agrees` is still by value (`Value::Object` equality ignores order), and nothing else depends on order. Do not add the feature.

- [ ] **Step 5: Generate the schema, then run the tests**

Run: `REGEN_FORM_DOCS=1 cargo fleet-test -- form_docs_are_current`
Expected: FAIL with "wrote docs/form-spec.schema.json — read the diff" (a REGEN run fails on purpose). Read `docs/form-spec.schema.json`; it must start with `"$schema"` and `"title": "FormSpec"`.

Run: `cargo fleet-test -- pages::forms_tests`
Expected: PASS, 6 tests. If a shared case's problem text differs, fix the code, not the case, unless the case contradicts the spec.

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo fleet-lint`
Expected: no warnings.

```bash
git add crates/fleet-core/src/pages/forms.rs crates/fleet-core/src/pages/forms_tests.rs crates/fleet-core/src/pages/mod.rs docs/form-examples docs/form-spec.schema.json
git commit -m "feat(forms): the fleet.form/1 spec, its validator and answer checker"
```

---

### Task 2: The `form_requests` table, its store, and `pending_form` on the session row

**Files:**
- Create: `crates/fleet-core/migrations/112_form_requests.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (`MIGRATIONS` tail, after the 111 entry at ~:1281)
- Create: `crates/fleet-core/src/store/forms.rs`
- Modify: `crates/fleet-core/src/store/mod.rs` (`mod forms;` in the list at :11-64, a `pub use`, the `form_notify` field next to `message_notify` at :188-196, its initialisation in every constructor at :433, :468, :507, :534, :558 and in `store/schema.rs` where `message_notify` is initialised, and the accessor next to :702)
- Modify: `crates/fleet-core/src/store/rows.rs` (`SessionRow` :122-343, `SESSION_COLUMNS` :412-497, `map_session_row` :530-623)
- Modify: `crates/fleet-core/src/mcp/tools/views.rs:96-125` (`PHONE_SESSION_FIELDS`) and the pinned lists in `crates/fleet-core/src/mcp/tools/tests.rs` (:4946, :5030, :6811-6869)
- Modify: `src-tauri/src/backend/tests_contract.rs` (`sample_session` :43, the 63-names test :993), `src-tauri/src/backend/hub_contract.golden.json` (regenerated)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `store::FormRow { id: i64, form_id: String, session_id: i64, host_alias: String, spec: String, why: Option<String>, state: String, answers: Option<String>, note: Option<String>, answered_by: Option<String>, secrets_on_host: bool, created_at: i64, decided_at: Option<i64> }`
  - `store::NewForm<'a> { form_id: &'a str, session_id: i64, host_alias: &'a str, spec: &'a str, why: Option<&'a str> }`
  - `store::FormFinish<'a> { state: &'a str, answers: Option<&'a str>, note: Option<&'a str>, answered_by: Option<&'a str>, secrets_on_host: bool }`
  - `store::PendingForm { form_id: String, title: String }` and `SessionRow.pending_form: Option<PendingForm>`
  - `impl Store`: `insert_form(&NewForm) -> rusqlite::Result<FormRow>`, `form(&str) -> Result<Option<FormRow>>`, `pending_form_of_session(i64) -> Result<Option<FormRow>>`, `forms(session_id: Option<i64>, state: Option<&str>) -> Result<Vec<FormRow>>`, `finish_form(&str, &FormFinish) -> Result<bool>`, `cancel_forms_of_session(i64) -> Result<usize>`, `expire_forms(older_than: i64) -> Result<usize>`, `purge_forms(decided_before: i64) -> Result<usize>`, `forms_to_sweep(decided_before: i64) -> Result<Vec<(String, String)>>` (form_id, host_alias), `mark_form_swept(&str) -> Result<()>`, `form_notify() -> Arc<tokio::sync::Notify>`
  - consts `FORM_STATES: [&str; 5]`

- [ ] **Step 1: Write the migration**

`crates/fleet-core/migrations/112_form_requests.sql`:

```sql
-- Chat forms (docs/superpowers/specs/2026-10-07-chat-forms-design.md): a
-- form an agent asked a person to fill in its session's chat.
-- state  'pending' | 'answered' | 'declined' | 'cancelled' | 'expired'
-- answers  JSON {"answers": {...}, "secrets": {field: path}}: never a secret's value
-- secrets_on_host  1 while a secret directory for it may exist on the host
CREATE TABLE IF NOT EXISTS form_requests (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  form_id         TEXT    NOT NULL UNIQUE,
  session_id      INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  host_alias      TEXT    NOT NULL,
  spec            TEXT    NOT NULL,
  why             TEXT,
  state           TEXT    NOT NULL DEFAULT 'pending'
                  CHECK (state IN ('pending', 'answered', 'declined', 'cancelled', 'expired')),
  answers         TEXT,
  note            TEXT,
  answered_by     TEXT,
  secrets_on_host INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  decided_at      INTEGER
);
-- One pending form per session, and the cheap lookup the session row reads.
CREATE UNIQUE INDEX IF NOT EXISTS idx_form_requests_one_pending
  ON form_requests(session_id) WHERE state = 'pending';
CREATE INDEX IF NOT EXISTS idx_form_requests_state
  ON form_requests(state, decided_at);

INSERT OR IGNORE INTO schema_version (version) VALUES (112);
```

In `store/schema.rs`, append after the 111 entry inside `MIGRATIONS`:

```rust
    // Chat forms: `form_requests`. A new table and indexes, `IF NOT EXISTS`,
    // safe to re-run.
    Migration::plain(112, include_str!("../../migrations/112_form_requests.sql")),
```

- [ ] **Step 2: Write the failing store tests**

At the bottom of the new `crates/fleet-core/src/store/forms.rs`, a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::store_with_recorder;

    fn seed(s: &Store) -> i64 {
        s.upsert_host("h").unwrap();
        s.upsert_session("dev", "h", None, None, 1, 1, "running", None).unwrap()
    }

    fn new<'a>(form_id: &'a str, session_id: i64) -> NewForm<'a> {
        NewForm {
            form_id,
            session_id,
            host_alias: "h",
            spec: r#"{"spec":"fleet.form/1","title":"Pick one","steps":[]}"#,
            why: Some("to know"),
        }
    }

    #[test]
    fn a_pending_form_shows_on_its_session_row_and_bumps_it() {
        let (s, bus) = store_with_recorder();
        let sid = seed(&s);
        let before = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(before.pending_form, None);
        bus.take();
        s.insert_form(&new("f_one", sid)).unwrap();
        let after = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(
            after.pending_form,
            Some(PendingForm { form_id: "f_one".into(), title: "Pick one".into() })
        );
        assert!(after.row_version > before.row_version, "the merge guard sees it");
        assert_eq!(bus.names(), vec!["session:updated"]);
    }

    #[test]
    fn a_second_pending_form_for_one_session_is_refused() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        let err = s.insert_form(&new("f_two", sid)).unwrap_err();
        assert!(err.to_string().contains("UNIQUE"), "{err}");
    }

    #[test]
    fn finishing_clears_the_row_once_and_wakes_waiters() {
        let (s, bus) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        let notify = s.form_notify();
        let woke = notify.notified();
        tokio::pin!(woke);
        assert!(woke.as_mut().enable(), "registered");
        bus.take();
        let done = FormFinish {
            state: "answered",
            answers: Some(r#"{"answers":{},"secrets":{}}"#),
            note: None,
            answered_by: Some("ada (desktop)"),
            secrets_on_host: false,
        };
        assert!(s.finish_form("f_one", &done).unwrap());
        assert!(!s.finish_form("f_one", &done).unwrap(), "only a pending form finishes");
        assert_eq!(s.get_session_by_id(sid).unwrap().unwrap().pending_form, None);
        assert_eq!(bus.names(), vec!["session:updated"]);
        let row = s.form("f_one").unwrap().unwrap();
        assert_eq!(row.state, "answered");
        assert!(row.decided_at.is_some());
        assert!(futures::FutureExt::now_or_never(woke).is_some(), "the waiter was woken");
    }

    #[test]
    fn expiry_purge_and_sweep_pick_the_right_rows() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_old", sid)).unwrap();
        s.conn_ref()
            .execute("UPDATE form_requests SET created_at = 10 WHERE form_id = 'f_old'", [])
            .unwrap();
        assert_eq!(s.expire_forms(100).unwrap(), 1);
        assert_eq!(s.form("f_old").unwrap().unwrap().state, "expired");
        s.conn_ref()
            .execute(
                "UPDATE form_requests SET decided_at = 50, secrets_on_host = 1 WHERE form_id = 'f_old'",
                [],
            )
            .unwrap();
        assert_eq!(s.forms_to_sweep(100).unwrap(), vec![("f_old".to_string(), "h".to_string())]);
        assert_eq!(s.purge_forms(100).unwrap(), 0, "a row with secrets on its host stays");
        s.mark_form_swept("f_old").unwrap();
        assert!(s.forms_to_sweep(100).unwrap().is_empty());
        assert_eq!(s.purge_forms(100).unwrap(), 1);
    }

    #[test]
    fn a_ghost_sessions_form_is_swept_at_once() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        s.conn_ref()
            .execute("UPDATE form_requests SET secrets_on_host = 1 WHERE form_id = 'f_one'", [])
            .unwrap();
        assert!(s.forms_to_sweep(0).unwrap().is_empty(), "a live session keeps its secrets");
        s.mark_session_killed(sid, 5).unwrap();
        assert_eq!(s.forms_to_sweep(0).unwrap().len(), 1);
    }

    #[test]
    fn a_killed_session_cancels_its_pending_form() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        assert_eq!(s.cancel_forms_of_session(sid).unwrap(), 1);
        assert_eq!(s.form("f_one").unwrap().unwrap().state, "cancelled");
        assert_eq!(s.cancel_forms_of_session(sid).unwrap(), 0);
    }
}
```

If `futures` is not a dev-dependency of fleet-core, replace the `now_or_never` assertion with `tokio::time::timeout(Duration::from_millis(10), woke).await.is_ok()` inside a `#[tokio::test]`. Check with `grep -n '^futures' crates/fleet-core/Cargo.toml`.

- [ ] **Step 3: Run them to see them fail**

Run: `cargo fleet-test -- store::forms`
Expected: compile errors (`FormRow`, `insert_form`, `pending_form` not found).

- [ ] **Step 4: Write the store module**

Top of `crates/fleet-core/src/store/forms.rs`:

```rust
//! Chat forms (`form_requests`, migration 112): a form an agent asked a
//! person to fill. Every change wakes `form_notify` (the `ask` tool's wait)
//! and bumps the asking session's `row_version` with a `session:updated`,
//! because the row carries `pending_form`.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};

pub const FORM_STATES: [&str; 5] = ["pending", "answered", "declined", "cancelled", "expired"];

#[derive(Debug, Clone, PartialEq)]
pub struct FormRow {
    pub id: i64,
    pub form_id: String,
    pub session_id: i64,
    pub host_alias: String,
    pub spec: String,
    pub why: Option<String>,
    pub state: String,
    pub answers: Option<String>,
    pub note: Option<String>,
    pub answered_by: Option<String>,
    pub secrets_on_host: bool,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

pub struct NewForm<'a> {
    pub form_id: &'a str,
    pub session_id: i64,
    pub host_alias: &'a str,
    pub spec: &'a str,
    pub why: Option<&'a str>,
}

pub struct FormFinish<'a> {
    pub state: &'a str,
    pub answers: Option<&'a str>,
    pub note: Option<&'a str>,
    pub answered_by: Option<&'a str>,
    pub secrets_on_host: bool,
}

const COLS: &str = "id, form_id, session_id, host_alias, spec, why, state, answers, note, \
                    answered_by, secrets_on_host, created_at, decided_at";

fn row(r: &rusqlite::Row<'_>) -> Result<FormRow> {
    Ok(FormRow {
        id: r.get(0)?,
        form_id: r.get(1)?,
        session_id: r.get(2)?,
        host_alias: r.get(3)?,
        spec: r.get(4)?,
        why: r.get(5)?,
        state: r.get(6)?,
        answers: r.get(7)?,
        note: r.get(8)?,
        answered_by: r.get(9)?,
        secrets_on_host: r.get::<_, i64>(10)? != 0,
        created_at: r.get(11)?,
        decided_at: r.get(12)?,
    })
}

impl Store {
    pub fn form_notify(&self) -> std::sync::Arc<tokio::sync::Notify> {
        self.form_notify.clone()
    }

    /// The asking session's row changed (its `pending_form`): bump it so
    /// the optimistic merge takes the new row, announce it, wake waiters.
    fn form_touched(&self, session_ids: &[i64]) -> Result<()> {
        for id in session_ids {
            self.conn.execute(
                "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
                [id],
            )?;
            self.emit_session(*id)?;
        }
        self.form_notify.notify_waiters();
        Ok(())
    }

    pub fn insert_form(&self, f: &NewForm<'_>) -> Result<FormRow> {
        self.conn.execute(
            "INSERT INTO form_requests (form_id, session_id, host_alias, spec, why, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![f.form_id, f.session_id, f.host_alias, f.spec, f.why, now_unix()],
        )?;
        self.form_touched(&[f.session_id])?;
        self.form(f.form_id).map(|r| r.expect("the row just inserted"))
    }

    pub fn form(&self, form_id: &str) -> Result<Option<FormRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM form_requests WHERE form_id = ?1"),
                [form_id],
                row,
            )
            .optional()
    }

    pub fn pending_form_of_session(&self, session_id: i64) -> Result<Option<FormRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM form_requests WHERE session_id = ?1 AND state = 'pending'"
                ),
                [session_id],
                row,
            )
            .optional()
    }

    /// Newest first, at most 200.
    pub fn forms(&self, session_id: Option<i64>, state: Option<&str>) -> Result<Vec<FormRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM form_requests
              WHERE (?1 IS NULL OR session_id = ?1) AND (?2 IS NULL OR state = ?2)
              ORDER BY id DESC LIMIT 200"
        ))?;
        let rows = st.query_map(rusqlite::params![session_id, state], row)?;
        rows.collect()
    }

    /// Finish a PENDING form. `false` when it was no longer pending (the
    /// caller lost a race: answered elsewhere, withdrawn, expired).
    pub fn finish_form(&self, form_id: &str, f: &FormFinish<'_>) -> Result<bool> {
        let Some(current) = self.form(form_id)? else {
            return Ok(false);
        };
        let n = self.conn.execute(
            "UPDATE form_requests
                SET state = ?2, answers = ?3, note = ?4, answered_by = ?5,
                    secrets_on_host = ?6, decided_at = ?7
              WHERE form_id = ?1 AND state = 'pending'",
            rusqlite::params![
                form_id,
                f.state,
                f.answers,
                f.note,
                f.answered_by,
                i64::from(f.secrets_on_host),
                now_unix()
            ],
        )?;
        if n == 1 {
            self.form_touched(&[current.session_id])?;
        }
        Ok(n == 1)
    }

    pub fn cancel_forms_of_session(&self, session_id: i64) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE form_requests SET state = 'cancelled', decided_at = ?2
              WHERE session_id = ?1 AND state = 'pending'",
            rusqlite::params![session_id, now_unix()],
        )?;
        if n > 0 {
            self.form_touched(&[session_id])?;
        }
        Ok(n)
    }

    /// Pending forms created before `older_than` become `expired`.
    pub fn expire_forms(&self, older_than: i64) -> Result<usize> {
        let mut st = self.conn.prepare(
            "UPDATE form_requests SET state = 'expired', decided_at = ?2
              WHERE state = 'pending' AND created_at < ?1 RETURNING session_id",
        )?;
        let ids: Vec<i64> = st
            .query_map(rusqlite::params![older_than, now_unix()], |r| r.get(0))?
            .collect::<Result<_>>()?;
        if !ids.is_empty() {
            self.form_touched(&ids)?;
        }
        Ok(ids.len())
    }

    /// Delete decided rows older than `decided_before`, except one whose
    /// secrets may still be on its host (the sweep clears that first).
    pub fn purge_forms(&self, decided_before: i64) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM form_requests
              WHERE state <> 'pending' AND decided_at < ?1 AND secrets_on_host = 0",
            [decided_before],
        )
    }

    /// `(form_id, host_alias)` of every form whose secret directory should
    /// go: its session is a ghost, or it was decided before `decided_before`.
    pub fn forms_to_sweep(&self, decided_before: i64) -> Result<Vec<(String, String)>> {
        let mut st = self.conn.prepare(
            "SELECT f.form_id, f.host_alias FROM form_requests f
               LEFT JOIN sessions s ON s.id = f.session_id
              WHERE f.secrets_on_host = 1
                AND (s.id IS NULL OR s.status = 'ghost'
                     OR (f.decided_at IS NOT NULL AND f.decided_at < ?1))
              ORDER BY f.id",
        )?;
        let rows = st.query_map([decided_before], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn mark_form_swept(&self, form_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE form_requests SET secrets_on_host = 0 WHERE form_id = ?1",
            [form_id],
        )?;
        Ok(())
    }
}
```

Check two names against the code before compiling: `emit_session` is `pub(super)` in `store/sessions.rs:1200` (reachable from a sibling store module), and `conn_ref()` exists for tests (used by `gate_fixture` in `mcp/tools/tests.rs`). SQLite's `RETURNING` needs SQLite ≥ 3.35; the bundled one is newer. If the build says otherwise, select the ids first, then update, inside `self.conn.unchecked_transaction()`.

In `store/mod.rs`:
- add `mod forms;` in the alphabetical list, and `pub use forms::{FormFinish, FormRow, NewForm, FORM_STATES};`
- below `message_notify` in `struct Store`:
```rust
    /// Signalled after a `form_requests` change, so `ask`'s wait wakes on an
    /// answer instead of polling. Not the event bus, for `message_notify`'s
    /// reasons.
    form_notify: Arc<tokio::sync::Notify>,
```
- in every place that writes `message_notify: Arc::new(tokio::sync::Notify::new()),` (find them all with `grep -rn "message_notify: Arc::new" crates/fleet-core/src`), add the same line for `form_notify`.

- [ ] **Step 5: Add `pending_form` to the session row**

In `store/rows.rs`, next to `PendingInput`'s definition, add:

```rust
/// The chat form a session's agent is waiting on (chat forms, migration
/// 112): read by a subselect on `form_requests`, null when none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingForm {
    pub form_id: String,
    pub title: String,
}
```

and on `SessionRow`, as the LAST field (after `visibility`):

```rust
    /// The form this session's agent asked and is waiting on. `serde(default)`
    /// so an older hub's row (without it) still parses.
    #[serde(default)]
    pub pending_form: Option<PendingForm>,
```

At the end of `SESSION_COLUMNS`, replace `pr_evidence, pr_checked_at, owner_person_id, visibility"` with:

```rust
     pr_evidence, pr_checked_at, owner_person_id, visibility, \
     (SELECT json_object('form_id', f.form_id, 'title', json_extract(f.spec, '$.title')) \
        FROM form_requests f WHERE f.session_id = sessions.id AND f.state = 'pending') \
       AS pending_form"
```

In `map_session_row`, after `visibility: row.get(65)?,` add:

```rust
        pending_form: row
            .get::<_, Option<String>>(66)?
            .and_then(|j| serde_json::from_str(&j).ok()),
```

(Confirm 65 is still `visibility`'s index; if main added columns, use `visibility`'s index + 1.) Re-export `PendingForm` where `PendingInput` is re-exported (`grep -n "PendingInput" crates/fleet-core/src/store/mod.rs`).

Run `cargo fleet-check`. Every struct literal of `SessionRow` it flags as missing a field (test fixtures, `sample_session`) gets `pending_form: None,`.

- [ ] **Step 6: The phone view and the contract**

In `mcp/tools/views.rs` `PHONE_SESSION_FIELDS`, add `"pending_form",` right after `"pending_input",` (the list is alphabetical). Update the pinned copies of that list in `mcp/tools/tests.rs` (`grep -n '"pending_input"' crates/fleet-core/src/mcp/tools/tests.rs`) the same way.

In `src-tauri/src/backend/tests_contract.rs`: in `sample_session()` add `pending_form: Some(fleet_core::store::PendingForm { form_id: "f_x".into(), title: "T".into() }),`; rename `a_session_rows_wire_names_are_these_exact_sixty_three` to `…_sixty_four`, add `"pending_form"` to its literal list in alphabetical position, and change `63` to `64`.

Run: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`
Expected: FAILS on purpose after writing `hub_contract.golden.json`. Read the diff: `pending_form` added to `SessionRow` and `DecidedRow`, nothing removed. Do NOT change `CONTRACT_REVISION` here (Task 5 does).

- [ ] **Step 7: Run the tests**

Run: `cargo fleet-test -- store::forms`
Expected: PASS, 6 tests.
Run: `cargo fleet-test -- schema:: views tests_contract mcp::tools::tests::phone`
Expected: PASS (`migrations_are_contiguous_from_one`, `every_migration_records_its_own_version`, the phone-view and contract tests).

- [ ] **Step 8: Lint and commit**

Run: `cargo fmt --all && cargo fleet-lint`

```bash
git add crates/fleet-core/migrations/112_form_requests.sql crates/fleet-core/src/store crates/fleet-core/src/mcp/tools/views.rs crates/fleet-core/src/mcp/tools/tests.rs src-tauri/src/backend/tests_contract.rs src-tauri/src/backend/hub_contract.golden.json
git commit -m "feat(forms): form_requests table and pending_form on the session row"
```

---

### Task 3: The forms service: open, wait, answer with secrets, decline, cancel, tick

**Files:**
- Create: `crates/fleet-core/src/service/forms.rs`
- Modify: `crates/fleet-core/src/service/mod.rs` (`pub mod forms;`, alphabetical)
- Modify: `crates/fleet-core/src/ipc_error.rs` (`codes`: `E_NOT_A_SESSION`, `E_HOST_WRITE`)
- Modify: `crates/fleet-core/src/service/attention.rs:115-128`
- Modify: `crates/fleet-core/src/service/sessions/lifecycle.rs:1356-1363` (`record_kill`)
- Modify: `crates/fleet-core/src/service/tick.rs` (after the `expire_stale_working` block, ~:230)

**Interfaces:**
- Consumes: Task 1 `forms::{parse, check_answers, FieldProblem}`; Task 2 store API.
- Produces (`fleet_core::service::forms`):
  - consts `DEFAULT_WAIT_SECS = 600`, `MAX_WAIT_SECS = 600`, `EXPIRE_SECS = 86_400`, `KEEP_SECS = 604_800`, `SECRET_DIR = "~/.cache/claude-fleet/forms"`, `SECRET_NOTE`
  - `fn wait_timeout(timeout_s: Option<u64>) -> Duration`
  - `FormResult { status: String, form_id: String, answers: Option<Value>, secrets: Option<BTreeMap<String, String>>, answered_by: Option<String>, note: Option<String> }` (Serialize, Deserialize)
  - `FormView { form_id, session_id, host_alias, title, spec: Value, why, state, answers: Option<Value>, secrets: Option<BTreeMap<String,String>>, note, answered_by, created_at, decided_at }` (Serialize, Deserialize)
  - `fn open(store: &Mutex<Store>, session_id: i64, spec: &Value, why: Option<&str>) -> Result<FormView, IpcError>`
  - `async fn wait(store: &Mutex<Store>, form_id: &str, timeout: Duration, recheck: &dyn AccessRecheck) -> Result<FormResult, IpcError>`
  - `fn cancel(store: &Mutex<Store>, form_id: &str) -> Result<FormResult, IpcError>`
  - `fn get(store: &Mutex<Store>, form_id: &str) -> Result<FormView, IpcError>`
  - `fn row(store: &Mutex<Store>, form_id: &str) -> Result<FormRow, IpcError>`
  - `fn view(row: &FormRow) -> FormView`, `fn result_of(row: &FormRow) -> FormResult`
  - `async fn answer(store: &Mutex<Store>, ssh: &dyn SshExec, form_id: &str, values: &Map<String, Value>, by: &str) -> Result<FormView, IpcError>`
  - `fn decline(store: &Mutex<Store>, form_id: &str, note: Option<&str>, by: &str) -> Result<FormView, IpcError>`
  - `fn expire_and_purge(store: &Mutex<Store>, now: i64) -> usize`
  - `async fn sweep_secret_dirs(store: &Mutex<Store>, ssh: &dyn SshExec, now: i64) -> usize`

- [ ] **Step 1: The two error codes**

In `ipc_error.rs` `pub mod codes`, next to `E_PANE_UNPROVEN`:

```rust
    /// `ask { form }` from a caller that is not a proven session: a form
    /// opens in the asking session's own chat, so the call must come from a
    /// per-host token whose `X-Fleet-Pane` matches a session row.
    pub const E_NOT_A_SESSION: &str = "E_NOT_A_SESSION";
    /// A chat form's secret could not be written to its session's host;
    /// `details.field` names it. The form stays pending.
    pub const E_HOST_WRITE: &str = "E_HOST_WRITE";
```

- [ ] **Step 2: Write the failing service tests**

At the bottom of `service/forms.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tasks::NoRecheck;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use serde_json::json;
    use std::time::Duration;

    const PASSWORD: &str = "hunter2-the-secret";

    fn spec() -> Value {
        json!({ "spec": "fleet.form/1", "title": "Deploy", "steps": [
            { "title": "Target", "fields": [
                { "name": "env", "type": "select", "label": "Env", "required": true,
                  "options": [["stg", "Staging"], ["prod", "Production"]] },
                { "name": "pw", "type": "secret", "label": "Password" } ] } ] })
    }

    fn fixture() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let sid = s.upsert_session("dev", "h", None, None, 1, 1, "running", None).unwrap();
        (Mutex::new(s), sid)
    }

    fn values(v: Value) -> Map<String, Value> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn an_invalid_spec_is_refused_with_every_problem() {
        let (st, sid) = fixture();
        let err = open(&st, sid, &json!({ "spec": "fleet.form/1", "title": "", "steps": [] }), None)
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("title must not be empty"), "{}", err.message);
        assert!(st.lock().unwrap().forms(None, None).unwrap().is_empty(), "nothing stored");
    }

    #[test]
    fn a_second_form_while_one_is_pending_is_a_conflict_naming_it() {
        let (st, sid) = fixture();
        let first = open(&st, sid, &spec(), None).unwrap();
        let err = open(&st, sid, &spec(), None).unwrap_err();
        assert_eq!(err.code, codes::E_CONFLICT);
        assert_eq!(err.details.unwrap()["form_id"], json!(first.form_id));
    }

    #[test]
    fn form_ids_are_unguessable_and_well_formed() {
        let (st, sid) = fixture();
        let a = open(&st, sid, &spec(), None).unwrap().form_id;
        assert!(a.starts_with("f_") && a.len() == 18, "{a}");
        assert!(a[2..].chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
    }

    #[tokio::test]
    async fn a_wait_that_runs_out_answers_pending_and_a_later_wait_gets_the_answer() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let r = wait(&st, &id, Duration::from_millis(30), &NoRecheck).await.unwrap();
        assert_eq!((r.status.as_str(), r.answers.is_none()), ("pending", true));

        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        let waiting = wait(&st, &id, Duration::from_secs(5), &NoRecheck);
        let answering = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            answer(&st, &fake, &id, &values(json!({ "env": "stg" })), "ada (desktop)").await
        };
        let (r, v) = tokio::join!(waiting, answering);
        let r = r.unwrap();
        v.unwrap();
        assert_eq!(r.status, "answered");
        assert_eq!(r.answers, Some(json!({ "env": "stg" })));
        assert_eq!(r.answered_by.as_deref(), Some("ada (desktop)"));
        assert!(fake.calls().is_empty(), "no secret, no host write");
        let again = wait(&st, &id, Duration::from_millis(1), &NoRecheck).await.unwrap();
        assert_eq!(again, r, "a finished form answers the same, at once, every time");
    }

    #[tokio::test]
    async fn a_secret_goes_to_the_host_over_stdin_and_only_its_path_is_kept() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        answer(&st, &fake, &id, &values(json!({ "env": "prod", "pw": PASSWORD })), "ada")
            .await
            .unwrap();
        let calls = fake.calls();
        assert!(
            calls.iter().all(|c| !c.command().contains(PASSWORD)),
            "never in argv or a script: {:?}",
            calls.iter().map(|c| c.command()).collect::<Vec<_>>()
        );
        let uploads: Vec<_> = calls.iter().filter(|c| c.stdin.is_some()).collect();
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads[0].stdin_str().as_deref(), Some(PASSWORD));
        let path = format!("/home/u/.cache/claude-fleet/forms/{id}/pw");
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert!(row.secrets_on_host);
        let stored = row.answers.unwrap();
        assert!(!stored.contains(PASSWORD), "{stored}");
        assert!(stored.contains(&path), "{stored}");
        let r = result_of(&st.lock().unwrap().form(&id).unwrap().unwrap());
        assert_eq!(r.secrets.unwrap().get("pw"), Some(&path));
        assert_eq!(r.note.as_deref(), Some(SECRET_NOTE));
    }

    #[tokio::test]
    async fn a_failed_secret_write_leaves_the_form_pending() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        fake.on(Match::Any, Reply::fail(1, "disk full"));
        let err = answer(&st, &fake, &id, &values(json!({ "env": "prod", "pw": PASSWORD })), "ada")
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_WRITE);
        assert_eq!(err.details.unwrap()["field"], json!("pw"));
        let row = st.lock().unwrap().form(&id).unwrap().unwrap();
        assert_eq!((row.state.as_str(), row.secrets_on_host), ("pending", false));
    }

    #[tokio::test]
    async fn bad_values_come_back_per_field_and_the_form_stays_pending() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let err = answer(&st, &FakeSsh::new(), &id, &values(json!({ "env": "dev" })), "ada")
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert_eq!(
            err.details.unwrap()["problems"],
            json!([{ "field": "env", "problem": "must be one of the options" }])
        );
        assert_eq!(get(&st, &id).unwrap().state, "pending");
    }

    #[tokio::test]
    async fn decline_cancel_and_a_late_answer() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        let v = decline(&st, &id, Some("not now"), "ada").unwrap();
        assert_eq!((v.state.as_str(), v.note.as_deref()), ("declined", Some("not now")));
        let late = answer(&st, &FakeSsh::new(), &id, &values(json!({ "env": "stg" })), "bob")
            .await
            .unwrap_err();
        assert_eq!(late.code, codes::E_CONFLICT);
        assert_eq!(late.details.unwrap()["state"], json!("declined"));

        let id2 = open(&st, sid, &spec(), None).unwrap().form_id;
        assert_eq!(cancel(&st, &id2).unwrap().status, "cancelled");
        assert_eq!(cancel(&st, &id2).unwrap_err().code, codes::E_CONFLICT);
        assert_eq!(get(&st, "f_nope").unwrap_err().code, codes::E_NOTFOUND);
    }

    #[tokio::test]
    async fn the_tick_expires_and_the_sweep_removes_a_ghosts_secrets() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        assert_eq!(expire_and_purge(&st, now_unix() + EXPIRE_SECS + 1), 1);
        assert_eq!(get(&st, &id).unwrap().state, "expired");
        {
            let s = st.lock().unwrap();
            s.conn_ref()
                .execute("UPDATE form_requests SET secrets_on_host = 1", [])
                .unwrap();
            s.mark_session_killed(sid, 5).unwrap();
        }
        let fake = FakeSsh::new();
        assert_eq!(sweep_secret_dirs(&st, &fake, now_unix()).await, 1);
        let script = fake.calls()[0].script().unwrap();
        assert!(script.contains("rm -rf") && script.contains(&id), "{script}");
        assert!(!st.lock().unwrap().form(&id).unwrap().unwrap().secrets_on_host);
        assert_eq!(sweep_secret_dirs(&st, &fake, now_unix()).await, 0, "done once");
    }

    #[tokio::test]
    async fn an_unreachable_host_is_swept_on_a_later_tick() {
        let (st, sid) = fixture();
        let id = open(&st, sid, &spec(), None).unwrap().form_id;
        {
            let s = st.lock().unwrap();
            s.conn_ref()
                .execute("UPDATE form_requests SET secrets_on_host = 1", [])
                .unwrap();
            s.mark_session_killed(sid, 5).unwrap();
        }
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::Unreachable);
        assert_eq!(sweep_secret_dirs(&st, &fake, now_unix()).await, 0);
        assert!(st.lock().unwrap().form(&id).unwrap().unwrap().secrets_on_host);
    }
}
```

Check the `FakeSsh` names against `crates/fleet-core/src/ssh_fake.rs` (`FakeSsh::new`, `with_home`, `on`, `Match::Any`, `Reply::fail`, `Reply::Unreachable`, `Call::{command, script, stdin, stdin_str}`); adjust the calls, not the assertions, if a signature differs (for example `Reply::fail`'s arguments).

- [ ] **Step 3: Run them to see them fail**

Run: `cargo fleet-test -- service::forms`
Expected: compile error, the module does not exist.

- [ ] **Step 4: Write the service**

`crates/fleet-core/src/service/forms.rs`:

```rust
//! Chat forms (`docs/superpowers/specs/2026-10-07-chat-forms-design.md`):
//! an agent opens a `fleet.form/1` form in its session's chat and waits; a
//! person answers or declines. Access is the caller's business (the `ask`
//! tool, the desktop commands); this module takes ids already gated.

use crate::ipc_error::{codes, lock, IpcError};
use crate::pages::forms::{self, FieldProblem};
use crate::service::tasks::AccessRecheck;
use crate::ssh::SshExec;
use crate::store::{now_unix, FormFinish, FormRow, NewForm, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

pub const DEFAULT_WAIT_SECS: u64 = 600;
pub const MAX_WAIT_SECS: u64 = 600;
/// A pending form nobody answered within this long expires.
pub const EXPIRE_SECS: i64 = 24 * 3600;
/// A decided form's row is kept this long.
pub const KEEP_SECS: i64 = 7 * 24 * 3600;
/// Where a form's secrets go on its session's host: `<dir>/<form_id>/<field>`.
pub const SECRET_DIR: &str = "~/.cache/claude-fleet/forms";
pub const SECRET_NOTE: &str = "Delete each secret file once you have used it.";
/// A waiter re-reads at least this often, so a missed wake costs latency only.
const POLL_FLOOR: Duration = Duration::from_millis(500);
const SWEEP_TIMEOUT: Duration = Duration::from_secs(30);

pub fn wait_timeout(timeout_s: Option<u64>) -> Duration {
    Duration::from_secs(timeout_s.unwrap_or(DEFAULT_WAIT_SECS).min(MAX_WAIT_SECS))
}

/// What `ask` answers the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormResult {
    pub status: String,
    pub form_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One form as a person's screen reads it (`list` / `get` / `answer` /
/// `decline`, and the desktop's commands).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormView {
    pub form_id: String,
    pub session_id: i64,
    pub host_alias: String,
    pub title: String,
    pub spec: Value,
    #[serde(default)]
    pub why: Option<String>,
    pub state: String,
    #[serde(default)]
    pub answers: Option<Value>,
    #[serde(default)]
    pub secrets: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub answered_by: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub decided_at: Option<i64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    answers: Map<String, Value>,
    secrets: BTreeMap<String, String>,
}

fn stored(row: &FormRow) -> Option<Stored> {
    row.answers.as_deref().and_then(|j| serde_json::from_str(j).ok())
}

pub fn view(row: &FormRow) -> FormView {
    let spec: Value = serde_json::from_str(&row.spec).unwrap_or(Value::Null);
    let st = stored(row);
    FormView {
        form_id: row.form_id.clone(),
        session_id: row.session_id,
        host_alias: row.host_alias.clone(),
        title: spec["title"].as_str().unwrap_or_default().to_string(),
        spec,
        why: row.why.clone(),
        state: row.state.clone(),
        answers: st.as_ref().map(|s| Value::Object(s.answers.clone())),
        secrets: st.map(|s| s.secrets),
        note: row.note.clone(),
        answered_by: row.answered_by.clone(),
        created_at: row.created_at,
        decided_at: row.decided_at,
    }
}

pub fn result_of(row: &FormRow) -> FormResult {
    let mut r = FormResult {
        status: row.state.clone(),
        form_id: row.form_id.clone(),
        answers: None,
        secrets: None,
        answered_by: None,
        note: None,
    };
    match row.state.as_str() {
        "answered" => {
            let st = stored(row).unwrap_or_default();
            if !st.secrets.is_empty() {
                r.note = Some(SECRET_NOTE.into());
                r.secrets = Some(st.secrets);
            }
            r.answers = Some(Value::Object(st.answers));
            r.answered_by = row.answered_by.clone();
        }
        "declined" => {
            r.note = row.note.clone();
            r.answered_by = row.answered_by.clone();
        }
        _ => {}
    }
    r
}

fn new_form_id() -> String {
    use rand::RngExt;
    const ABC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let tail: String = (0..16)
        .map(|_| ABC[rng.random_range(0..ABC.len())] as char)
        .collect();
    format!("f_{tail}")
}

fn not_found(form_id: &str) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("form {form_id} not found"))
}

fn not_pending(row: &FormRow) -> IpcError {
    IpcError::new(
        codes::E_CONFLICT,
        format!("form {} is {}, not pending", row.form_id, row.state),
    )
    .with_details(serde_json::json!({ "form_id": row.form_id, "state": row.state }))
}

fn bounded_text(what: &str, t: Option<&str>) -> Result<(), IpcError> {
    match t {
        Some(t) if t.chars().count() > forms::MAX_TEXT => Err(IpcError::new(
            codes::E_INVALID,
            format!("{what} is longer than {} characters", forms::MAX_TEXT),
        )),
        _ => Ok(()),
    }
}

pub fn row(store: &Mutex<Store>, form_id: &str) -> Result<FormRow, IpcError> {
    lock(store)?.form(form_id)?.ok_or_else(|| not_found(form_id))
}

pub fn get(store: &Mutex<Store>, form_id: &str) -> Result<FormView, IpcError> {
    row(store, form_id).map(|r| view(&r))
}

/// Validate `spec` and open it for `session_id` (a session the caller
/// proved it is).
pub fn open(
    store: &Mutex<Store>,
    session_id: i64,
    spec: &Value,
    why: Option<&str>,
) -> Result<FormView, IpcError> {
    bounded_text("why", why)?;
    let form = forms::parse(spec).map_err(|problems| {
        IpcError::new(codes::E_INVALID, problems.join("; "))
            .with_details(serde_json::json!({ "problems": problems }))
    })?;
    let s = lock(store)?;
    let session = s
        .get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))?;
    if let Some(open) = s.pending_form_of_session(session_id)? {
        return Err(IpcError::new(
            codes::E_CONFLICT,
            format!("this session already waits on form {}; wait on it or cancel it", open.form_id),
        )
        .with_details(serde_json::json!({ "form_id": open.form_id })));
    }
    let text = serde_json::to_string(&form).expect("a form serialises");
    let id = new_form_id();
    let row = s.insert_form(&NewForm {
        form_id: &id,
        session_id,
        host_alias: &session.host_alias,
        spec: &text,
        why,
    })?;
    Ok(view(&row))
}

/// Wait up to `timeout` for `form_id` to finish. A pending form at the
/// deadline answers `pending`, not an error.
pub async fn wait(
    store: &Mutex<Store>,
    form_id: &str,
    timeout: Duration,
    recheck: &dyn AccessRecheck,
) -> Result<FormResult, IpcError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let notify = {
        let s = lock(store)?;
        s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
        s.form_notify()
    };
    loop {
        {
            let s = lock(store)?;
            recheck.check(&s)?;
            let row = s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
            if row.state != "pending" {
                return Ok(result_of(&row));
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(FormResult {
                status: "pending".into(),
                form_id: form_id.into(),
                answers: None,
                secrets: None,
                answered_by: None,
                note: None,
            });
        }
        let _ = tokio::time::timeout(POLL_FLOOR.min(deadline - now), notify.notified()).await;
    }
}

pub fn cancel(store: &Mutex<Store>, form_id: &str) -> Result<FormResult, IpcError> {
    let s = lock(store)?;
    let row = s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
    let done = FormFinish {
        state: "cancelled",
        answers: None,
        note: None,
        answered_by: None,
        secrets_on_host: false,
    };
    if !s.finish_form(form_id, &done)? {
        return Err(not_pending(&row));
    }
    Ok(result_of(&s.form(form_id)?.ok_or_else(|| not_found(form_id))?))
}

pub fn decline(
    store: &Mutex<Store>,
    form_id: &str,
    note: Option<&str>,
    by: &str,
) -> Result<FormView, IpcError> {
    bounded_text("note", note)?;
    let s = lock(store)?;
    let row = s.form(form_id)?.ok_or_else(|| not_found(form_id))?;
    let done = FormFinish {
        state: "declined",
        answers: None,
        note,
        answered_by: Some(by),
        secrets_on_host: false,
    };
    if !s.finish_form(form_id, &done)? {
        return Err(not_pending(&row));
    }
    Ok(view(&s.form(form_id)?.ok_or_else(|| not_found(form_id))?))
}

/// The form's secret directory on its host, `~/` form (for
/// `write_host_file_secret`) and absolute (for the agent).
async fn secret_dir(ssh: &dyn SshExec, host: &str, form_id: &str) -> Result<(String, String), IpcError> {
    let tilde = format!("{SECRET_DIR}/{form_id}");
    let home = if host == crate::service::projects::LOCAL_HOST {
        std::env::var("HOME").unwrap_or_default()
    } else {
        ssh.remote_home(host).await?
    };
    let abs = format!("{home}/{}", tilde.trim_start_matches("~/"));
    Ok((tilde, abs))
}

async fn remove_secret_dir(ssh: &dyn SshExec, host: &str, form_id: &str) -> Result<(), IpcError> {
    let script = format!(
        "rm -rf -- \"$HOME\"/.cache/claude-fleet/forms/{}",
        crate::shell::quote(form_id)
    );
    let out = crate::ssh::run_shell(ssh, host, &script, SWEEP_TIMEOUT).await?;
    if out.status.success() {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_HOST_WRITE,
            format!("{host}: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ))
    }
}

/// Answer `form_id` as `by`. Secrets are written to the host first; any
/// failure leaves the form pending.
pub async fn answer(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    form_id: &str,
    values: &Map<String, Value>,
    by: &str,
) -> Result<FormView, IpcError> {
    let row = row(store, form_id)?;
    if row.state != "pending" {
        return Err(not_pending(&row));
    }
    let form: forms::FormSpec = serde_json::from_str(&row.spec)
        .map_err(|e| IpcError::new(codes::E_INTERNAL, format!("stored form {form_id}: {e}")))?;
    let answers = forms::check_answers(&form, values).map_err(|problems: Vec<FieldProblem>| {
        IpcError::new(
            codes::E_INVALID,
            problems
                .iter()
                .map(|p| format!("{}: {}", p.field, p.problem))
                .collect::<Vec<_>>()
                .join("; "),
        )
        .with_details(serde_json::json!({ "problems": problems }))
    })?;
    let mut paths = BTreeMap::new();
    if !answers.secrets.is_empty() {
        let (dir, abs) = secret_dir(ssh, &row.host_alias, form_id).await?;
        for (field, secret) in &answers.secrets {
            let path = format!("{dir}/{field}");
            if let Err(e) = crate::service::provision::write_host_file_secret(
                ssh,
                &row.host_alias,
                &dir,
                &path,
                secret,
            )
            .await
            {
                let _ = remove_secret_dir(ssh, &row.host_alias, form_id).await;
                return Err(IpcError::new(
                    codes::E_HOST_WRITE,
                    format!("{field}: {}", e.message),
                )
                .with_details(serde_json::json!({ "field": field })));
            }
            paths.insert(field.clone(), format!("{abs}/{field}"));
        }
    }
    let text = serde_json::to_string(&Stored {
        answers: answers.values,
        secrets: paths.clone(),
    })
    .expect("answers serialise");
    let done = FormFinish {
        state: "answered",
        answers: Some(&text),
        note: None,
        answered_by: Some(by),
        secrets_on_host: !paths.is_empty(),
    };
    let finished = {
        let s = lock(store)?;
        s.finish_form(form_id, &done)?
    };
    if !finished {
        if !paths.is_empty() {
            let _ = remove_secret_dir(ssh, &row.host_alias, form_id).await;
        }
        return Err(not_pending(&self::row(store, form_id)?));
    }
    get(store, form_id)
}

/// The tick: expire pending forms older than [`EXPIRE_SECS`], delete
/// decided ones older than [`KEEP_SECS`]. Returns the rows touched.
pub fn expire_and_purge(store: &Mutex<Store>, now: i64) -> usize {
    let Ok(s) = lock(store) else { return 0 };
    let expired = s.expire_forms(now - EXPIRE_SECS).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "[forms] expiry failed");
        0
    });
    let purged = s.purge_forms(now - KEEP_SECS).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "[forms] purge failed");
        0
    });
    expired + purged
}

/// The tick: remove secret directories no form needs any more. A host
/// that does not answer is tried again on a later tick.
pub async fn sweep_secret_dirs(store: &Mutex<Store>, ssh: &dyn SshExec, now: i64) -> usize {
    let due = match lock(store).and_then(|s| Ok(s.forms_to_sweep(now - KEEP_SECS)?)) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e.message, "[forms] sweep query failed");
            return 0;
        }
    };
    let mut swept = 0;
    for (form_id, host) in due {
        match remove_secret_dir(ssh, &host, &form_id).await {
            Ok(()) => {
                if let Ok(s) = lock(store) {
                    if s.mark_form_swept(&form_id).is_ok() {
                        swept += 1;
                    }
                }
            }
            Err(e) => tracing::debug!(%form_id, %host, error = %e.message, "[forms] sweep deferred"),
        }
    }
    swept
}
```

Before compiling, confirm these names in the code and adjust the call (never the behaviour): `crate::service::projects::LOCAL_HOST`, `crate::ssh::run_shell`'s signature (`ssh.rs:1270`), `crate::service::provision::write_host_file_secret` (`provision.rs:1095`, must be `pub`), `codes::E_INTERNAL`, `now_unix` re-export from `store`, and `rand::RngExt` (as `service/names.rs:15` imports it).

- [ ] **Step 5: Attention, kill, tick**

`service/attention.rs` in `needs_attention_with`, change the first branch:

```rust
    let reason = if row.claude_status.as_deref() == Some("blocked") || row.pending_form.is_some() {
        Reason::Waiting
```

and add a test in that file's `mod tests`, next to `a_blocked_session_is_waiting_and_an_ordinary_one_is_nothing` (it uses the module's own `row()` helper, a working session):

```rust
    #[test]
    fn a_session_waiting_on_a_form_needs_attention_while_working() {
        let mut r = row();
        assert_eq!(needs_attention(&r), None, "a working session needs nobody");
        r.pending_form = Some(crate::store::PendingForm { form_id: "f_x".into(), title: "T".into() });
        assert_eq!(needs_attention(&r).unwrap().reason, Reason::Waiting);
    }
```

`service/sessions/lifecycle.rs` `record_kill`, append:

```rust
    if let Err(e) = s.cancel_forms_of_session(id) {
        tracing::warn!(session_id = id, error = %e, "[forms] cancel on kill failed");
    }
```

`service/tick.rs`, after the `expire_stale_working` block inside the tick closure:

```rust
                // Chat forms: expire unanswered ones, drop old rows, and
                // remove secret files nobody needs from their hosts.
                let now = unix_now();
                let forms = service::forms::expire_and_purge(store, now);
                let swept = service::forms::sweep_secret_dirs(store, &***ssh, now).await;
                if forms + swept > 0 {
                    tracing::debug!("reconcile tick: {forms} form row(s) aged, {swept} secret dir(s) removed");
                }
```

(`ssh` there is `&Arc<SshClient>`; `&***ssh` is `&SshClient`, which implements `SshExec`. If `unix_now()` returns `u64`, cast with `as i64`.)

- [ ] **Step 6: Run the tests**

Run: `cargo fleet-test -- service::forms service::attention store::forms`
Expected: PASS.

- [ ] **Step 7: Lint and commit**

Run: `cargo fmt --all && cargo fleet-lint`

```bash
git add crates/fleet-core/src/service/forms.rs crates/fleet-core/src/service/mod.rs crates/fleet-core/src/ipc_error.rs crates/fleet-core/src/service/attention.rs crates/fleet-core/src/service/sessions/lifecycle.rs crates/fleet-core/src/service/tick.rs
git commit -m "feat(forms): the forms service: wait, answer with host secrets, decline, tick"
```

---

### Task 4: The `ask` MCP tool

**Files:**
- Modify: `crates/fleet-core/src/mcp/tools/params.rs` (new `AskParams`, `AskListFilter` after `GuideParams` ~:1022)
- Create: `crates/fleet-core/src/mcp/tools/forms.rs` (a `#[tool_router(router = forms_router, vis = "pub(super)")]` block)
- Modify: `crates/fleet-core/src/mcp/tools/mod.rs` (`mod forms;`, `+ Self::forms_router()` in `tool_router()` :351-363)
- Modify: `crates/fleet-core/src/mcp/guard.rs` (`TOOL_POLICIES` row; the mutating list in `readonly_allow_list_admits_reads_and_refuses_mutations`)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (new tests; `SESSION_REACH` / `NO_PER_ROW_GATE` if required; `BUDGET_BYTES`)
- Modify: `crates/fleet-core/src/mcp/tools/tests_sessions_isolation.rs` (only if its matrix requires `ask`)
- Modify: `docs/control-api.md`; regenerate `docs/control-api-reference.md`

**Interfaces:**
- Consumes: Task 3 `service::forms::*`.
- Produces: MCP tool `ask` with arguments exactly: `form`, `why`, `wait`, `cancel`, `list: {session_id?, state?}`, `get`, `answer`, `values`, `decline`, `note`, `timeout_s`. Results: `FormResult` for `form`/`wait`/`cancel`; `Vec<FormView>` for `list`; `FormView` for `get`/`answer`/`decline`. Task 5 routes `list_forms`, `get_form`, `answer_form`, `decline_form` here with these exact argument shapes.

- [ ] **Step 1: Params**

In `params.rs`:

```rust
#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema, Default)]
pub struct AskListFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    /// pending, answered, declined, cancelled or expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct AskParams {
    /// A fleet.form/1 form for your own session's chat (docs/forms.md).
    #[serde(default)]
    pub form: Option<serde_json::Value>,
    /// form: why you ask (≤500 chars).
    #[serde(default)]
    pub why: Option<String>,
    /// Wait again on this pending form_id.
    #[serde(default)]
    pub wait: Option<String>,
    /// Withdraw this form_id.
    #[serde(default)]
    pub cancel: Option<String>,
    #[serde(default)]
    pub list: Option<AskListFilter>,
    #[serde(default)]
    pub get: Option<String>,
    /// Answer this form_id with `values`.
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub values: Option<serde_json::Map<String, serde_json::Value>>,
    /// Decline this form_id, with an optional `note`.
    #[serde(default)]
    pub decline: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// form / wait: default and max 600.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}
```

- [ ] **Step 2: The guard row**

In `guard.rs` `TOOL_POLICIES`, next to `wait_for_reply`:

```rust
    // forms.rs — chat forms: an agent's `ask { form | wait }` is a bounded
    // wait (≤ 600 s), the rest are quick. Answering is a person's (refused to
    // host tokens in the tool).
    ToolPolicy {
        name: "ask",
        access: Access::Client,
        readonly: false,
        confirm: false,
        deadline: Deadline::LongPoll,
    },
```

Add `"ask"` to the mutating-tools list in `readonly_allow_list_admits_reads_and_refuses_mutations`.

- [ ] **Step 3: Write the failing tool tests**

In `mcp/tools/tests.rs` (near the `wait_for_reply` tests, reusing `gate_fixture`, `pane_caller`, `device_of`, `while_waiting`, `err_code`, `text_of`, `test_tools`):

```rust
fn small_form() -> serde_json::Value {
    serde_json::json!({ "spec": "fleet.form/1", "title": "Pick", "steps": [
        { "title": "One", "fields": [ { "name": "x", "type": "text", "label": "X", "required": true } ] } ] })
}

fn ask_p() -> AskParams {
    AskParams {
        form: None, why: None, wait: None, cancel: None, list: None, get: None,
        answer: None, values: None, decline: None, note: None, timeout_s: None,
    }
}

#[tokio::test]
async fn an_agent_asks_a_person_answers_and_the_agent_gets_the_answers() {
    let g = gate_fixture();
    let (a_row, ada) = (g.a_row, g.ada);
    let t = test_tools(g.store);
    let asking = t.ask(
        Extension(pane_caller(Some("%7"))),
        Parameters(AskParams { form: Some(small_form()), timeout_s: Some(30), ..ask_p() }),
    );
    let out = while_waiting(&t, asking, Duration::from_millis(50), |t| {
        let id = t.store.lock().unwrap().pending_form_of_session(a_row).unwrap().unwrap().form_id;
        let values = serde_json::from_value(serde_json::json!({ "x": "hello" })).unwrap();
        let answered = futures::executor::block_on(t.ask(
            Extension(device_of(ada, ada)),
            Parameters(AskParams { answer: Some(id), values: Some(values), ..ask_p() }),
        ));
        assert!(answered.is_ok(), "{answered:?}");
    })
    .await
    .expect("the agent's call returns");
    let body = text_of(&out.content[0]);
    assert!(body.contains("\"answered\"") && body.contains("hello"), "{body}");
}

#[tokio::test]
async fn a_caller_that_is_no_session_cannot_ask() {
    let g = gate_fixture();
    let t = test_tools(g.store);
    for caller in [Caller::master(), pane_caller(None)] {
        let err = t
            .ask(Extension(caller), Parameters(AskParams { form: Some(small_form()), ..ask_p() }))
            .await
            .unwrap_err();
        assert_eq!(err_code(&err), "E_NOT_A_SESSION");
    }
}

#[tokio::test]
async fn a_host_token_never_answers_not_even_its_own_form() {
    let g = gate_fixture();
    let a_row = g.a_row;
    let t = test_tools(g.store);
    let id = {
        let s = &t.store;
        crate::service::forms::open(s, a_row, &small_form(), None).unwrap().form_id
    };
    let values = serde_json::from_value(serde_json::json!({ "x": "y" })).unwrap();
    let err = t
        .ask(
            Extension(pane_caller(Some("%7"))),
            Parameters(AskParams { answer: Some(id.clone()), values: Some(values), ..ask_p() }),
        )
        .await
        .unwrap_err();
    assert_eq!(err_code(&err), "E_FORBIDDEN");
    let err = t
        .ask(Extension(pane_caller(Some("%7"))), Parameters(AskParams { decline: Some(id), ..ask_p() }))
        .await
        .unwrap_err();
    assert_eq!(err_code(&err), "E_FORBIDDEN");
}

#[tokio::test]
async fn someone_elses_session_form_is_not_answerable() {
    let g = gate_fixture();
    let (b_row, ada) = (g.b_row, g.ada);
    let t = test_tools(g.store);
    let id = crate::service::forms::open(&t.store, b_row, &small_form(), None).unwrap().form_id;
    let values = serde_json::from_value(serde_json::json!({ "x": "y" })).unwrap();
    let err = t
        .ask(
            Extension(device_of(ada, ada)),
            Parameters(AskParams { answer: Some(id), values: Some(values), ..ask_p() }),
        )
        .await
        .unwrap_err();
    assert!(["E_FORBIDDEN", "E_NOTFOUND"].contains(&err_code(&err).as_str()), "{err:?}");
}

#[tokio::test]
async fn exactly_one_action_per_call() {
    let g = gate_fixture();
    let t = test_tools(g.store);
    let err = t
        .ask(
            Extension(Caller::master()),
            Parameters(AskParams { get: Some("f_a".into()), decline: Some("f_a".into()), ..ask_p() }),
        )
        .await
        .unwrap_err();
    assert_eq!(err_code(&err), "E_INVALID");
}
```

If `futures::executor::block_on` is unavailable in this crate's tests, make the `while_waiting` closure spawn the answer with `tokio::spawn` on a cloned `FleetTools` instead (check `impl Clone for FleetTools`), and join it after.

- [ ] **Step 4: Run them to see them fail**

Run: `cargo fleet-test -- mcp::tools::tests::an_agent_asks`
Expected: compile error, no method `ask`.

- [ ] **Step 5: Write the tool**

`crates/fleet-core/src/mcp/tools/forms.rs`:

```rust
//! The `ask` tool: chat forms (docs/forms.md). An agent opens a form in its
//! own session's chat and waits; a person (never a host token) answers.

use super::params::{AskListFilter, AskParams};
use super::support::*;
use super::FleetTools;
use crate::ipc_error::{codes, lock};
use crate::mcp::auth::Caller;
use crate::service::forms;
use rmcp::handler::server::wrapper::{Extension, Parameters};
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};

/// Who answered, in words, for the agent and the card.
fn answered_by(caller: &Caller) -> String {
    match &caller.client {
        Some(c) => format!("{} (device)", c.name),
        None => "the control API".into(),
    }
}

#[tool_router(router = forms_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Chat forms: `form` (fleet.form/1) opens a form in YOUR \
        session's chat and waits ≤600 s for the person's answers (status \
        answered | pending | declined | cancelled | expired; on pending call \
        `wait`). `cancel` withdraws. A person's side: list, get, answer, \
        decline. Spec: docs/forms.md.")]
    pub(super) async fn ask(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<AskParams>,
    ) -> Result<CallToolResult, McpError> {
        let actions = [
            p.form.is_some(),
            p.wait.is_some(),
            p.cancel.is_some(),
            p.list.is_some(),
            p.get.is_some(),
            p.answer.is_some(),
            p.decline.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if actions != 1 {
            return Err(mcp_err(
                codes::E_INVALID,
                "say exactly one of form, wait, cancel, list, get, answer or decline",
                None,
            ));
        }
        if let Some(spec) = &p.form {
            audit("ask", "action=form");
            let scope = self.view_scope(&caller)?;
            let Some(session_id) = scope.proven_session.filter(|_| caller.host_alias.is_some())
            else {
                return Err(mcp_err(
                    codes::E_NOT_A_SESSION,
                    "a form opens in the asking session's chat: call ask from inside a fleet session (its per-host token and X-Fleet-Pane)",
                    None,
                ));
            };
            let view = forms::open(&self.store, session_id, spec, p.why.as_deref())
                .map_err(to_mcp_err)?;
            return self.wait_on(&caller, session_id, &view.form_id, p.timeout_s).await;
        }
        if let Some(id) = &p.wait {
            audit("ask", &format!("action=wait form_id={}", id.escape_debug()));
            let row = forms::row(&self.store, id).map_err(to_mcp_err)?;
            self.resolve_target_row(&caller, Some(row.session_id), None, None, Reach::Read, "the form's session")?;
            return self.wait_on(&caller, row.session_id, id, p.timeout_s).await;
        }
        if let Some(id) = &p.cancel {
            audit("ask", &format!("action=cancel form_id={}", id.escape_debug()));
            let row = forms::row(&self.store, id).map_err(to_mcp_err)?;
            let own = self.view_scope(&caller)?.proven_session == Some(row.session_id);
            if !(own || caller.is_master()) {
                return Err(mcp_err(codes::E_FORBIDDEN, "only the asking session withdraws its form", None));
            }
            return ok_json_compact(&forms::cancel(&self.store, id).map_err(to_mcp_err)?);
        }
        if let Some(filter) = &p.list {
            audit("ask", "action=list");
            return ok_json_compact(&self.visible_forms(&caller, filter)?);
        }
        if let Some(id) = &p.get {
            audit("ask", &format!("action=get form_id={}", id.escape_debug()));
            let row = forms::row(&self.store, id).map_err(to_mcp_err)?;
            self.resolve_target_row(&caller, Some(row.session_id), None, None, Reach::Read, "the form's session")?;
            return ok_json_compact(&forms::view(&row));
        }
        // answer / decline: a person's, through `drive` on the session.
        let id = p.answer.as_ref().or(p.decline.as_ref()).expect("one action");
        audit("ask", &format!("action={} form_id={}", if p.answer.is_some() { "answer" } else { "decline" }, id.escape_debug()));
        if caller.host_alias.is_some() {
            return Err(mcp_err(codes::E_FORBIDDEN, "an agent never answers a form; a person does", None));
        }
        let row = forms::row(&self.store, id).map_err(to_mcp_err)?;
        self.resolve_target_row(&caller, Some(row.session_id), None, None, Reach::Drive, "the form's session")?;
        let by = answered_by(&caller);
        let view = if p.answer.is_some() {
            let values = p.values.clone().unwrap_or_default();
            forms::answer(&self.store, &*self.ssh, id, &values, &by).await
        } else {
            forms::decline(&self.store, id, p.note.as_deref(), &by)
        }
        .map_err(to_mcp_err)?;
        ok_json_compact(&view)
    }
}

impl FleetTools {
    async fn wait_on(
        &self,
        caller: &Caller,
        session_id: i64,
        form_id: &str,
        timeout_s: Option<u64>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.long_poll_permit(caller, "ask")?;
        let recheck = SessionRecheck {
            caller,
            session_id,
            reach: Reach::Read,
            what: "the form's session",
        };
        let r = forms::wait(&self.store, form_id, forms::wait_timeout(timeout_s), &recheck)
            .await
            .map_err(to_mcp_err)?;
        self.recheck_now(&recheck)?;
        ok_json(&r)
    }

    fn visible_forms(&self, caller: &Caller, f: &AskListFilter) -> Result<Vec<forms::FormView>, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let rows = s.forms(f.session_id, f.state.as_deref()).map_err(|e| to_mcp_err(e.into()))?;
        Ok(rows
            .iter()
            .filter(|r| {
                resolve_row_and_gate(&s, caller, Some(r.session_id), None, None, Reach::Read, "the form's session")
                    .is_ok()
            })
            .map(forms::view)
            .collect())
    }
}
```

Adjust imports to whatever `messaging.rs` imports for the same helpers (`audit`, `mcp_err`, `to_mcp_err`, `ok_json`, `ok_json_compact`, `Reach`, `SessionRecheck`, `resolve_row_and_gate`); copy its `use` block and drop what is unused. `view_scope` on `FleetTools` is the wrapper in `orchestration.rs:1966`.

In `mod.rs`: `mod forms;` with the other tool modules, and `+ Self::forms_router()` at the end of the sum in `tool_router()`.

- [ ] **Step 6: Run the router-wide checks**

Run: `cargo fleet-test -- mcp::tools::tests guard`
Expected: the five new tests PASS. Then fix what the router-wide tests report, by their own instructions:
- `every_router_tool_has_exactly_one_tool_policy_row`: passes with the row from Step 2.
- `every_session_addressed_tool_declares_its_reach`: if it names `ask` (its schema carries `session_id` under `list`), add `("ask", &["Read", "Drive"]),` to `SESSION_REACH` in alphabetical position, following that table's doc comment.
- `tests_sessions_isolation.rs`: if `run_matrix` now requires `ask`, add a `call()` arm `"ask" => fx.t.ask(ext, p!()).await,` like `wait_for_reply`'s at :705, and list it in `run_matrix` the way `wait_for_reply` is at :1282.
- `the_served_definition_budget_stays_bounded`: it prints the measured bytes. Set `BUDGET_BYTES` to the measured value + 100, and add one line to its doc comment: `+<n> B: the ask tool (chat forms), <before> → <after>.`

- [ ] **Step 7: Docs**

In `docs/control-api.md`:
- add `` `ask` `` to the tool index with one line: "Chat forms: open a `fleet.form/1` form in your own session's chat and wait for a person's answers; the person's side lists, gets, answers and declines. See `docs/forms.md`.";
- in "Errors and limits" (:908), add `ask` to the sentence listing the 660 s long-poll tools, and two lines for `E_NOT_A_SESSION` and `E_HOST_WRITE` with the wording from `ipc_error.rs`.

Run: `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`
Expected: it writes `docs/control-api-reference.md` (a REGEN run may fail on purpose; read the diff: one new `ask` section). Then: `cargo fleet-test -- reference_is_current narrative_guide_names_every_tool` → PASS.

- [ ] **Step 8: Lint and commit**

Run: `cargo fmt --all && cargo fleet-lint`

```bash
git add crates/fleet-core/src/mcp docs/control-api.md docs/control-api-reference.md
git commit -m "feat(forms): the ask tool — an agent asks, a person answers"
```

---

### Task 5: Desktop commands, hub routing, the contract bump

**Files:**
- Create: `src-tauri/src/commands/forms.rs`
- Modify: `src-tauri/src/commands/mod.rs` (`pub mod forms;`, alphabetical)
- Modify: `src-tauri/src/lib.rs` (`generate_handler!`, after `commands::pages::remove_guide` ~:521)
- Modify: `src-tauri/src/backend/verdicts.rs` (rows after the guide rows ~:708)
- Modify: `src-tauri/src/backend/tests_routing.rs` (read case in `routed_read_cases_but_org_admin` ~:649, mutation cases in `routed_mutation_cases_but_the_catalog` ~:1884, `M1_REVIEWED_DESKTOP_COMMANDS` ~:4623, `SOURCES` ~:5078)
- Modify: `crates/fleet-core/src/wire_contract.rs:143`, `src-tauri/src/backend/contract.rs:133,146`, the golden's `revision`
- Regenerate: `src/lib/hub_verdicts.generated.json`, `docs/hub.md` refusal table

**Interfaces:**
- Consumes: Task 3 `forms::{FormView, get, answer, decline}`, `store.forms(..)`; Task 4 argument shapes.
- Produces Tauri commands (top-level args camelCase from TS):
  - `list_forms(session_id: Option<i64>, state: Option<String>) -> Vec<FormView>`
  - `get_form(form_id: String) -> FormView`
  - `answer_form(form_id: String, values: Map<String, Value>) -> FormView`
  - `decline_form(form_id: String, note: Option<String>) -> FormView`

- [ ] **Step 1: Write the failing routing cases**

In `tests_routing.rs`, `routed_read_cases_but_org_admin()`:

```rust
        (
            "list_forms",
            "ask",
            json!({ "list": { "session_id": 4, "state": "pending" } }),
            r#"[]"#,
            Box::new(|b, s, _| {
                block_on(commands::forms::routed::list_forms(b, s, Some(4), Some("pending".into())))
                    .map(|_| ())
            }),
        ),
        (
            "get_form",
            "ask",
            json!({ "get": "f_a" }),
            FORM_VIEW_JSON,
            Box::new(|b, s, _| block_on(commands::forms::routed::get_form(b, s, "f_a".into())).map(|_| ())),
        ),
```

In `routed_mutation_cases_but_the_catalog()`:

```rust
        (
            "answer_form",
            "ask",
            json!({ "answer": "f_a", "values": { "x": "y" } }),
            FORM_VIEW_JSON,
            Box::new(|b, s, ssh| {
                let values = serde_json::from_value(json!({ "x": "y" })).unwrap();
                block_on(commands::forms::routed::answer_form(b, s, ssh, "f_a".into(), values)).map(|_| ())
            }),
        ),
        (
            "decline_form",
            "ask",
            json!({ "decline": "f_a", "note": "later" }),
            FORM_VIEW_JSON,
            Box::new(|b, s, _| {
                block_on(commands::forms::routed::decline_form(b, s, "f_a".into(), Some("later".into())))
                    .map(|_| ())
            }),
        ),
```

and near the top of the file:

```rust
const FORM_VIEW_JSON: &str = r#"{"form_id":"f_a","session_id":4,"host_alias":"h","title":"T","spec":{},"state":"pending","created_at":1}"#;
```

Add to `M1_REVIEWED_DESKTOP_COMMANDS`, following the guide rows' shape:

```rust
    ("list_forms", "ask", "the hub filters forms to sessions this device may read"),
    ("get_form", "ask", "the hub gates the form's session with Reach::Read"),
    ("answer_form", "ask", "the hub gates the form's session with Reach::Drive and refuses host tokens"),
    ("decline_form", "ask", "the hub gates the form's session with Reach::Drive and refuses host tokens"),
```

Add `("commands/forms.rs", include_str!("../commands/forms.rs")),` to `SOURCES`.

Run: `cargo fleet-test -- backend::tests_routing`
Expected: compile error, `commands::forms` not found.

- [ ] **Step 2: Write the commands**

`src-tauri/src/commands/forms.rs`:

```rust
//! Chat forms on the desktop: the forms an agent asked, and the person's
//! answer or decline. Standalone they run here; paired, on the hub's `ask`.

use crate::backend::FleetBackend;
use fleet_core::ipc_error::{lock, IpcError};
use fleet_core::service::forms::{self, FormView};
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};
use tauri::State;

/// How an answer from this desktop reads to the agent.
const BY_DESKTOP: &str = "you (desktop)";

#[tauri::command]
pub async fn list_forms(
    session_id: Option<i64>,
    state: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<FormView>, IpcError> {
    routed::list_forms(&backend, &store, session_id, state).await
}

#[tauri::command]
pub async fn get_form(
    form_id: String,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<FormView, IpcError> {
    routed::get_form(&backend, &store, form_id).await
}

#[tauri::command]
pub async fn answer_form(
    form_id: String,
    values: Map<String, Value>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<FormView, IpcError> {
    routed::answer_form(&backend, &store, &ssh, form_id, values).await
}

#[tauri::command]
pub async fn decline_form(
    form_id: String,
    note: Option<String>,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<FormView, IpcError> {
    routed::decline_form(&backend, &store, form_id, note).await
}

pub(crate) mod routed {
    use super::*;
    use serde_json::json;

    pub async fn list_forms(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        session_id: Option<i64>,
        state: Option<String>,
    ) -> Result<Vec<FormView>, IpcError> {
        match backend.hub() {
            Some(hub) => {
                hub.route("list_forms", &json!({ "list": { "session_id": session_id, "state": state } }))
                    .await
            }
            None => Ok(lock(store)?
                .forms(session_id, state.as_deref())?
                .iter()
                .map(forms::view)
                .collect()),
        }
    }

    pub async fn get_form(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        form_id: String,
    ) -> Result<FormView, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("get_form", &json!({ "get": form_id })).await,
            None => forms::get(store, &form_id),
        }
    }

    pub async fn answer_form(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        ssh: &Arc<SshClient>,
        form_id: String,
        values: Map<String, Value>,
    ) -> Result<FormView, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("answer_form", &json!({ "answer": form_id, "values": values })).await,
            None => forms::answer(store, &**ssh, &form_id, &values, BY_DESKTOP).await,
        }
    }

    pub async fn decline_form(
        backend: &FleetBackend,
        store: &Mutex<Store>,
        form_id: String,
        note: Option<String>,
    ) -> Result<FormView, IpcError> {
        match backend.hub() {
            Some(hub) => hub.route("decline_form", &json!({ "decline": form_id, "note": note })).await,
            None => forms::decline(store, &form_id, note.as_deref(), BY_DESKTOP),
        }
    }
}
```

The `list_forms` read case expects `{"list":{"session_id":4,"state":"pending"}}`; with `None` values `json!` writes `null`, which `AskListFilter`'s `Option`s accept.

Register in `lib.rs` after `commands::pages::remove_guide,`:

```rust
            commands::forms::list_forms,
            commands::forms::get_form,
            commands::forms::answer_form,
            commands::forms::decline_form,
```

In `verdicts.rs` after the guide rows:

```rust
    ("list_forms", Verdict::Routed { tool: "ask" }),
    ("get_form", Verdict::Routed { tool: "ask" }),
    ("answer_form", Verdict::Routed { tool: "ask" }),
    ("decline_form", Verdict::Routed { tool: "ask" }),
```

- [ ] **Step 3: The contract bump**

`crates/fleet-core/src/wire_contract.rs:143`: `pub const CONTRACT_REVISION: u32 = 9;` and add a history entry in that file's revision list (find the entry for 8 and add below it):
`9 — the desktop routes list_forms / get_form / answer_form / decline_form to the hub's new ask tool (chat forms); session rows carry pending_form.`

`src-tauri/src/backend/contract.rs`: `MIN_HUB_CONTRACT: u32 = 9` (:133) and `MAX_HUB_CONTRACT: u32 = 9` (:146).

Run: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (fails on purpose after writing; the golden's `"revision"` becomes 9), then
Run: `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen` (fails on purpose after writing `src/lib/hub_verdicts.generated.json` and `docs/hub.md`'s table; read the diff: four new rows).

- [ ] **Step 4: Run the tests**

Run: `cargo fleet-test -- backend::`
Expected: PASS, including `every_command_has_a_verdict`, `every_routed_row_is_driven_by_a_case`, `every_commands_body_does_what_its_row_says`, `the_goldens_revision_matches_the_wire_contract_constant`, `verdict_gen`.
Run: `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` (the reference lists frontend commands too), read the diff, then `cargo fleet-test -- reference_is_current` → PASS.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all && cargo fleet-lint`

```bash
git add src-tauri crates/fleet-core/src/wire_contract.rs src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md
git commit -m "feat(forms): desktop commands routed to the hub's ask; contract 9"
```

---

### Task 6: The frontend form model and API

**Files:**
- Create: `src/lib/forms/forms.ts`, `src/lib/forms/form_model.ts`, `src/lib/forms/form_model.test.ts`

**Interfaces:**
- Consumes: Task 5 commands; `docs/form-examples/answers.json`.
- Produces:
  - types `FormSpec`, `FormStep`, `FormField`, `FieldType`, `FieldCondition`, `FormView`, `FieldProblem`, `Values = Record<string, unknown>`
  - `listForms(sessionId?: number, state?: string): Promise<Result<FormView[]>>`, `getForm(formId: string)`, `answerForm(formId: string, values: Values)`, `declineForm(formId: string, note?: string)`
  - `holds(c: FieldCondition | undefined, shown: Values): boolean`
  - `visibleSteps(spec: FormSpec, values: Values): FormStep[]` (each step with only its visible fields)
  - `checkAnswers(spec: FormSpec, values: Values): { ok: true; answers: Values; secrets: string[] } | { ok: false; problems: FieldProblem[] }`
  - `stepProblems(spec: FormSpec, stepIndex: number, values: Values): FieldProblem[]` (problems of one visible step's fields only, ignoring unknown names)

- [ ] **Step 1: Write the failing tests**

`src/lib/forms/form_model.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { checkAnswers, visibleSteps, stepProblems, type FormSpec } from './form_model';

const doc = JSON.parse(readFileSync(resolve(__dirname, '../../../docs/form-examples/answers.json'), 'utf8'));
const spec = doc.spec as FormSpec;

describe('form_model, against the shared cases', () => {
  for (const c of doc.cases) {
    it(c.name, () => {
      const r = checkAnswers(spec, c.values);
      if (c.problems) {
        expect(r.ok).toBe(false);
        if (!r.ok) expect(r.problems).toEqual(c.problems);
      } else {
        expect(r.ok).toBe(true);
        if (r.ok) {
          expect(r.answers).toEqual(c.answers);
          expect(r.secrets).toEqual(c.secrets);
        }
      }
    });
  }
});

describe('visibleSteps', () => {
  it('drops a step whose condition does not hold, and follows the answers', () => {
    expect(visibleSteps(spec, { db: false }).map((s) => s.title)).toEqual(['Basics']);
    expect(visibleSteps(spec, { db: true }).map((s) => s.title)).toEqual(['Basics', 'Database']);
  });
  it('drops a field whose condition does not hold', () => {
    const db = visibleSteps(spec, { db: true, engine: 'sqlite' })[1];
    expect(db.fields.map((f) => f.name)).toEqual(['engine']);
  });
});

describe('stepProblems', () => {
  it('reports only the step it is asked about', () => {
    expect(stepProblems(spec, 0, { agree: true })).toEqual([{ field: 'name', problem: 'is required' }]);
    expect(stepProblems(spec, 0, { name: 'x', agree: true })).toEqual([]);
  });
});
```

Run: `npx vitest run src/lib/forms/form_model.test.ts`
Expected: FAIL, cannot resolve `./form_model`.

- [ ] **Step 2: Write `forms.ts`**

```ts
// Chat forms: an agent's `ask` opens a fleet.form/1 form in its session's
// chat; a person answers or declines here. The backend checks everything
// again (crates/fleet-core/src/pages/forms.rs); this file only carries it.
import { invokeCmd, type Result } from '../result';

export type FieldType = 'text' | 'textarea' | 'number' | 'bool' | 'select' | 'multiselect' | 'secret';

export interface FieldCondition {
  field?: string;
  eq?: unknown;
  in?: unknown[];
  truthy?: boolean;
  all?: FieldCondition[];
  any?: FieldCondition[];
  not?: FieldCondition;
}

export interface FormField {
  name: string;
  type: FieldType;
  label: string;
  help?: string;
  required?: boolean;
  value?: unknown;
  when?: FieldCondition;
  placeholder?: string;
  max_len?: number;
  min?: number;
  max?: number;
  integer?: boolean;
  options?: [string, string][];
}

export interface FormStep {
  title: string;
  intro?: string;
  when?: FieldCondition;
  fields: FormField[];
}

export interface FormSpec {
  spec: 'fleet.form/1';
  title: string;
  intro?: string;
  submit?: string;
  steps: FormStep[];
}

export type FormState = 'pending' | 'answered' | 'declined' | 'cancelled' | 'expired';

export interface FormView {
  form_id: string;
  session_id: number;
  host_alias: string;
  title: string;
  spec: FormSpec;
  why?: string | null;
  state: FormState;
  answers?: Record<string, unknown> | null;
  secrets?: Record<string, string> | null;
  note?: string | null;
  answered_by?: string | null;
  created_at: number;
  decided_at?: number | null;
}

export interface FieldProblem {
  field: string;
  problem: string;
}

export type Values = Record<string, unknown>;

export function listForms(sessionId?: number, state?: FormState): Promise<Result<FormView[]>> {
  return invokeCmd<FormView[]>('list_forms', { sessionId: sessionId ?? null, state: state ?? null });
}

export function getForm(formId: string): Promise<Result<FormView>> {
  return invokeCmd<FormView>('get_form', { formId });
}

export function answerForm(formId: string, values: Values): Promise<Result<FormView>> {
  return invokeCmd<FormView>('answer_form', { formId, values });
}

export function declineForm(formId: string, note?: string): Promise<Result<FormView>> {
  return invokeCmd<FormView>('decline_form', { formId, note: note?.trim() ? note.trim() : null });
}
```

- [ ] **Step 3: Write `form_model.ts`**

```ts
// The TS twin of crates/fleet-core/src/pages/forms.rs `check_answers`:
// which steps and fields are asked, and what is wrong with an answer. Both
// run docs/form-examples/answers.json, so the words must match exactly.
import type { FieldCondition, FieldProblem, FormField, FormSpec, FormStep, Values } from './forms';
export type { FormSpec, FormStep, FormField, FieldProblem, Values } from './forms';

const TEXT_LEN = 500;
const TEXTAREA_LEN = 5000;
const SECRET_LEN = 2000;

function same(a: unknown, b: unknown): boolean {
  return a === b;
}

export function holds(c: FieldCondition | undefined, shown: Values): boolean {
  if (!c) return true;
  if (c.all) return c.all.every((x) => holds(x, shown));
  if (c.any) return c.any.some((x) => holds(x, shown));
  if (c.not) return !holds(c.not, shown);
  if (c.field === undefined) return true;
  const got = shown[c.field];
  const matches = (want: unknown) =>
    Array.isArray(got) ? got.some((g) => same(g, want)) : got !== undefined && same(got, want);
  if (c.eq !== undefined) return matches(c.eq);
  if (c.in !== undefined) return c.in.some(matches);
  if (c.truthy !== undefined) return (got === true) === c.truthy;
  return true;
}

function isBlank(v: unknown): boolean {
  return v === undefined || v === null || (typeof v === 'string' && v.trim() === '') || (Array.isArray(v) && v.length === 0);
}

function optionValues(f: FormField): string[] {
  return (f.options ?? []).map(([v]) => v);
}

/** `v` as an answer to `f`, normalised, or what is wrong with it. */
function checkValue(f: FormField, v: unknown): { ok: true; value: unknown } | { ok: false; problem: string } {
  switch (f.type) {
    case 'text':
    case 'textarea':
    case 'secret': {
      if (typeof v !== 'string') return { ok: false, problem: 'must be text' };
      const cap = f.type === 'text' ? (f.max_len ?? TEXT_LEN) : f.type === 'textarea' ? (f.max_len ?? TEXTAREA_LEN) : SECRET_LEN;
      if ([...v].length > cap) return { ok: false, problem: `is longer than ${cap} characters` };
      return { ok: true, value: v };
    }
    case 'number': {
      if (typeof v !== 'number' || !Number.isFinite(v)) return { ok: false, problem: 'must be a number' };
      if (f.integer && !Number.isInteger(v)) return { ok: false, problem: 'must be a whole number' };
      if (f.min !== undefined && v < f.min) return { ok: false, problem: `must be at least ${f.min}` };
      if (f.max !== undefined && v > f.max) return { ok: false, problem: `must be at most ${f.max}` };
      return { ok: true, value: v };
    }
    case 'bool':
      return typeof v === 'boolean' ? { ok: true, value: v } : { ok: false, problem: 'must be on or off' };
    case 'select':
      return typeof v === 'string' && optionValues(f).includes(v)
        ? { ok: true, value: v }
        : { ok: false, problem: 'must be one of the options' };
    case 'multiselect': {
      const bad = { ok: false as const, problem: 'must be a list of the options' };
      if (!Array.isArray(v) || !v.every((x) => typeof x === 'string')) return bad;
      const picked = new Set(v as string[]);
      if (picked.size !== v.length || ![...picked].every((p) => optionValues(f).includes(p))) return bad;
      return { ok: true, value: optionValues(f).filter((o) => picked.has(o)) };
    }
  }
}

interface Walk {
  answers: Values;
  secrets: string[];
  problems: FieldProblem[];
  steps: FormStep[];
}

function walk(spec: FormSpec, values: Values, onlyStep?: number): Walk {
  const out: Walk = { answers: {}, secrets: [], problems: [], steps: [] };
  spec.steps.forEach((step) => {
    if (!holds(step.when, out.answers)) return;
    const shown: FormField[] = [];
    const index = out.steps.length;
    for (const f of step.fields) {
      if (!holds(f.when, out.answers)) continue;
      shown.push(f);
      const report = onlyStep === undefined || onlyStep === index;
      const given = values[f.name];
      if (isBlank(given)) {
        if (f.required && report) out.problems.push({ field: f.name, problem: 'is required' });
        continue;
      }
      const r = checkValue(f, given);
      if (!r.ok) {
        if (report) out.problems.push({ field: f.name, problem: r.problem });
      } else if (r.value === false && f.required) {
        if (report) out.problems.push({ field: f.name, problem: 'is required' });
      } else if (f.type === 'secret') {
        out.secrets.push(f.name);
      } else {
        out.answers[f.name] = r.value;
      }
    }
    out.steps.push({ ...step, fields: shown });
  });
  return out;
}

export function visibleSteps(spec: FormSpec, values: Values): FormStep[] {
  return walk(spec, values).steps;
}

export function stepProblems(spec: FormSpec, stepIndex: number, values: Values): FieldProblem[] {
  return walk(spec, values, stepIndex).problems;
}

export function checkAnswers(
  spec: FormSpec,
  values: Values,
): { ok: true; answers: Values; secrets: string[] } | { ok: false; problems: FieldProblem[] } {
  const w = walk(spec, values);
  const known = new Set(spec.steps.flatMap((s) => s.fields.map((f) => f.name)));
  for (const k of Object.keys(values)) {
    if (!known.has(k)) w.problems.push({ field: k, problem: 'is not a field of this form' });
  }
  if (w.problems.length > 0) return { ok: false, problems: w.problems };
  return { ok: true, answers: w.answers, secrets: w.secrets.sort() };
}
```

The Rust side keeps secrets in a `BTreeMap` (sorted), hence `.sort()`. Number comparison: JSON numbers arrive in JS as one `number` type, so `===` matches Rust's `same`.

- [ ] **Step 4: Run the tests**

Run: `npx vitest run src/lib/forms/form_model.test.ts`
Expected: PASS (6 shared cases + 3).
Run: `npx svelte-check`
Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/forms
git commit -m "feat(forms): the frontend form model and API, on the shared cases"
```

---

### Task 7: `FormWizard` and `FormCard`

**Files:**
- Create: `src/lib/forms/FormWizard.svelte`, `src/lib/forms/FormCard.svelte`, `src/lib/forms/FormCard.test.ts`

**Interfaces:**
- Consumes: Task 6.
- Produces:
  - `FormWizard` props: `spec: FormSpec`, `busy: boolean`, `serverProblems: FieldProblem[]`, `onsubmit: (values: Values) => void`. Test ids: `form-step-title`, `form-step-count`, `form-field-<name>`, `form-problem-<name>`, `form-back`, `form-next`, `form-submit`.
  - `FormCard` props: `formId: string`, `sessionName: string`, `blocked: string | null`, `closed?: boolean`, `ondismiss?: () => void`. Test ids: `form-card`, `form-why`, `form-decline`, `form-decline-note`, `form-decline-confirm`, `form-blocked`, `form-error`, `form-outcome`.

- [ ] **Step 1: Write the failing tests**

`src/lib/forms/FormCard.test.ts`:

```ts
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FormCard from './FormCard.svelte';
import type { FormView } from './forms';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

function view(over: Partial<FormView> = {}): FormView {
  return {
    form_id: 'f_a',
    session_id: 4,
    host_alias: 'h',
    title: 'Deploy',
    why: 'to pick a target',
    state: 'pending',
    created_at: 1,
    spec: {
      spec: 'fleet.form/1',
      title: 'Deploy',
      submit: 'Go',
      steps: [
        { title: 'Target', fields: [
          { name: 'env', type: 'select', label: 'Env', required: true, options: [['stg', 'Staging'], ['prod', 'Production']] },
          { name: 'extra', type: 'bool', label: 'More options' } ] },
        { title: 'More', when: { field: 'extra', truthy: true }, fields: [
          { name: 'pw', type: 'secret', label: 'Password', required: true } ] },
      ],
    },
    ...over,
  };
}

beforeEach(() => inv.mockReset());

describe('FormCard', () => {
  it('walks the steps, keeps values on Back, and submits typed answers', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'get_form' ? view() : view({ state: 'answered' })));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    expect(await screen.findByTestId('form-why')).toHaveTextContent('to pick a target');
    expect(screen.queryByTestId('form-step-count')).toBeNull();

    const next = () => screen.getByTestId('form-next');
    expect(screen.getByTestId('form-submit')).toBeDisabled();
    await fireEvent.change(screen.getByTestId('form-field-env'), { target: { value: 'prod' } });
    await fireEvent.click(screen.getByTestId('form-field-extra'));
    expect(await screen.findByTestId('form-step-count')).toHaveTextContent('Step 1 of 2');
    await fireEvent.click(next());
    expect(screen.getByTestId('form-step-title')).toHaveTextContent('More');
    await fireEvent.click(screen.getByTestId('form-back'));
    expect((screen.getByTestId('form-field-env') as HTMLSelectElement).value).toBe('prod');
    await fireEvent.click(next());
    await fireEvent.input(screen.getByTestId('form-field-pw'), { target: { value: 'hunter2' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('answer_form', { formId: 'f_a', values: { env: 'prod', extra: true, pw: 'hunter2' } }),
    );
    await waitFor(() => expect((screen.queryByTestId('form-field-pw') as HTMLInputElement | null)?.value ?? '').toBe(''));
  });

  it('shows the server’s per-field problems and stays open', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_form') return view();
      throw { code: 'E_INVALID', message: 'env: must be one of the options', details: { problems: [{ field: 'env', problem: 'must be one of the options' }] } };
    });
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.change(await screen.findByTestId('form-field-env'), { target: { value: 'stg' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
    expect(await screen.findByTestId('form-problem-env')).toHaveTextContent('must be one of the options');
  });

  it('declines with a reason', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'get_form' ? view() : view({ state: 'declined' })));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null } });
    await fireEvent.click(await screen.findByTestId('form-decline'));
    await fireEvent.input(screen.getByTestId('form-decline-note'), { target: { value: 'not today' } });
    await fireEvent.click(screen.getByTestId('form-decline-confirm'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('decline_form', { formId: 'f_a', note: 'not today' }));
  });

  it('is read-only with the reason when this client may not answer', async () => {
    inv.mockImplementation(async () => view());
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: 'You can watch this session, not drive it.' } });
    expect(await screen.findByTestId('form-blocked')).toHaveTextContent('not drive it');
    expect(screen.getByTestId('form-field-env')).toBeDisabled();
    expect(screen.queryByTestId('form-decline')).toBeNull();
  });

  it('says how a closed form ended', async () => {
    inv.mockImplementation(async () => view({ state: 'answered', answered_by: 'phone (device)' }));
    render(FormCard, { props: { formId: 'f_a', sessionName: 'dev', blocked: null, closed: true } });
    expect(await screen.findByTestId('form-outcome')).toHaveTextContent('Deploy: answered by phone (device)');
  });
});
```

How a thrown `invoke` error reaches `Result`: `invokeCmd` turns a rejection into `{ ok: false, error }` (see `src/lib/result.ts`); the test throws the IpcError shape. If `invokeCmd` reads the error differently, throw what `FlowView.test.ts`'s failure case throws.

Run: `npx vitest run src/lib/forms/FormCard.test.ts`
Expected: FAIL, cannot resolve `./FormCard.svelte`.

- [ ] **Step 2: Write `FormWizard.svelte`**

```svelte
<script lang="ts">
  // One chat form, step by step. The field markup is FlowView's; what is
  // asked follows the answers (form_model.ts). A secret is cleared once
  // sent and never prefilled.
  import { stepProblems, visibleSteps } from './form_model';
  import type { FieldProblem, FormField, FormSpec, Values } from './forms';

  let {
    spec,
    busy = false,
    disabled = false,
    serverProblems = [],
    onsubmit,
  }: {
    spec: FormSpec;
    busy?: boolean;
    disabled?: boolean;
    serverProblems?: FieldProblem[];
    onsubmit: (values: Values) => void;
  } = $props();

  function defaults(s: FormSpec): Values {
    const out: Values = {};
    for (const step of s.steps)
      for (const f of step.fields) if (f.type !== 'secret' && f.value !== undefined) out[f.name] = f.value;
    return out;
  }

  let values = $state<Values>(defaults(spec));
  let index = $state(0);
  const steps = $derived(visibleSteps(spec, values));
  const step = $derived(steps[Math.min(index, steps.length - 1)]);
  const last = $derived(index >= steps.length - 1);
  const ready = $derived(stepProblems(spec, Math.min(index, steps.length - 1), values).length === 0);
  const problemOf = (name: string) => serverProblems.find((p) => p.field === name)?.problem ?? null;

  function set(name: string, v: unknown) {
    values = { ...values, [name]: v };
  }

  /** Only what a visible field holds is sent: a hidden step's values stay
   *  behind, as the backend would drop them anyway. */
  function submit() {
    const shown = new Set(steps.flatMap((s) => s.fields.map((f) => f.name)));
    const out: Values = {};
    for (const [k, v] of Object.entries(values)) if (shown.has(k)) out[k] = v;
    onsubmit(out);
  }

  export function clearSecrets() {
    const next = { ...values };
    for (const s of spec.steps) for (const f of s.fields) if (f.type === 'secret') delete next[f.name];
    values = next;
  }

  const off = $derived(busy || disabled);
  const str = (f: FormField) => (typeof values[f.name] === 'string' ? (values[f.name] as string) : '');
</script>

<div class="wizard">
  {#if steps.length > 1}
    <div class="count" data-testid="form-step-count">Step {index + 1} of {steps.length}</div>
  {/if}
  {#if step}
    <h6 data-testid="form-step-title">{step.title}</h6>
    {#if step.intro}<p class="intro">{step.intro}</p>{/if}
    {#each step.fields as f (f.name)}
      <div class="field">
        {#if f.type === 'bool'}
          <label class="check">
            <input
              type="checkbox"
              data-testid={`form-field-${f.name}`}
              checked={values[f.name] === true}
              disabled={off}
              onchange={(e) => set(f.name, (e.currentTarget as HTMLInputElement).checked)} />
            {f.label}
          </label>
        {:else if f.type === 'multiselect'}
          <span class="label">{f.label}</span>
          {#each f.options ?? [] as [v, l] (v)}
            <label class="check">
              <input
                type="checkbox"
                data-testid={`form-field-${f.name}-${v}`}
                checked={Array.isArray(values[f.name]) && (values[f.name] as string[]).includes(v)}
                disabled={off}
                onchange={(e) => {
                  const cur = Array.isArray(values[f.name]) ? (values[f.name] as string[]) : [];
                  set(f.name, (e.currentTarget as HTMLInputElement).checked ? [...cur, v] : cur.filter((x) => x !== v));
                }} />
              {l}
            </label>
          {/each}
        {:else}
          <label for={`form-${f.name}`}>{f.label}{f.required ? ' *' : ''}</label>
          {#if f.type === 'select'}
            <select
              id={`form-${f.name}`}
              data-testid={`form-field-${f.name}`}
              value={str(f)}
              disabled={off}
              onchange={(e) => set(f.name, (e.currentTarget as HTMLSelectElement).value || undefined)}>
              <option value="">—</option>
              {#each f.options ?? [] as [v, l] (v)}<option value={v}>{l}</option>{/each}
            </select>
          {:else if f.type === 'textarea'}
            <textarea
              id={`form-${f.name}`}
              rows="3"
              placeholder={f.placeholder ?? ''}
              data-testid={`form-field-${f.name}`}
              value={str(f)}
              disabled={off}
              oninput={(e) => set(f.name, (e.currentTarget as HTMLTextAreaElement).value)}></textarea>
          {:else if f.type === 'number'}
            <input
              id={`form-${f.name}`}
              type="number"
              min={f.min}
              max={f.max}
              step={f.integer ? 1 : 'any'}
              data-testid={`form-field-${f.name}`}
              value={typeof values[f.name] === 'number' ? values[f.name] : ''}
              disabled={off}
              oninput={(e) => {
                const raw = (e.currentTarget as HTMLInputElement).value;
                set(f.name, raw === '' ? undefined : Number(raw));
              }} />
          {:else}
            <input
              id={`form-${f.name}`}
              type={f.type === 'secret' ? 'password' : 'text'}
              autocomplete="off"
              spellcheck="false"
              placeholder={f.placeholder ?? ''}
              data-testid={`form-field-${f.name}`}
              value={str(f)}
              disabled={off}
              oninput={(e) => set(f.name, (e.currentTarget as HTMLInputElement).value)} />
          {/if}
        {/if}
        {#if f.help}<span class="help">{f.help}</span>{/if}
        {#if problemOf(f.name)}<span class="err" data-testid={`form-problem-${f.name}`}>{problemOf(f.name)}</span>{/if}
      </div>
    {/each}
  {/if}
  <div class="row">
    {#if index > 0}
      <button type="button" data-testid="form-back" disabled={busy} onclick={() => (index -= 1)}>Back</button>
    {/if}
    {#if last}
      <button type="button" class="primary" data-testid="form-submit" disabled={off || !ready} onclick={submit}>
        {spec.submit ?? 'Submit'}
      </button>
    {:else}
      <button type="button" class="primary" data-testid="form-next" disabled={off || !ready} onclick={() => (index += 1)}>Next</button>
    {/if}
  </div>
</div>

<style>
  .wizard { display: flex; flex-direction: column; gap: 0.6rem; }
  .count { font-size: 0.72rem; color: var(--fg-muted); }
  h6 { margin: 0; font-size: 0.9rem; }
  .intro { margin: 0; font-size: 0.8rem; color: var(--fg-muted); }
  .field { display: flex; flex-direction: column; gap: 0.2rem; }
  label, .label { font-size: 0.82rem; }
  .check { display: flex; gap: 0.35rem; align-items: flex-start; }
  input:not([type='checkbox']), select, textarea { font: inherit; font-size: 0.82rem; padding: 0.25rem 0.4rem; }
  .help { font-size: 0.72rem; color: var(--fg-muted); }
  .err { font-size: 0.75rem; color: var(--usage-crit); }
  .row { display: flex; gap: 0.4rem; justify-content: flex-end; }
</style>
```

- [ ] **Step 3: Write `FormCard.svelte`**

```svelte
<script lang="ts">
  // The chat form card: who asks, why, the wizard, Decline. `closed` shows
  // one line saying how a form that left the row ended (answered on the
  // phone, withdrawn, expired).
  import FormWizard from './FormWizard.svelte';
  import { answerForm, declineForm, getForm, type FieldProblem, type FormView, type Values } from './forms';

  let {
    formId,
    sessionName,
    blocked,
    closed = false,
    ondismiss,
  }: {
    formId: string;
    sessionName: string;
    blocked: string | null;
    closed?: boolean;
    ondismiss?: () => void;
  } = $props();

  let form = $state<FormView | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let problems = $state<FieldProblem[]>([]);
  let declining = $state(false);
  let note = $state('');
  let wizard: { clearSecrets: () => void } | undefined = $state();

  $effect(() => {
    const id = formId;
    form = null;
    error = null;
    void getForm(id).then((r) => {
      if (id !== formId) return;
      if (r.ok) form = r.value;
      else error = r.error.message;
    });
  });

  const OUTCOME: Record<string, string> = {
    answered: 'answered',
    declined: 'declined',
    cancelled: 'withdrawn by the agent',
    expired: 'expired unanswered',
    pending: 'still waiting',
  };
  const outcome = $derived(
    form
      ? `${form.title}: ${OUTCOME[form.state] ?? form.state}${form.answered_by && (form.state === 'answered' || form.state === 'declined') ? ` by ${form.answered_by}` : ''}`
      : '',
  );

  async function submit(values: Values) {
    busy = true;
    error = null;
    problems = [];
    const r = await answerForm(formId, values);
    busy = false;
    wizard?.clearSecrets();
    if (r.ok) {
      form = r.value;
      return;
    }
    const details = (r.error as { details?: { problems?: FieldProblem[] } }).details;
    if (details?.problems) problems = details.problems;
    else error = r.error.message;
  }

  async function decline() {
    busy = true;
    error = null;
    const r = await declineForm(formId, note);
    busy = false;
    if (r.ok) form = r.value;
    else error = r.error.message;
  }
</script>

{#if closed}
  {#if form}
    <div class="outcome" data-testid="form-outcome" role="status">
      <span>Form {outcome}</span>
      {#if ondismiss}<button type="button" class="x" aria-label="Dismiss" onclick={ondismiss}>×</button>{/if}
    </div>
  {/if}
{:else}
  <section class="card" data-testid="form-card" aria-label={`Form from ${sessionName}`}>
    {#if form}
      <header>
        <strong>{form.title}</strong>
        <span class="who">asked by {sessionName}</span>
      </header>
      {#if form.why}<p class="why" data-testid="form-why">{form.why}</p>{/if}
      {#if form.spec.intro}<p class="intro">{form.spec.intro}</p>{/if}
      {#if blocked}<p class="blocked" data-testid="form-blocked">{blocked}</p>{/if}
      {#if form.state === 'pending'}
        <FormWizard
          bind:this={wizard}
          spec={form.spec}
          {busy}
          disabled={blocked !== null}
          serverProblems={problems}
          onsubmit={submit} />
        {#if blocked === null}
          {#if declining}
            <div class="decline">
              <input
                type="text"
                placeholder="Why not (optional)"
                data-testid="form-decline-note"
                maxlength="500"
                value={note}
                oninput={(e) => (note = (e.currentTarget as HTMLInputElement).value)} />
              <button type="button" data-testid="form-decline-confirm" disabled={busy} onclick={decline}>Decline</button>
              <button type="button" disabled={busy} onclick={() => (declining = false)}>Keep</button>
            </div>
          {:else}
            <button type="button" class="link" data-testid="form-decline" disabled={busy} onclick={() => (declining = true)}>Decline</button>
          {/if}
        {/if}
      {:else}
        <p class="done" data-testid="form-outcome">Form {outcome}</p>
      {/if}
    {/if}
    {#if error}<p class="err" data-testid="form-error">{error}</p>{/if}
  </section>
{/if}

<style>
  .card { display: flex; flex-direction: column; gap: 0.5rem; padding: 0.7rem 0.8rem; border: 1px solid var(--border); border-radius: 6px; background: var(--bg-elev, var(--bg)); }
  header { display: flex; gap: 0.5rem; align-items: baseline; }
  .who { font-size: 0.75rem; color: var(--fg-muted); }
  .why, .intro { margin: 0; font-size: 0.8rem; }
  .blocked { margin: 0; font-size: 0.78rem; color: var(--fg-muted); }
  .decline { display: flex; gap: 0.4rem; }
  .decline input { flex: 1; font: inherit; font-size: 0.8rem; }
  .link { align-self: flex-start; background: none; border: none; padding: 0; color: var(--fg-muted); text-decoration: underline; cursor: pointer; font-size: 0.78rem; }
  .err { margin: 0; font-size: 0.78rem; color: var(--usage-crit); }
  .outcome, .done { display: flex; justify-content: space-between; margin: 0; font-size: 0.8rem; color: var(--fg-muted); }
  .x { background: none; border: none; cursor: pointer; color: inherit; }
</style>
```

Use the CSS variables `AnswerPrompt.svelte` uses for its card (`grep -n "var(--" src/lib/AnswerPrompt.svelte`); replace `--border` / `--bg-elev` above with those exact names if they differ.

- [ ] **Step 4: Run the tests**

Run: `npx vitest run src/lib/forms`
Expected: PASS.
Run: `npx svelte-check`
Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/forms
git commit -m "feat(forms): the form wizard and the chat card"
```

---

### Task 8: Put the card in the chat, the row, triage and the gates

**Files:**
- Modify: `src/lib/sessions.ts:167-174` (`SessionRow`)
- Modify: `src/lib/ConversationPanel.svelte` (imports :20-21, derived state near :636-650 and :1137-1159, render :2202-2205) and `src/lib/ConversationPanel.test.ts`
- Modify: `src/lib/SessionRowItem.svelte` (:913-922) and its test
- Modify: `src/lib/attention.ts:180-182` and `src/lib/attention.test.ts`
- Modify: `src/lib/share.ts` (`SESSION_TIER` :81-127), `src/lib/hub.ts` (`ROUTED_ACTIONS` :324-406)
- Modify: `src/lib/conversation.ts:611-622` (`TOOL_VERBS`) and `src/lib/conversation.test.ts`

**Interfaces:**
- Consumes: Tasks 6–7, Task 2's `pending_form` field.
- Produces: `SessionRow.pending_form?: { form_id: string; title: string } | null`; actions `answer_form`, `decline_form` in `SESSION_TIER` (`'drive'`) and `ROUTED_ACTIONS`.

- [ ] **Step 1: Write the failing tests**

`src/lib/ConversationPanel.test.ts`, next to the answer-card tests (reuse `session()`, `settle()`, `mockedConv`, `ok`, `conv`):

```ts
  it('a session waiting on a form shows the form card instead of the indicator', async () => {
    mockedConv.mockReturnValue(ok(conv()));
    render(ConversationPanel, {
      session: session({ claude_status: 'working', pending_form: { form_id: 'f_a', title: 'Deploy' } }),
      visible: true,
    });
    await settle();
    expect(screen.getByTestId('form-card')).toBeTruthy();
  });
```

If `FormCard` calls `invoke('get_form')` and the file does not mock `@tauri-apps/api/core`, add `vi.mock('./forms/forms', async () => ({ ...(await vi.importActual<typeof import('./forms/forms')>('./forms/forms')), getForm: vi.fn(async () => ({ ok: false, error: { code: 'E_NOTFOUND', message: 'x' } })) }));` at the top with the other mocks; the card still renders its frame.

`src/lib/attention.test.ts`:

```ts
  it('a session waiting on a form is waiting, even while its tool call runs', () => {
    const row = { ...base(), claude_status: 'working', pending_form: { form_id: 'f_a', title: 'T' } } as SessionRow;
    expect(classify(row, opts)).toBe('waiting');
  });
```

Use the file's own row builder and options in place of `base()` / `opts` (look at its first `classify` test).

`src/lib/conversation.test.ts`:

```ts
  it('names the fleet ask tool a Form', () => {
    expect(toolVerb('mcp__claude-fleet__ask')).toBe('Form');
  });
```

Run: `npx vitest run src/lib/ConversationPanel.test.ts src/lib/attention.test.ts src/lib/conversation.test.ts`
Expected: the three new tests FAIL.

- [ ] **Step 2: Implement**

`sessions.ts`, after `pending_input`:

```ts
  // Chat forms (migration 112): the form this session's agent asked and is
  // waiting on. Optional: an older hub sends none.
  pending_form?: { form_id: string; title: string } | null;
```

`attention.ts`:

```ts
function isWaiting(s: SessionRow): boolean {
  return s.claude_status === 'blocked' || s.pending_form != null;
}
```

`conversation.ts` `TOOL_VERBS`: add `'mcp__claude-fleet__ask': 'Form',`.

`share.ts` `SESSION_TIER`, next to `send_prompt: 'drive'`: `answer_form: 'drive', decline_form: 'drive',`.

`hub.ts` `ROUTED_ACTIONS`: add `'answer_form', 'decline_form',` in the list's order (alphabetical if it is).

`ConversationPanel.svelte`:

```ts
  import FormCard from './forms/FormCard.svelte';
```

with the derived state, near `answerView`:

```ts
  // Chat forms: the form this session's agent waits on. `closedForm` keeps
  // the last one for a line saying how it ended, once the row drops it.
  const pendingForm = $derived(viewing === null ? (session.pending_form ?? null) : null);
  let closedForm = $state<string | null>(null);
  let lastFormId: string | null = null;
  $effect(() => {
    const id = session.pending_form?.form_id ?? null;
    if (lastFormId && id === null) closedForm = lastFormId;
    if (id !== null) closedForm = null;
    lastFormId = id;
  });
  const formBlocked = $derived(
    hubActionBlocked('answer_form', $hubStatus, $hubConnection) ?? $sessionBlocked(session, 'answer_form'),
  );
```

and in the render, replace the opening `{#if answerView && writeBlocked === null}` with:

```svelte
        {#if pendingForm}
          <FormCard
            formId={pendingForm.form_id}
            sessionName={session.friendly_name ?? session.tmux_name}
            blocked={formBlocked} />
        {:else if answerView && writeBlocked === null}
```

and, right after the whole `{#if viewing === null} … {/if}` indicator block closes, add:

```svelte
        {#if closedForm && !pendingForm}
          <FormCard formId={closedForm} sessionName={session.tmux_name} blocked={null} closed ondismiss={() => (closedForm = null)} />
        {/if}
```

`SessionRowItem.svelte`, before `{#if answerView && promptBlocked === null}`:

```svelte
        {#if sess.pending_form}
          <!-- Chat forms: the agent waits on a form; the row's own click opens the conversation. -->
          <span class="form-chip" data-testid="row-form-chip" title={sess.pending_form.title}>Form waiting</span>
        {/if}
```

with a style next to the row's other chips (copy the class of the nearest existing chip and name it `.form-chip`). Add a test to `SessionRowItem.test.ts` that renders a row with `pending_form` and finds `row-form-chip`.

- [ ] **Step 3: Run the frontend suite**

Run: `npx vitest run src/lib/ConversationPanel.test.ts src/lib/SessionRowItem.test.ts src/lib/attention.test.ts src/lib/conversation.test.ts src/lib/share_sweep.test.ts src/lib/hub_verdicts.test.ts src/lib/forms`
Expected: PASS. If `share_sweep.test.ts` or `hub_verdicts.test.ts` names a missing entry, add it where the message says.
Run: `npx svelte-check`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add src/lib
git commit -m "feat(forms): the form card in the chat, the row chip, waiting triage"
```

---

### Task 9: Documentation, the control skill, status, and the hub e2e scenario

**Files:**
- Create: `docs/forms.md`
- Modify: `skills/claude-fleet-control/SKILL.md`, `docs/status.md`, `scripts/hub-e2e.sh` (Agent leg, before the rotation block ~:849)

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: `docs/forms.md`**

Write it for people and agents, in the style of `docs/pages.md`:

```markdown
# Chat forms

An agent in a fleet session can ask the person a form instead of a
question in the terminal. The form appears as a card in the session's
Conversation panel on the desktop (and on a paired desktop or the phone
through the hub). The person fills it in step by step; the answers come
back as the result of the agent's tool call. The design is
`docs/superpowers/specs/2026-10-07-chat-forms-design.md`.

## Ask

    ask { form: <fleet.form/1>, why: "one sentence", timeout_s: 600 }

- Only from inside a fleet session: the per-host token and `X-Fleet-Pane`
  prove which session asks; anything else is `E_NOT_A_SESSION`.
- One open form per session (`E_CONFLICT` names the open one).
- The call waits up to 600 s. `{status: "pending", form_id}` means the
  person has not answered yet: call `ask { wait: form_id }` again.
- `ask { cancel: form_id }` withdraws it.

Results: `answered` (with `answers`, `secrets`, `answered_by`), `pending`,
`declined` (with the person's `note`), `cancelled`, `expired` (24 h).

## The format

The JSON Schema is `docs/form-spec.schema.json`; the validator is
`crates/fleet-core/src/pages/forms.rs`.

## Secrets

A `secret` field's value never reaches the agent or any log. It is written
to `~/.cache/claude-fleet/forms/<form_id>/<field>` on the session's host
(0600); the result's `secrets` maps the field to that path. Read it, use
it, delete it. Fleet removes the directory once the session is gone or a
week after the answer.

## Who answers

A person with `drive` on the session: the master token, the owner's
device, a `drive` grantee. Never a per-host token: an agent cannot fill a
form. On the desktop: the card in the Conversation panel; the commands are
`list_forms`, `get_form`, `answer_form`, `decline_form`, routed to the
hub's `ask` when paired.

## When to use it instead of AskUserQuestion

When the person may not be at the terminal, or the input is several
fields or steps. A single yes/no in front of the terminal stays a
question.
```

Then, under "The format", after its two lines, paste sections 2.1–2.4 of `docs/superpowers/specs/2026-10-07-chat-forms-design.md` verbatim (their `###` headings become `###` here too), and the spec's example JSON from §2 above them.

- [ ] **Step 2: The control skill**

In `skills/claude-fleet-control/SKILL.md`, in the section about talking to the person (find where `wait_for_reply` or messaging is described), add:

```markdown
### Asking the person a form

`ask { form, why }` opens a `fleet.form/1` form in YOUR session's chat and
waits ≤ 600 s for the answers. Use it instead of AskUserQuestion when the
person may not be at the terminal or the input has several fields or
steps. `pending` → call `ask { wait: form_id }` again. A secret field
comes back as a file path: use the file, then delete it. Format and
limits: `docs/forms.md`.
```

`skills/*/SKILL.md` files embedded in fleet-core recompile it when edited (CLAUDE.md); run `cargo fleet-fast-check` after.

- [ ] **Step 3: `docs/status.md`**

Add a line in the landed features list: `Chat forms, part 1 (spec 2026-10-07-chat-forms-design.md): the ask tool, fleet.form/1, the form card in the Conversation panel, secrets to host files. Contract revision 9: the desktop and its hub ship together. Part 2 (forms in guide steps) is not started; the fleet-mobile card is its own plan.`

- [ ] **Step 4: The hub e2e scenario**

In `scripts/hub-e2e.sh`, inside the Agent leg after the sessions on the agent's tmux exist (after `S1=$(sid_of agt1)` and its capture check), before the rotation block:

```bash
echo "== Chat forms (ask)"
PANE1=$(aenv tmux display-message -p -t agt1 '#{pane_id}')
ask_rpc() {  # like rpc, with the pane header an agent's MCP entry sends
  curl -s -m 70 -X POST "http://127.0.0.1:$PC/mcp" -H "Host: $PUB" -H "Authorization: Bearer $ATOK" \
    -H "X-Fleet-Pane: $PANE1" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"ask\",\"arguments\":$1}}"
}
FORM='{"spec":"fleet.form/1","title":"E2E","steps":[{"title":"One","fields":[{"name":"color","type":"select","label":"Color","required":true,"options":[["red","Red"],["blue","Blue"]]},{"name":"pw","type":"secret","label":"Password"}]}]}'
ASK_OUT="$ROOT/ask.out"
( ask_rpc "{\"form\":$FORM,\"timeout_s\":60}" > "$ASK_OUT" ) &
ASK_PID=$!
FID=""
for _ in $(seq 1 50); do
  FID=$(tool "$PC" "$PUB" "$TOKC" ask '{"list":{"state":"pending"}}' | grep -o 'f_[0-9A-Za-z]\{16\}' | head -1)
  [ -n "$FID" ] && break
  python3 -c 'import time; time.sleep(0.2)'
done
check "an agent's ask opens a pending form" '[ -n "$FID" ]' "no pending form listed"
deny=$(ask_rpc "{\"answer\":\"$FID\",\"values\":{\"color\":\"red\"}}")
check "a host token cannot answer a form" 'echo "$deny" | grep -q E_FORBIDDEN' "$deny"
ans=$(tool "$PC" "$PUB" "$TOKC" ask "{\"answer\":\"$FID\",\"values\":{\"color\":\"blue\",\"pw\":\"e2e-secret\"}}")
check "the master answers it" 'echo "$ans" | grep -q "\"answered\""' "$ans"
wait "$ASK_PID"
check "the agent's call returns the answers" 'grep -q "blue" "$ASK_OUT" && grep -q "\"answered\"" "$ASK_OUT"' "$(cat "$ASK_OUT")"
check "the secret is not in the agent's result" '! grep -q "e2e-secret" "$ASK_OUT"' "$(cat "$ASK_OUT")"
SECRET="$AHOME/.cache/claude-fleet/forms/$FID/pw"
check "the secret file is on the host, 0600" '[ "$(filemode "$SECRET")" = 600 ] && [ "$(cat "$SECRET")" = e2e-secret ]' "$(ls -l "$SECRET" 2>&1)"
```

Variable names (`aenv`, `tool`, `check`, `filemode`, `PC`, `PUB`, `TOKC`, `ATOK`, `AHOME`, `ROOT`) are the script's own (sections at :111-144, :686-760); confirm each before use. The agent's sessions run in its own tmux server (`aenv tmux`), so `display-message` must go through `aenv`.

Run: `scripts/ci-local.sh --hub-e2e` (macOS: `PATH=/opt/homebrew/bin:$PATH`, Homebrew bash, see the hub-e2e memory note)
Expected: the six new `PASS` lines; no new `FAIL`.

- [ ] **Step 5: Full verification**

Run: `cargo fleet-fast-check && scripts/verify.sh full`
Expected: green. Then `npx vitest run` (the whole frontend) → green.

- [ ] **Step 6: Commit**

```bash
git add docs/forms.md skills/claude-fleet-control/SKILL.md docs/status.md scripts/hub-e2e.sh
git commit -m "docs(forms): the chat forms guide, the control skill, status; hub e2e scenario"
```

---

## Release note

Contract revision 9 makes this desktop refuse an older hub and an older desktop refuse this hub. Release both together (`scripts/release.sh`), then upgrade the NAS hub (`fleet-hub-deploy-nas` memory) right after. The fleet-mobile card is a separate plan in that repo; until it ships, the phone sees `pending_form` and `needs_attention: waiting` on the row but cannot open the form.
