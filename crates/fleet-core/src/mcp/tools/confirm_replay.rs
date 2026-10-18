//! An approval runs the call and tells Control (review 2026-10-17, F1).
//!
//! When the operator's call stops at `E_CONFIRM_REQUIRED`, `call_tool`
//! attaches a [`guard::ConfirmReplay`] to the nonce it was handed: the
//! request as Control made it and its context. Whoever answers the card —
//! the hub's `answer_mcp_confirm`, the desktop's `mcp_confirm`, a phone —
//! goes through `PendingConfirms::answer`, which hands the answer to that
//! replay once:
//!
//! - approved: the same call runs again through `call_tool` with the nonce
//!   (`guard::as_replay`, so the gate lets exactly this run spend the
//!   approval, once; Control's own retry is told `RunByFleet`), and a
//!   `[fleet] approved: <call> → <result>` note is queued for Control's next
//!   idle moment;
//! - denied: a `[fleet] denied: <call>` note.
//!
//! The nonce keeps its TTL: an unanswered request expires with its replay,
//! and an answer to an expired one runs nothing.

use super::*;
use crate::ipc_error::lock;
use crate::mcp::guard;

/// A note's result is cut to this many characters.
pub const NOTE_RESULT_MAX: usize = 400;

/// PURE: `text` on one line, cut to `max` characters with an ellipsis.
fn one_line(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// PURE: the note Control is queued after a person answered its card.
/// `outcome` is the replayed call's result text, `Err` when it failed;
/// `None` for a denial. Opens with `[fleet]`
/// (`prompt_origin::FLEET_PROMPT_MARKS`), so it is typed as fleet's.
pub fn answered_note(tool: &str, summary: &str, outcome: Option<Result<&str, &str>>) -> String {
    let call = one_line(&format!("{tool} {summary}"), 200);
    match outcome {
        Some(Ok(result)) => format!(
            "[fleet] approved: {call} → {}. A person approved it and fleet ran it for you; \
             carry on from here and do not run it again.",
            one_line(result, NOTE_RESULT_MAX)
        ),
        Some(Err(error)) => format!(
            "[fleet] approved: {call} → failed: {}. A person approved it and fleet ran it; \
             say in one line what failed and what you suggest.",
            one_line(error, NOTE_RESULT_MAX)
        ),
        None => format!(
            "[fleet] denied: {call}. The person said no: do not retry it; ask what they want \
             instead if it matters."
        ),
    }
}

/// The nonce an `E_CONFIRM_REQUIRED` result carries, if this is one.
pub(super) fn confirm_nonce_of(result: &CallToolResult) -> Option<String> {
    if !result.is_error.unwrap_or(false) {
        return None;
    }
    let sc = result.structured_content.as_ref()?;
    if sc.get("code")?.as_str()? != codes::E_CONFIRM_REQUIRED {
        return None;
    }
    Some(
        sc.get("details")?
            .get("confirm_nonce")?
            .as_str()?
            .to_string(),
    )
}

/// The text of a result, its blocks joined.
fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join(" ")
}

impl FleetTools {
    /// The replay for the operator's call `request` that just stopped at a
    /// card ([`guard::ConfirmReplay`]). `summary` is what the card shows.
    pub(super) fn operator_replay(
        &self,
        nonce: String,
        summary: String,
        mut request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> guard::ConfirmReplay {
        let this = self.clone();
        Box::new(move |approved| {
            Box::pin(async move {
                let tool = request.name.to_string();
                let outcome = if approved {
                    let args = request.arguments.get_or_insert_with(Default::default);
                    args.insert(
                        "confirm_nonce".into(),
                        serde_json::Value::String(nonce.clone()),
                    );
                    // The request Control made has long been answered: its
                    // cancellation is not this run's.
                    context.ct = tokio_util::sync::CancellationToken::new();
                    let r = guard::as_replay(nonce, this.call_tool(request, context)).await;
                    Some(match r {
                        Ok(r) if !r.is_error.unwrap_or(false) => Ok(result_text(&r)),
                        Ok(r) => Err(result_text(&r)),
                        Err(e) => Err(e.message.to_string()),
                    })
                } else {
                    None
                };
                let note = answered_note(
                    &tool,
                    &summary,
                    outcome
                        .as_ref()
                        .map(|o| o.as_deref().map_err(String::as_str)),
                );
                audit(
                    "confirm_replay",
                    &format!(
                        "tool={tool} approved={approved} ok={:?}",
                        outcome.map(|o| o.is_ok())
                    ),
                );
                this.note_operator(note).await;
            })
        })
    }

    /// Queue `note` for Control's next idle moment (the deferred queue, as
    /// the auto-restore note is). Best-effort: no operator, no note.
    async fn note_operator(&self, note: String) {
        let id = {
            let Ok(s) = lock(&self.store) else { return };
            crate::service::operator::operator_ref(&s).and_then(|o| {
                s.get_session(&o.tmux_name, &o.host_alias)
                    .ok()
                    .flatten()
                    .map(|r| r.id)
            })
        };
        let Some(session_id) = id else {
            tracing::warn!("no operator session to tell about an answered confirm");
            return;
        };
        if let Err(e) = sessions::queue_prompt(
            sessions::QueuePromptArgs {
                session_id,
                prompt: note,
                ..Default::default()
            },
            &self.store,
            &self.ssh,
        )
        .await
        {
            tracing::warn!(session_id, "answered-confirm note not queued: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notes_say_what_was_answered_and_what_came_of_it() {
        let ok = answered_note(
            "send_prompt",
            "session=cf-1 bytes=12",
            Some(Ok("{\"sent\":true}")),
        );
        assert!(
            ok.starts_with("[fleet] approved: send_prompt session=cf-1"),
            "{ok}"
        );
        assert!(ok.contains("→ {\"sent\":true}"), "{ok}");
        assert!(crate::service::prompt_origin::is_fleet_prompt(&ok));
        let failed = answered_note("new_session", "host=h", Some(Err("E_HOST_OFFLINE: down")));
        assert!(
            failed.contains("→ failed: E_HOST_OFFLINE: down"),
            "{failed}"
        );
        let denied = answered_note("safe_kill_session", "cf-2", None);
        assert!(
            denied.starts_with("[fleet] denied: safe_kill_session cf-2."),
            "{denied}"
        );
        assert!(crate::service::prompt_origin::is_fleet_prompt(&denied));
    }

    #[test]
    fn a_long_result_is_one_cut_line() {
        let long = format!("a\n{}", "x".repeat(2_000));
        let note = answered_note("run_prompt", "s", Some(Ok(&long)));
        assert!(!note.contains('\n'));
        assert!(note.contains('…'));
        assert!(note.chars().count() < NOTE_RESULT_MAX + 400);
    }

    #[test]
    fn only_a_confirm_required_result_carries_a_nonce() {
        let r = tool_error_result(mcp_err(
            codes::E_CONFIRM_REQUIRED,
            "needs a person",
            Some(serde_json::json!({ "confirm_nonce": "n1" })),
        ))
        .unwrap();
        assert_eq!(confirm_nonce_of(&r).as_deref(), Some("n1"));
        let other = tool_error_result(mcp_err(codes::E_FORBIDDEN, "no", None)).unwrap();
        assert_eq!(confirm_nonce_of(&other), None);
        assert_eq!(confirm_nonce_of(&CallToolResult::success(vec![])), None);
    }
}
