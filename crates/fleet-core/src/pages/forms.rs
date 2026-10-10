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
/// A step's `name`, the word on its step chip.
pub const MAX_STEP_NAME: usize = 24;
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
    /// Offer "Save and finish later": the person may leave the form with
    /// what they typed kept, and come back to it while it is pending.
    #[serde(default, skip_serializing_if = "is_false")]
    pub save_later: bool,
    pub steps: Vec<FormStep>,
}

/// What a step is: fields to fill (the default), or a review of the steps
/// before it, each with an Edit link back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    #[default]
    Fields,
    /// Summarises the answers so far; has no fields and is the last step.
    Review,
}

impl StepKind {
    fn is_fields(&self) -> bool {
        *self == StepKind::Fields
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct FormStep {
    pub title: String,
    /// The step's short name on its step chip (≤ 24 chars). Default: the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `review`: a summary of the earlier steps, with no fields of its own.
    #[serde(default, skip_serializing_if = "StepKind::is_fields")]
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<FieldCondition>,
    /// At least one, except on a `review` step, which has none.
    #[serde(default)]
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
    /// select, multiselect: `[value, label]` pairs, or
    /// `{value, label, detail?, proposed?}` objects; the two mix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<FormOption>>,
    /// select: offer "Another…", a free entry beside the options. The
    /// answer may then be any text (≤ 500 chars), not only an option value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub other: bool,
    /// Shown but not answerable, with this reason under it ("Needs 2 GB
    /// free; mercury has 1.4 GB"). Never required; never in the answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// The default `value` was drafted by an AI: who and from what. The
    /// field shows the Drafted label until the person changes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drafted: Option<Drafted>,
    /// secret: where the value goes, shown under the field ("Written to a
    /// 0600 file on mercury, never shown to the agent").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_note: Option<String>,
}

/// One option of a select or multiselect: the original `[value, label]`
/// pair, or an object that may add a detail line and a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(untagged)]
pub enum FormOption {
    /// `[value, label]`.
    Pair(String, String),
    Full(OptionSpec),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct OptionSpec {
    pub value: String,
    pub label: String,
    /// One line under the label ("2 idle", "next free on main").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// This is the likely choice: shown first with "Proposed by …", the
    /// reason and Change. select only; at most one option per field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<OptionProposal>,
}

impl FormOption {
    pub fn value(&self) -> &str {
        match self {
            FormOption::Pair(v, _) => v,
            FormOption::Full(o) => &o.value,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            FormOption::Pair(_, l) => l,
            FormOption::Full(o) => &o.label,
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            FormOption::Pair(..) => None,
            FormOption::Full(o) => o.detail.as_deref(),
        }
    }

    pub fn proposed(&self) -> Option<&OptionProposal> {
        match self {
            FormOption::Pair(..) => None,
            FormOption::Full(o) => o.proposed.as_ref(),
        }
    }
}

/// Who proposes an option, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct OptionProposal {
    pub by: ProposedBy,
    /// Why, in words a person reads (≤ 500 chars).
    pub reason: String,
}

/// The three proposers of the design manual's AI patterns: a fleet rule,
/// the decision model (Jev), or an LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
pub enum ProposedBy {
    Rule,
    Jev,
    Llm,
}

/// Where a drafted default came from: `by` who wrote it ("haiku on
/// mercury"), `from` what it read ("the Jira epic PD-3012").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct Drafted {
    pub by: String,
    pub from: String,
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

/// What a person answered: the visible, non-secret values (typed; their
/// order is unspecified, key-sorted without serde_json's `preserve_order`),
/// and each visible secret's value, kept apart so it is never stored or
/// returned. `Debug` prints the secrets' names only.
#[derive(Clone, Default, PartialEq)]
pub struct Answers {
    pub values: Map<String, Value>,
    pub secrets: BTreeMap<String, String>,
}

impl std::fmt::Debug for Answers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Answers")
            .field("values", &self.values)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
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
        v.bad(
            "",
            format!("{total} fields, over the {MAX_FIELDS} a form may have"),
        );
    }
    let mut titles = BTreeSet::new();
    let mut seen: BTreeMap<&str, &FormField> = BTreeMap::new();
    for (i, step) in form.steps.iter().enumerate() {
        let at = format!("step {}", i + 1);
        v.text(&at, "title", &step.title, MAX_TITLE);
        if !titles.insert(step.title.as_str()) {
            v.bad(&at, "another step has this title");
        }
        if let Some(t) = &step.name {
            v.text(&at, "name", t, MAX_STEP_NAME);
        }
        if let Some(t) = &step.intro {
            v.text(&at, "intro", t, MAX_TEXT);
        }
        if let Some(c) = &step.when {
            check_condition(&mut v, &format!("{at} › when"), c, &seen);
        }
        match step.kind {
            StepKind::Fields if step.fields.is_empty() => {
                v.bad(&at, "a step has at least one field")
            }
            StepKind::Fields => {}
            StepKind::Review => {
                if !step.fields.is_empty() {
                    v.bad(&at, "a review step has no fields");
                }
                if i == 0 {
                    v.bad(&at, "a review step needs a step before it");
                } else if i + 1 != form.steps.len() {
                    v.bad(&at, "a review step is the last step");
                }
            }
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
    let not_for =
        |v: &mut Problems, key: &str| v.bad(at, format!("`{key}` is not for a {word} field"));
    if let Some(t) = &f.disabled_reason {
        v.text(at, "disabled_reason", t, MAX_TEXT);
        if f.required {
            v.bad(at, "a disabled field cannot be required");
        }
    }
    if let Some(d) = &f.drafted {
        v.text(at, "drafted.by", &d.by, MAX_LABEL);
        v.text(at, "drafted.from", &d.from, MAX_LABEL);
        if f.kind == Secret {
            not_for(v, "drafted");
        } else if f.value.is_none() {
            v.bad(at, "a drafted field needs the drafted `value`");
        }
    }
    match (&f.secret_note, f.kind) {
        (Some(t), Secret) => v.text(at, "secret_note", t, MAX_TEXT),
        (Some(_), _) => not_for(v, "secret_note"),
        (None, _) => {}
    }
    if f.other && f.kind != Select {
        not_for(v, "other");
    }
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
                v.bad(
                    at,
                    format!("a {word} field needs 1 to {MAX_OPTIONS} options"),
                );
            }
            let mut values = BTreeSet::new();
            let mut proposed = 0;
            for o in opts {
                let value = o.value();
                if !values.insert(value) {
                    v.bad(at, format!("option value {value:?} appears twice"));
                }
                if value.is_empty() {
                    v.bad(at, "an option value must not be empty");
                }
                v.text(at, "an option label", o.label(), MAX_LABEL);
                if let Some(d) = o.detail() {
                    v.text(at, "an option detail", d, MAX_LABEL);
                }
                if let Some(p) = o.proposed() {
                    proposed += 1;
                    v.text(at, "a proposal's reason", &p.reason, MAX_TEXT);
                }
            }
            if proposed > 0 && f.kind != Select {
                v.bad(at, "only a select's option can be proposed");
            } else if proposed > 1 {
                v.bad(at, "at most one option is proposed");
            }
        }
        (None, Select | Multiselect) => v.bad(
            at,
            format!("a {word} field needs 1 to {MAX_OPTIONS} options"),
        ),
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
    let forms = [
        c.field.is_some(),
        c.all.is_some(),
        c.any.is_some(),
        c.not.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if forms != 1 {
        v.bad(
            at,
            "a condition is exactly one of {field, …}, {all}, {any} or {not}",
        );
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
            v.bad(
                at,
                format!("`{name}` is a secret; a condition cannot read it"),
            );
            return;
        }
        if c.truthy.is_some() && target.kind != FieldType::Bool {
            v.bad(
                at,
                format!("`{name}`: truthy is for on/off fields; use eq or in"),
            );
        }
        if c.one_of.as_ref().is_some_and(Vec::is_empty) {
            v.bad(at, format!("`{name}`: `in` needs at least one value"));
        }
        for want in c.eq.iter().chain(c.one_of.iter().flatten()) {
            let ok = if target.kind == FieldType::Multiselect {
                want.as_str()
                    .is_some_and(|w| option_values(target).any(|o| o == w))
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
    f.options.iter().flatten().map(FormOption::value)
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
        Bool => v
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| "must be on or off".into()),
        Select => match v.as_str() {
            Some(s) if option_values(f).any(|o| o == s) => Ok(v.clone()),
            // "Another…": any text the person typed.
            Some(s) if f.other => {
                if s.chars().count() > TEXT_LEN.0 as usize {
                    return Err(format!("is longer than {} characters", TEXT_LEN.0));
                }
                Ok(v.clone())
            }
            _ if f.other => Err("must be one of the options or your own text".into()),
            _ => Err("must be one of the options".into()),
        },
        Multiselect => {
            let items = v.as_array().ok_or("must be a list of the options")?;
            let picked: BTreeSet<&str> = items.iter().filter_map(Value::as_str).collect();
            if picked.len() != items.len()
                || !picked.iter().all(|p| option_values(f).any(|o| o == *p))
            {
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
/// Values of hidden steps and fields are dropped, not checked; so is the
/// value of a disabled field (`disabled_reason`), which nobody can answer.
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
            if !holds(f.when.as_ref(), &out.values) || f.disabled_reason.is_some() {
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
                    out.secrets.insert(
                        f.name.clone(),
                        norm.as_str().unwrap_or_default().to_string(),
                    );
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
