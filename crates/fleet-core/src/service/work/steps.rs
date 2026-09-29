//! Agent steps (design 2026-09-29 §2): the transient level under a task —
//! an agent's own todos, captured, never typed. One adapter per agent turns
//! its native events into [`StepEvent`]s; storage, the view and the UI see
//! only those. Claude Code is the first adapter (`TaskCreate`,
//! `TaskUpdate`, legacy `TodoWrite`; shapes verified 2026-09-29). A step's
//! `completed` is the agent's word, never a task status and never evidence.

use serde_json::Value;

pub const STEP_CAP: usize = 200;
pub const STEP_TEXT_MAX_CHARS: usize = 300;
/// The tools whose PostToolUse carries steps (the hook matcher adds these).
pub const CLAUDE_STEP_TOOLS: &[&str] = &["TaskCreate", "TaskUpdate", "TodoWrite"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl StepState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "cancelled" | "deleted" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEvent {
    pub native_id: String,
    /// `None`: keep the text already known for this step.
    pub text: Option<String>,
    /// `None`: keep the state already known.
    pub state: Option<StepState>,
    pub agent: &'static str,
}

fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(STEP_TEXT_MAX_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Claude Code's adapter.
pub fn steps_from_claude_tool(
    tool: &str,
    input: &Value,
    response: Option<&Value>,
) -> Vec<StepEvent> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match tool {
        "TaskCreate" => {
            let Some(id) = response.and_then(|r| r.pointer("/task/id")).and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            }) else {
                return Vec::new();
            };
            vec![StepEvent {
                native_id: format!("task:{id}"),
                text: s(input, "subject")
                    .map(|t| clean(&t))
                    .filter(|t| !t.is_empty()),
                state: Some(StepState::Pending),
                agent: "claude_code",
            }]
        }
        "TaskUpdate" => {
            let Some(id) = s(input, "taskId").or_else(|| s(input, "task_id")) else {
                return Vec::new();
            };
            vec![StepEvent {
                native_id: format!("task:{id}"),
                text: s(input, "subject")
                    .map(|t| clean(&t))
                    .filter(|t| !t.is_empty()),
                state: s(input, "status").and_then(|st| StepState::parse(&st)),
                agent: "claude_code",
            }]
        }
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|todos| {
                todos
                    .iter()
                    .filter_map(|t| {
                        let text = clean(&s(t, "content")?);
                        (!text.is_empty()).then(|| StepEvent {
                            native_id: format!("todo:{}", text.to_lowercase()),
                            text: Some(text),
                            state: s(t, "status").and_then(|st| StepState::parse(&st)),
                            agent: "claude_code",
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(name: &str) -> serde_json::Value {
        let p = format!(
            "{}/src/service/work/testdata/steps/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn task_create_takes_its_id_from_the_response() {
        let f = fx("task_create.json");
        let e = steps_from_claude_tool("TaskCreate", &f["input"], Some(&f["response"]));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].native_id, "task:1");
        assert_eq!(
            e[0].text.as_deref(),
            Some("Design mandatory-test-coverage feature")
        );
        assert_eq!(e[0].state, Some(StepState::Pending));
    }

    #[test]
    fn task_update_moves_the_state_and_keeps_the_text() {
        let f = fx("task_update.json");
        let e = steps_from_claude_tool("TaskUpdate", &f["input"], None);
        assert_eq!(
            (e[0].native_id.as_str(), e[0].text.as_deref(), e[0].state),
            ("task:1", None, Some(StepState::InProgress))
        );
        let alt = steps_from_claude_tool(
            "TaskUpdate",
            &serde_json::json!({"task_id": "2", "status": "completed"}),
            None,
        );
        assert_eq!(alt[0].native_id, "task:2");
    }

    #[test]
    fn todo_write_is_one_event_per_item_keyed_by_its_text() {
        let f = fx("todo_write.json");
        let e = steps_from_claude_tool("TodoWrite", &f["input"], None);
        assert_eq!(e.len(), 3);
        assert_eq!(e[1].native_id, "todo:write the synthesis");
        assert_eq!(e[1].state, Some(StepState::InProgress));
    }

    #[test]
    fn a_task_create_without_an_id_and_other_tools_yield_nothing() {
        let f = fx("task_create.json");
        assert!(steps_from_claude_tool("TaskCreate", &f["input"], None).is_empty());
        assert!(
            steps_from_claude_tool("Bash", &serde_json::json!({"command": "ls"}), None).is_empty()
        );
    }

    #[test]
    fn step_text_is_capped_and_stripped_of_control_characters() {
        let long = format!("a\u{7}{}", "x".repeat(400));
        let e = steps_from_claude_tool(
            "TodoWrite",
            &serde_json::json!({"todos": [{"content": long, "status": "pending"}]}),
            None,
        );
        let t = e[0].text.clone().unwrap();
        assert_eq!(t.chars().count(), STEP_TEXT_MAX_CHARS);
        assert!(!t.contains('\u{7}'));
    }
}
