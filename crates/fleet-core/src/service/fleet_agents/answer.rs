//! A configurable agent's answer: the JSON Schema subset its definition
//! describes it with (`run.output_schema`), the check of an answer against
//! it, and the conditions actions are taken on. Design:
//! `docs/superpowers/specs/2026-10-12-configurable-agents-design.md`, C, E.
//!
//! The subset: `type` (`object`, `string`, `number`, `integer`, `boolean`,
//! `array`), `properties`, `required`, `items`, `enum`, `maxLength`,
//! `minimum`, `maximum`, `description`. The root is an object. Nothing
//! else (`$ref`, `oneOf`, `pattern`, `additionalProperties` …) is accepted:
//! a schema the runtime cannot check is refused, not half-checked.

use crate::pages::forms::FieldCondition;
use serde_json::{Map, Value};

const KEYWORDS: [&str; 9] = [
    "type",
    "properties",
    "required",
    "items",
    "enum",
    "maxLength",
    "minimum",
    "maximum",
    "description",
];
const TYPES: [&str; 6] = ["object", "string", "number", "integer", "boolean", "array"];

/// Deepest nesting a schema may have.
const MAX_DEPTH: usize = 6;

fn check_node(node: &Value, at: &str, depth: usize, out: &mut Vec<String>) {
    let Some(obj) = node.as_object() else {
        out.push(format!("{at}: a schema is an object"));
        return;
    };
    if depth > MAX_DEPTH {
        out.push(format!("{at}: nested deeper than {MAX_DEPTH}"));
        return;
    }
    for k in obj.keys() {
        if !KEYWORDS.contains(&k.as_str()) {
            out.push(format!("{at}: `{k}` is not in the supported subset"));
        }
    }
    let ty = obj.get("type").and_then(Value::as_str);
    match (ty, obj.get("enum")) {
        (None, None) => out.push(format!("{at}: give a type or an enum")),
        (Some(t), _) if !TYPES.contains(&t) => {
            out.push(format!("{at}: type `{t}` is not one of {TYPES:?}"))
        }
        _ => {}
    }
    if let Some(e) = obj.get("enum") {
        match e.as_array() {
            Some(a)
                if !a.is_empty()
                    && a.iter()
                        .all(|v| v.is_string() || v.is_number() || v.is_boolean()) => {}
            _ => out.push(format!(
                "{at}: enum is a non-empty list of strings, numbers or booleans"
            )),
        }
    }
    let props = obj.get("properties");
    if let Some(p) = props {
        if ty != Some("object") {
            out.push(format!("{at}: properties belong to an object"));
        }
        match p.as_object() {
            Some(m) => {
                for (name, sub) in m {
                    check_node(sub, &format!("{at}.{name}"), depth + 1, out);
                }
            }
            None => out.push(format!("{at}: properties is an object")),
        }
    }
    if let Some(r) = obj.get("required") {
        let names = props.and_then(Value::as_object);
        match r.as_array() {
            Some(a) => {
                for n in a {
                    match n.as_str() {
                        Some(s) if names.is_some_and(|m| m.contains_key(s)) => {}
                        _ => out.push(format!("{at}: required names {n}, which is not a property")),
                    }
                }
            }
            None => out.push(format!("{at}: required is a list")),
        }
    }
    if let Some(i) = obj.get("items") {
        if ty != Some("array") {
            out.push(format!("{at}: items belong to an array"));
        }
        check_node(i, &format!("{at}[]"), depth + 1, out);
    }
    if obj.contains_key("maxLength") && obj.get("maxLength").and_then(Value::as_u64).is_none() {
        out.push(format!("{at}: maxLength is a whole number"));
    }
    for k in ["minimum", "maximum"] {
        if obj.contains_key(k) && !obj[k].is_number() {
            out.push(format!("{at}: {k} is a number"));
        }
    }
}

/// PURE: what is wrong with an output schema; empty when it is fine.
pub fn check_schema(schema: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        out.push("output_schema: the root is `type: object`".into());
    }
    if schema
        .get("properties")
        .and_then(Value::as_object)
        .is_none_or(|m| m.is_empty())
    {
        out.push("output_schema: name at least one property".into());
    }
    check_node(schema, "output_schema", 0, &mut out);
    out
}

/// The top-level fields an answer may have.
pub fn fields(schema: &Value) -> Vec<String> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Whether `path` (after `answer.`) names something the schema describes.
pub fn has_path(schema: &Value, path: &[String]) -> bool {
    let mut node = schema;
    for seg in path {
        match node.get("properties").and_then(|p| p.get(seg)) {
            Some(n) => node = n,
            None => {
                // Inside an array, a numeric segment is an item.
                if node.get("type").and_then(Value::as_str) == Some("array")
                    && seg.chars().all(|c| c.is_ascii_digit())
                {
                    node = node.get("items").unwrap_or(&Value::Null);
                } else {
                    return false;
                }
            }
        }
    }
    true
}

fn check_value(schema: &Value, v: &Value, at: &str, out: &mut Vec<String>) {
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        if !e.contains(v) {
            out.push(format!(
                "{at}: {v} is not one of {}",
                Value::Array(e.clone())
            ));
            return;
        }
    }
    let ok = match schema.get("type").and_then(Value::as_str) {
        None => true,
        Some("object") => v.is_object(),
        Some("string") => v.is_string(),
        Some("number") => v.is_number(),
        Some("integer") => v.as_i64().is_some() || v.as_u64().is_some(),
        Some("boolean") => v.is_boolean(),
        Some("array") => v.is_array(),
        Some(_) => false,
    };
    if !ok {
        out.push(format!(
            "{at}: not a {}",
            schema["type"].as_str().unwrap_or("?")
        ));
        return;
    }
    if let (Some(max), Some(s)) = (schema.get("maxLength").and_then(Value::as_u64), v.as_str()) {
        if s.chars().count() as u64 > max {
            out.push(format!("{at}: longer than {max} characters"));
        }
    }
    if let Some(n) = v.as_f64() {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|m| n < m)
        {
            out.push(format!("{at}: below the minimum"));
        }
        if schema
            .get("maximum")
            .and_then(Value::as_f64)
            .is_some_and(|m| n > m)
        {
            out.push(format!("{at}: above the maximum"));
        }
    }
    if let Some(obj) = v.as_object() {
        for r in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = r.as_str() {
                if !obj.contains_key(name) || obj[name].is_null() {
                    out.push(format!("{at}.{name}: required"));
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (k, sub) in props {
                if let Some(x) = obj.get(k).filter(|x| !x.is_null()) {
                    check_value(sub, x, &format!("{at}.{k}"), out);
                }
            }
        }
    }
    if let (Some(items), Some(a)) = (schema.get("items"), v.as_array()) {
        for (i, x) in a.iter().enumerate() {
            check_value(items, x, &format!("{at}[{i}]"), out);
        }
    }
}

/// PURE: an answer checked against its schema: the answer with only the
/// properties the schema names, or every problem.
pub fn check_answer(schema: &Value, answer: &Value) -> Result<Value, Vec<String>> {
    let mut out = Vec::new();
    check_value(schema, answer, "answer", &mut out);
    if !out.is_empty() {
        return Err(out);
    }
    let names = fields(schema);
    let kept: Map<String, Value> = answer
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| names.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok(Value::Object(kept))
}

/// An answer value's truth: `false`, `null`, `0`, `""`, `[]` and `{}` are false.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
    }
}

/// PURE: does an action's condition hold over the answer? The form
/// conditions (`docs/forms.md` → *Conditions*), over the answer's
/// top-level fields, with `truthy` meaning a value that is not empty.
pub fn holds(c: Option<&FieldCondition>, answer: &Value) -> bool {
    let Some(c) = c else { return true };
    if let Some(all) = &c.all {
        return all.iter().all(|x| holds(Some(x), answer));
    }
    if let Some(any) = &c.any {
        return any.iter().any(|x| holds(Some(x), answer));
    }
    if let Some(not) = &c.not {
        return !holds(Some(not), answer);
    }
    let Some(name) = &c.field else { return true };
    let got = answer.get(name);
    let matches = |want: &Value| match got {
        Some(Value::Array(items)) => items.contains(want),
        Some(g) => g == want,
        None => false,
    };
    if let Some(want) = &c.eq {
        return matches(want);
    }
    if let Some(list) = &c.one_of {
        return list.iter().any(matches);
    }
    if let Some(t) = c.truthy {
        return truthy(got) == t;
    }
    true
}

/// PURE: the answer fields a condition names, every one.
pub fn condition_fields(c: &FieldCondition) -> Vec<String> {
    let mut out: Vec<String> = c.field.iter().cloned().collect();
    for list in [&c.all, &c.any].into_iter().flatten() {
        for x in list {
            out.extend(condition_fields(x));
        }
    }
    if let Some(n) = &c.not {
        out.extend(condition_fields(n));
    }
    out
}

/// PURE: what is wrong with a condition's shape: exactly one form each.
pub fn check_condition(c: &FieldCondition, at: &str) -> Vec<String> {
    let forms = [
        c.field.is_some(),
        c.all.is_some(),
        c.any.is_some(),
        c.not.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let tests = [c.eq.is_some(), c.one_of.is_some(), c.truthy.is_some()]
        .iter()
        .filter(|b| **b)
        .count();
    let mut out = Vec::new();
    if forms != 1 || (c.field.is_some() && tests != 1) || (c.field.is_none() && tests != 0) {
        out.push(format!(
            "{at}: one of {{field + eq|in|truthy}}, {{all}}, {{any}}, {{not}}"
        ));
    }
    for (i, x) in c.all.iter().chain(c.any.iter()).flatten().enumerate() {
        out.extend(check_condition(x, &format!("{at} › {}", i + 1)));
    }
    if let Some(n) = &c.not {
        out.extend(check_condition(n, &format!("{at} › not")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "required": ["rootCause", "confidence"],
            "properties": {
                "rootCause": { "type": "string", "maxLength": 20 },
                "confidence": { "enum": ["high", "medium", "low"] },
                "count": { "type": "integer", "minimum": 0 },
                "tags": { "type": "array", "items": { "type": "string" } },
                "needsMoreInfo": { "type": "string" }
            }
        })
    }

    #[test]
    fn a_schema_stays_in_the_subset() {
        assert!(
            check_schema(&schema()).is_empty(),
            "{:?}",
            check_schema(&schema())
        );
        let bad = json!({ "type": "object", "properties": {
            "a": { "type": "string", "pattern": "x" },
            "b": { "$ref": "#/x" },
            "c": { "type": "date" },
            "d": { "type": "string", "items": { "type": "string" } }
        }, "required": ["zz"] });
        let p = check_schema(&bad);
        for want in [
            "`pattern`",
            "`$ref`",
            "type `date`",
            "items belong",
            "required names \"zz\"",
        ] {
            assert!(p.iter().any(|x| x.contains(want)), "{want}: {p:?}");
        }
        assert!(!check_schema(&json!({ "type": "string" })).is_empty());
    }

    #[test]
    fn an_answer_is_checked_and_trimmed_to_the_schema() {
        let ok = check_answer(
            &schema(),
            &json!({ "rootCause": "key", "confidence": "high", "count": 2, "extra": 1 }),
        )
        .unwrap();
        assert_eq!(
            ok,
            json!({ "rootCause": "key", "confidence": "high", "count": 2 })
        );
        let errs = check_answer(
            &schema(),
            &json!({ "rootCause": "x".repeat(30), "confidence": "sure", "count": -1, "tags": [1] }),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 4, "{errs:?}");
        assert!(
            check_answer(&schema(), &json!({ "confidence": "low" })).unwrap_err()[0]
                .contains("rootCause: required")
        );
    }

    #[test]
    fn conditions_read_the_answer() {
        let c: FieldCondition = serde_json::from_value(json!({ "all": [
            { "field": "confidence", "eq": "high" },
            { "not": { "field": "needsMoreInfo", "truthy": true } }
        ] }))
        .unwrap();
        assert!(check_condition(&c, "when").is_empty());
        assert!(holds(
            Some(&c),
            &json!({ "confidence": "high", "needsMoreInfo": "" })
        ));
        assert!(!holds(
            Some(&c),
            &json!({ "confidence": "high", "needsMoreInfo": "which device?" })
        ));
        assert!(holds(
            Some(&serde_json::from_value(json!({ "field": "tags", "in": ["a", "z"] })).unwrap()),
            &json!({ "tags": ["z"] })
        ));
        assert_eq!(condition_fields(&c), ["confidence", "needsMoreInfo"]);
        let two: FieldCondition =
            serde_json::from_value(json!({ "field": "a", "eq": 1, "truthy": true })).unwrap();
        assert!(!check_condition(&two, "when").is_empty());
    }

    #[test]
    fn paths_follow_properties_and_items() {
        let s = schema();
        assert!(has_path(&s, &["tags".into(), "0".into()]));
        assert!(has_path(&s, &["rootCause".into()]));
        assert!(!has_path(&s, &["nope".into()]));
    }
}
