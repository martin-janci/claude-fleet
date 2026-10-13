//! Templates (`{{path}}`) for configurable agents. Design:
//! `docs/superpowers/specs/2026-10-12-configurable-agents-design.md`, A.
//!
//! One syntax, no expressions: `{{ root.segment.segment }}` where the root
//! is `inputs`, `subject`, `answer` or `now`; a path with no known root is
//! an input's name (`{{project}}` is `{{inputs.project}}`). A segment is
//! `[A-Za-z0-9_]+`. Rendering looks the path up in a JSON context and
//! writes a string as it is, a number or bool as its JSON text, an array as
//! its items joined by `, `, and anything missing as nothing.

use serde_json::Value;

/// A template's roots.
pub const ROOTS: [&str; 4] = ["inputs", "subject", "answer", "now"];

/// One `{{…}}` in a template, resolved to its root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub root: String,
    /// The segments after the root (`subject.comment.body` → `[comment, body]`).
    pub rest: Vec<String>,
}

impl Path {
    pub fn text(&self) -> String {
        std::iter::once(self.root.as_str())
            .chain(self.rest.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(".")
    }
}

fn segment_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// PURE: the paths of a template, or what is wrong with it (an unclosed
/// `{{`, an empty or malformed path).
pub fn paths(template: &str) -> Result<Vec<Path>, String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| format!("unclosed {{{{ in {template:?}"))?;
        let inner = after[..end].trim();
        let segs: Vec<&str> = inner.split('.').collect();
        if inner.is_empty() || !segs.iter().all(|s| segment_ok(s)) {
            return Err(format!("{{{{{inner}}}}} is not a path (name.name…)"));
        }
        let path = if ROOTS.contains(&segs[0]) {
            Path {
                root: segs[0].to_string(),
                rest: segs[1..].iter().map(|s| s.to_string()).collect(),
            }
        } else {
            Path {
                root: "inputs".into(),
                rest: segs.iter().map(|s| s.to_string()).collect(),
            }
        };
        if path.root == "now" && !path.rest.is_empty() {
            return Err(format!("{{{{{inner}}}}}: now has no fields"));
        }
        if path.root != "now" && path.rest.is_empty() {
            return Err(format!("{{{{{inner}}}}}: name a field of {}", path.root));
        }
        out.push(path);
        rest = &after[end + 2..];
    }
    Ok(out)
}

/// The paths of every string inside a JSON value (keys are not templates).
pub fn paths_in(value: &Value) -> Result<Vec<Path>, String> {
    match value {
        Value::String(s) => paths(s),
        Value::Array(a) => {
            let mut out = Vec::new();
            for v in a {
                out.extend(paths_in(v)?);
            }
            Ok(out)
        }
        Value::Object(m) => {
            let mut out = Vec::new();
            for v in m.values() {
                out.extend(paths_in(v)?);
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

fn lookup<'a>(ctx: &'a Value, path: &Path) -> Option<&'a Value> {
    let mut v = ctx.get(&path.root)?;
    for s in &path.rest {
        v = v.get(s)?;
    }
    Some(v)
}

fn as_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(as_text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// PURE: the template with each path replaced from `ctx`, a JSON object
/// with the roots as keys (`{"inputs": …, "subject": …, "answer": …,
/// "now": …}`). A template that does not parse renders as itself: the
/// validator refuses it long before anything renders.
pub fn render(template: &str, ctx: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        match paths(&rest[start..start + 2 + end + 2]) {
            Ok(p) if p.len() == 1 => {
                if let Some(v) = lookup(ctx, &p[0]) {
                    out.push_str(&as_text(v));
                }
            }
            _ => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// PURE: a JSON value with every string rendered; a string that is exactly
/// one path takes the value's own type (`"{{answer.count}}"` stays a number).
pub fn render_value(value: &Value, ctx: &Value) -> Value {
    match value {
        Value::String(s) => {
            let t = s.trim();
            if t.starts_with("{{") && t.ends_with("}}") && t[2..t.len() - 2].find("{{").is_none() {
                if let Ok(p) = paths(t) {
                    if p.len() == 1 {
                        return lookup(ctx, &p[0]).cloned().unwrap_or(Value::Null);
                    }
                }
            }
            Value::String(render(s, ctx))
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| render_value(v, ctx)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), render_value(v, ctx)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_resolve_their_root_and_bare_names_are_inputs() {
        let p = paths("Hi {{ subject.key }} in {{project}} at {{now}}").unwrap();
        assert_eq!(
            p.iter().map(Path::text).collect::<Vec<_>>(),
            ["subject.key", "inputs.project", "now"]
        );
        for bad in [
            "{{",
            "{{ }}",
            "{{a..b}}",
            "{{a b}}",
            "{{now.x}}",
            "{{answer}}",
        ] {
            assert!(paths(bad).is_err(), "{bad}");
        }
        assert_eq!(paths("no templates").unwrap(), []);
    }

    #[test]
    fn rendering_fills_what_is_there_and_drops_what_is_not() {
        let ctx = json!({
            "inputs": { "project": "L2S", "phrases": ["a", "b"] },
            "subject": { "key": "L2S-7", "comment": { "id": 9 } },
            "answer": { "ok": true },
            "now": 1700,
        });
        assert_eq!(
            render("{{project}}/{{subject.key}} c{{subject.comment.id}} {{inputs.phrases}} {{answer.ok}} {{answer.missing}}@{{now}}", &ctx),
            "L2S/L2S-7 c9 a, b true @1700"
        );
        assert_eq!(render("{{ broken", &ctx), "{{ broken");
    }

    #[test]
    fn a_lone_path_keeps_its_type_in_a_value() {
        let ctx = json!({ "answer": { "n": 3, "tags": ["x"] }, "subject": { "key": "K-1" } });
        let v = render_value(
            &json!({ "count": "{{answer.n}}", "tags": "{{ answer.tags }}", "title": "on {{subject.key}}", "gone": "{{answer.none}}" }),
            &ctx,
        );
        assert_eq!(
            v,
            json!({ "count": 3, "tags": ["x"], "title": "on K-1", "gone": null })
        );
        assert_eq!(
            paths_in(&json!({ "a": ["{{x}}", { "b": "{{answer.y}}" }] }))
                .unwrap()
                .len(),
            2
        );
    }
}
