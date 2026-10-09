//! `fleet.ui/1`: a block an agent writes in its reply that the Conversation
//! view draws as a card (`docs/chat-blocks.md`). The desktop reads blocks
//! out of the transcript in `src/lib/rich_blocks.ts`; this module is the
//! same check in Rust, meant for what fleet itself writes or relays
//! (Control's cards, a phone) but not yet called by any production path,
//! and the model `docs/chat-block.schema.json` is generated from
//! (`REGEN_FORM_DOCS=1`). Both checks run
//! `docs/chat-block-examples/blocks.json` and must report the same
//! problems, word for word.
//!
//! Unknown keys are ignored (a newer writer may add one); every string is
//! capped; a block is at most [`MAX_BLOCK_BYTES`].

use super::forms::FormSpec;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const UI_SPEC: &str = "fleet.ui/1";
/// The largest block JSON drawn as a card (`UI_MAX_BYTES` in rich_blocks.ts).
pub const MAX_BLOCK_BYTES: usize = 32 * 1024;
const MAX_PROBLEMS: usize = 20;
const KEY_MAX: usize = 64;

pub const KINDS: &[&str] = &[
    "report", "steps", "guide", "callout", "facts", "choices", "form", "progress", "results",
    "error", "setting", "wizard",
];
/// The app's wizards a `wizard` block may open in the chat (redesign 10.12):
/// the ones in `src/lib/forms/wizards/` whose last button runs in the chat
/// (`CHAT_WIZARD_IDS` in wizards.ts). `link_hub` stays in Settings › Hub.
pub const CHAT_WIZARDS: &[&str] = &[
    "add_host",
    "add_project",
    "get_started",
    "new_session",
    "pair_device",
];
const TONES: &[&str] = &["info", "tip", "success", "warning", "danger"];
pub const PROGRESS_STATES: &[&str] = &["running", "waiting", "done", "failed"];
pub const PROGRESS_STEP_STATES: &[&str] = &["pending", "running", "done", "failed", "skipped"];
/// The page data sources' column types (`ColType` in pages.ts).
pub const RESULT_TYPES: &[&str] = &["text", "int", "tokens", "usd_micros", "day", "time"];
pub const RESULT_CHARTS: &[&str] = &["line", "bar", "sparkline"];
const RESULT_ITEM_TYPES: &[&str] = &["stat", "chart", "table"];
const FIELD_TYPES_SHOWN: &[&str] = &[
    "text",
    "textarea",
    "number",
    "bool",
    "select",
    "multiselect",
];

// ── The model (for the schema) ─────────────────────────────────────────────

/// One `fleet.ui/1` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ChatBlock {
    /// Always "fleet.ui/1".
    pub spec: String,
    #[serde(flatten)]
    pub block: BlockKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockKind {
    /// A run's result, read the way a run's report is read.
    Report {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default)]
        summary: String,
        /// done, partial, blocked or failed; anything else reads as partial.
        #[serde(default)]
        outcome: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tests_run: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blockers: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        followups: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<Value>,
    },
    /// A tutorial the person ticks off.
    Steps {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intro: Option<String>,
        steps: Vec<TutorialStep>,
    },
    /// Reference material in folding sections, or with `page` a guide
    /// fleet already has (a `fleet.page/1` page of layout `guide`), drawn
    /// as Settings draws it; the other keys are then ignored.
    Guide {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page: Option<String>,
        /// Required without `page`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intro: Option<String>,
        /// Required without `page`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sections: Vec<GuideSection>,
    },
    Callout {
        /// info (default), tip, success, warning or danger.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tone: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        body: String,
    },
    Facts {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        items: Vec<(String, FactValue)>,
    },
    /// Buttons that fill the composer.
    Choices {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<String>,
        options: Vec<Choice>,
    },
    /// A `fleet.form/1` whose answers fill the composer. No `secret` fields.
    Form { form: FormSpec },
    /// A long job's state. Blocks with the same `id` in one conversation are
    /// one card that updates in place.
    Progress {
        id: String,
        title: String,
        /// running (default), waiting (on a person), done or failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        done: Option<u64>,
        /// Absent when the size is not known yet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<Vec<ProgressStep>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Numbers, a chart and a table, drawn with the page widgets.
    Results {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        items: Vec<ResultItem>,
    },
    /// What failed, its code, and what the person can do next.
    Error {
        code: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        /// A log excerpt, drawn as code under a fold.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        next: Vec<Choice>,
    },
    /// A settings change waiting for a person: the id `set_setting` with
    /// `propose: true` answered. The card reads the key and both values
    /// from the proposal itself, never from the block, and applies it only
    /// after a confirm.
    Setting {
        proposal: u64,
        /// What the change does, in words.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// One of the app's own wizards, opened in the chat as a form (redesign
    /// 10.12): the spec is the app's (`src/lib/forms/wizards/<id>.json`),
    /// never the block's, and nothing runs until the person presses its last
    /// step's button.
    Wizard {
        wizard: ChatWizard,
        /// Why the agent opens it, in one sentence.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
}

/// A wizard a `wizard` block opens ([`CHAT_WIZARDS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ChatWizard {
    AddHost,
    AddProject,
    GetStarted,
    NewSession,
    PairDevice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct TutorialStep {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GuideSection {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(untagged)]
pub enum FactValue {
    Text(String),
    Number(f64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Choice {
    pub label: String,
    /// What a click puts in the composer, as the person's instruction.
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProgressStep {
    pub title: String,
    /// pending (default), running, done, failed or skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ResultAxis {
    pub label: String,
    /// text, int, tokens, usd_micros, day or time; as a page column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(untagged)]
pub enum StatValue {
    Number(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(untagged)]
pub enum PointX {
    Text(String),
    Number(f64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResultItem {
    Stat {
        label: String,
        value: StatValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ty: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    /// One series: line, bar or sparkline.
    Chart {
        chart: String,
        title: String,
        x: ResultAxis,
        y: ResultAxis,
        points: Vec<(PointX, f64)>,
    },
    Table {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        columns: Vec<ResultAxis>,
        /// One cell per column: text, a number, a bool or null.
        rows: Vec<Vec<Value>>,
    },
}

// ── The check ──────────────────────────────────────────────────────────────

/// A block from its JSON text: the size first, then [`check`].
pub fn check_text(raw: &str) -> Result<(), Vec<String>> {
    if raw.len() > MAX_BLOCK_BYTES {
        return Err(vec![format!(
            "is larger than {} KiB",
            MAX_BLOCK_BYTES / 1024
        )]);
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(v) => check(&v),
        Err(_) => Err(vec!["is not valid JSON".into()]),
    }
}

/// Every problem with a block (at most 20), worded and ordered as
/// `rich_blocks.ts` words them. A block that passes deserialises into
/// [`ChatBlock`] except for a `report`, which is read as loosely as a run's
/// report (a single string for a list, say).
pub fn check(v: &Value) -> Result<(), Vec<String>> {
    let Some(o) = v.as_object() else {
        return Err(vec!["must be a JSON object".into()]);
    };
    let mut p = Problems::default();
    if o.get("spec").and_then(Value::as_str) != Some(UI_SPEC) {
        p.add("", format!("`spec` must be \"{UI_SPEC}\""));
    }
    let kind = o
        .get("kind")
        .and_then(Value::as_str)
        .filter(|k| KINDS.contains(k));
    let Some(kind) = kind else {
        p.add("", format!("`kind` must be one of {}", KINDS.join(", ")));
        return Err(p.0);
    };
    match kind {
        "report" => {
            p.str(o, "title", "", false, 120);
        }
        "steps" => {
            p.str(o, "title", "", true, 120);
            p.str(o, "intro", "", false, 2000);
            let steps = p.arr(o, "steps", "", 1, 30);
            p.each(&steps, "step", |p, s, at| {
                p.str(s, "title", at, true, 200);
                p.str(s, "body", at, false, 4000);
                p.str(s, "code", at, false, 8000);
                p.str(s, "lang", at, false, 20);
            });
        }
        "guide" if !matches!(o.get("page"), None | Some(Value::Null)) => {
            p.key(o, "page", "");
        }
        "guide" => {
            p.str(o, "title", "", true, 120);
            p.str(o, "intro", "", false, 2000);
            let sections = p.arr(o, "sections", "", 1, 20);
            p.each(&sections, "section", |p, s, at| {
                p.str(s, "title", at, true, 200);
                p.str(s, "body", at, true, 8000);
            });
        }
        "callout" => {
            match o.get("tone") {
                None | Some(Value::Null) => {}
                Some(Value::String(t)) if TONES.contains(&t.as_str()) => {}
                Some(_) => p.add("", format!("`tone` must be one of {}", TONES.join(", "))),
            }
            p.str(o, "title", "", false, 120);
            p.str(o, "body", "", true, 4000);
        }
        "facts" => {
            let items = p.arr(o, "items", "", 1, 40);
            for (k, x) in items.iter().enumerate() {
                let pair = x.as_array().filter(|a| {
                    a.len() == 2
                        && a[0].is_string()
                        && (a[1].is_string() || a[1].is_number() || a[1].is_boolean())
                });
                if pair.is_none() {
                    p.add(&format!("item {}", k + 1), "must be [label, value]");
                }
            }
            p.str(o, "title", "", false, 120);
        }
        "choices" => {
            let options = p.arr(o, "options", "", 1, 8);
            p.each(&options, "option", choice);
            p.str(o, "title", "", false, 120);
            p.str(o, "question", "", false, 500);
        }
        "form" => form(&mut p, o.get("form")),
        "progress" => {
            p.key(o, "id", "");
            p.str(o, "title", "", true, 120);
            p.one_of(o, "state", "", PROGRESS_STATES, false);
            let done = p.num(o, "done", "", 0);
            let total = p.num(o, "total", "", 1);
            if let (Some(d), Some(t)) = (done, total) {
                if d > t {
                    p.add("", "`done` is more than `total`");
                }
            }
            if let Some(steps) = p.opt_arr(o, "steps", "", 1, 20) {
                p.each(&steps, "step", |p, s, at| {
                    p.str(s, "title", at, true, 200);
                    p.one_of(s, "state", at, PROGRESS_STEP_STATES, false);
                });
            }
            p.str(o, "unit", "", false, 20);
            p.str(o, "note", "", false, 2000);
        }
        "results" => {
            p.str(o, "title", "", false, 120);
            p.str(o, "summary", "", false, 2000);
            let items = p.arr(o, "items", "", 1, 12);
            p.each(&items, "item", result_item);
        }
        "error" => {
            p.key(o, "code", "");
            p.str(o, "title", "", true, 120);
            let next = p.opt_arr(o, "next", "", 1, 4).unwrap_or_default();
            p.each(&next, "next", choice);
            p.str(o, "body", "", false, 4000);
            p.str(o, "detail", "", false, 8000);
        }
        "setting" => {
            if matches!(o.get("proposal"), None | Some(Value::Null)) {
                p.add("", "`proposal` is required");
            } else {
                p.num(o, "proposal", "", 1);
            }
            p.str(o, "note", "", false, 500);
        }
        "wizard" => {
            p.one_of(o, "wizard", "", CHAT_WIZARDS, true);
            p.str(o, "why", "", false, 500);
        }
        _ => unreachable!("kind is one of KINDS"),
    }
    if p.0.is_empty() {
        Ok(())
    } else {
        Err(p.0)
    }
}

fn choice(p: &mut Problems, o: &Map<String, Value>, at: &str) {
    p.str(o, "label", at, true, 80);
    p.str(o, "prompt", at, true, 4000);
    p.str(o, "hint", at, false, 200);
}

fn axis(p: &mut Problems, v: Option<&Value>, at: &str) {
    let Some(o) = v.and_then(Value::as_object) else {
        p.add(at, "must be an object");
        return;
    };
    p.str(o, "label", at, true, 80);
    p.one_of(o, "ty", at, RESULT_TYPES, false);
}

fn finite(v: &Value) -> bool {
    v.as_f64().is_some_and(f64::is_finite)
}

fn result_item(p: &mut Problems, o: &Map<String, Value>, at: &str) {
    match o.get("type").and_then(Value::as_str) {
        Some("stat") => {
            p.str(o, "label", at, true, 80);
            let ok = match o.get("value") {
                Some(Value::Number(_)) => true,
                Some(Value::String(s)) => s.chars().count() <= 80,
                _ => false,
            };
            if !ok {
                p.add(
                    at,
                    "`value` must be a number or text of at most 80 characters",
                );
            }
            p.one_of(o, "ty", at, RESULT_TYPES, false);
            p.str(o, "hint", at, false, 200);
        }
        Some("chart") => {
            p.one_of(o, "chart", at, RESULT_CHARTS, true);
            p.str(o, "title", at, true, 120);
            axis(p, o.get("x"), &format!("{at} › x"));
            axis(p, o.get("y"), &format!("{at} › y"));
            let points = p.arr(o, "points", at, 1, 200);
            for (k, pt) in points.iter().enumerate() {
                let ok = pt.as_array().is_some_and(|a| {
                    a.len() == 2 && (a[0].is_string() || finite(&a[0])) && finite(&a[1])
                });
                if !ok {
                    p.add(&format!("{at} › point {}", k + 1), "must be [x, number]");
                }
            }
        }
        Some("table") => {
            p.str(o, "title", at, false, 120);
            let columns = p.arr(o, "columns", at, 1, 12);
            for (k, c) in columns.iter().enumerate() {
                axis(p, Some(c), &format!("{at} › column {}", k + 1));
            }
            let rows = p.arr(o, "rows", at, 0, 200);
            for (k, r) in rows.iter().enumerate() {
                let rat = format!("{at} › row {}", k + 1);
                let Some(cells) = r.as_array() else {
                    p.add(&rat, "must be a list");
                    continue;
                };
                if cells.len() != columns.len() {
                    p.add(
                        &rat,
                        format!("has {} cells for {} columns", cells.len(), columns.len()),
                    );
                }
                for (j, c) in cells.iter().enumerate() {
                    if !(c.is_null() || c.is_string() || c.is_boolean() || c.is_number()) {
                        p.add(
                            &rat,
                            format!("cell {} must be text, a number, a bool or null", j + 1),
                        );
                    }
                }
            }
        }
        _ => p.add(
            at,
            format!("`type` must be one of {}", RESULT_ITEM_TYPES.join(", ")),
        ),
    }
}

/// Enough of `fleet.form/1` for the card to draw it (`checkForm` in
/// rich_blocks.ts): looser than [`super::forms::parse`], which is for `ask`.
/// A secret is refused: its answer would land in the transcript.
fn form(p: &mut Problems, v: Option<&Value>) {
    let where_ = "form";
    let Some(o) = v.and_then(Value::as_object) else {
        p.add(where_, "must be a fleet.form/1 object");
        return;
    };
    if o.get("spec").and_then(Value::as_str) != Some("fleet.form/1") {
        p.add(where_, "`spec` must be \"fleet.form/1\"");
    }
    p.str(o, "title", where_, true, 120);
    p.str(o, "intro", where_, false, 500);
    p.str(o, "submit", where_, false, 40);
    let mut names = std::collections::BTreeSet::new();
    let mut fields = 0usize;
    let steps = p.arr(o, "steps", where_, 1, 12);
    p.each(&steps, "form › step", |p, s, at| {
        p.str(s, "title", at, true, 120);
        p.str(s, "intro", at, false, 500);
        let fs = p.arr(s, "fields", at, 1, 40);
        p.each(&fs, &format!("{at} › field"), |p, f, fat| {
            fields += 1;
            if let Some(name) = p.str(f, "name", fat, true, 40) {
                if !valid_name(name) {
                    p.add(
                        fat,
                        format!("name \"{name}\" must be lowercase letters, digits and _"),
                    );
                }
                if !names.insert(name.to_string()) {
                    p.add(fat, format!("name \"{name}\" appears twice"));
                }
            }
            p.str(f, "label", fat, true, 200);
            p.str(f, "help", fat, false, 500);
            p.str(f, "placeholder", fat, false, 200);
            let ty = f.get("type");
            match ty.and_then(Value::as_str) {
                Some("secret") => p.add(
                    fat,
                    "a secret field is only for `ask`: its answer would land in the transcript",
                ),
                Some(t) if FIELD_TYPES_SHOWN.contains(&t) => {}
                _ => p.add(fat, format!("type {} is not a field type", js(ty))),
            }
            if matches!(
                ty.and_then(Value::as_str),
                Some("select") | Some("multiselect")
            ) {
                let opts = p.arr(f, "options", fat, 1, 50);
                for (k, opt) in opts.iter().enumerate() {
                    let ok = opt
                        .as_array()
                        .is_some_and(|a| a.len() == 2 && a[0].is_string() && a[1].is_string());
                    if !ok {
                        p.add(
                            &format!("{fat} › option {}", k + 1),
                            "must be [value, label]",
                        );
                    }
                }
            }
            for k in ["min", "max", "max_len"] {
                if f.get(k).is_some_and(|x| !finite(x)) {
                    p.add(fat, format!("`{k}` must be a number"));
                }
            }
        });
    });
    if fields > 40 {
        p.add(where_, "has more than 40 fields");
    }
}

/// `[a-z][a-z0-9_]{0,39}`.
fn valid_name(n: &str) -> bool {
    let mut cs = n.chars();
    cs.next().is_some_and(|c| c.is_ascii_lowercase())
        && n.len() <= 40
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A value as `JSON.stringify` writes it in a problem.
fn js(v: Option<&Value>) -> String {
    v.map_or_else(|| "undefined".into(), Value::to_string)
}

#[derive(Default)]
struct Problems(Vec<String>);

impl Problems {
    fn add(&mut self, at: &str, what: impl AsRef<str>) {
        if self.0.len() < MAX_PROBLEMS {
            let what = what.as_ref();
            self.0.push(if at.is_empty() {
                what.to_string()
            } else {
                format!("{at}: {what}")
            });
        }
    }

    fn str<'a>(
        &mut self,
        o: &'a Map<String, Value>,
        key: &str,
        at: &str,
        required: bool,
        max: usize,
    ) -> Option<&'a str> {
        match o.get(key) {
            None | Some(Value::Null) => {
                if required {
                    self.add(at, format!("`{key}` is required"));
                }
                None
            }
            Some(Value::String(s)) => {
                if required && s.trim().is_empty() {
                    self.add(at, format!("`{key}` is empty"));
                }
                if s.chars().count() > max {
                    self.add(at, format!("`{key}` is longer than {max} characters"));
                }
                Some(s)
            }
            Some(_) => {
                self.add(at, format!("`{key}` must be text"));
                None
            }
        }
    }

    fn arr(
        &mut self,
        o: &Map<String, Value>,
        key: &str,
        at: &str,
        min: usize,
        max: usize,
    ) -> Vec<Value> {
        let Some(v) = o.get(key).and_then(Value::as_array) else {
            self.add(at, format!("`{key}` must be a list"));
            return vec![];
        };
        if v.len() < min {
            let entries = if min == 1 { "entry" } else { "entries" };
            self.add(at, format!("`{key}` needs at least {min} {entries}"));
        }
        if v.len() > max {
            self.add(at, format!("`{key}` has more than {max} entries"));
        }
        v.iter().take(max).cloned().collect()
    }

    /// Only when the key is there: an optional list.
    fn opt_arr(
        &mut self,
        o: &Map<String, Value>,
        key: &str,
        at: &str,
        min: usize,
        max: usize,
    ) -> Option<Vec<Value>> {
        match o.get(key) {
            None | Some(Value::Null) => None,
            Some(_) => Some(self.arr(o, key, at, min, max)),
        }
    }

    fn each(
        &mut self,
        items: &[Value],
        at: &str,
        mut f: impl FnMut(&mut Self, &Map<String, Value>, &str),
    ) {
        for (k, x) in items.iter().enumerate() {
            let at = format!("{at} {}", k + 1);
            match x.as_object() {
                Some(o) => f(self, o, &at),
                None => self.add(&at, "must be an object"),
            }
        }
    }

    /// A whole number of at least `min`, when present.
    fn num(&mut self, o: &Map<String, Value>, key: &str, at: &str, min: i64) -> Option<f64> {
        match o.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => {
                let x = n.as_f64().unwrap_or(f64::NAN);
                if x.fract() != 0.0 {
                    self.add(at, format!("`{key}` must be a whole number"));
                } else if x < min as f64 {
                    self.add(at, format!("`{key}` must be at least {min}"));
                }
                Some(x)
            }
            Some(_) => {
                self.add(at, format!("`{key}` must be a number"));
                None
            }
        }
    }

    /// One of `allowed`. Absent is a problem only when `required` (the key
    /// has no default).
    fn one_of(
        &mut self,
        o: &Map<String, Value>,
        key: &str,
        at: &str,
        allowed: &[&str],
        required: bool,
    ) {
        match o.get(key) {
            None | Some(Value::Null) if required => self.add(at, format!("`{key}` is required")),
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if allowed.contains(&s.as_str()) => {}
            Some(_) => self.add(at, format!("`{key}` must be one of {}", allowed.join(", "))),
        }
    }

    /// A progress `id`, an error `code` or a guide's `page`: a key, never
    /// prose.
    fn key(&mut self, o: &Map<String, Value>, name: &str, at: &str) {
        if let Some(v) = self.str(o, name, at, true, KEY_MAX) {
            let ok = v
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'));
            if !v.is_empty() && !ok {
                self.add(at, format!("`{name}` must be letters, digits and . _ : -"));
            }
        }
    }
}
