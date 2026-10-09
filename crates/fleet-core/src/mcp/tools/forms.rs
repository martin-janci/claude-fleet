//! The `ask` tool: chat forms (docs/forms.md). An agent opens a form in its
//! own session's chat and waits; a person (never a host token) answers.

use super::*;
use crate::ipc_error::lock;
use crate::service::forms;

/// Who answered, in words, for the agent and the card.
fn answered_by(caller: &Caller) -> String {
    match &caller.client {
        Some(c) => format!("{} (device)", c.name),
        None => "the control API".into(),
    }
}

#[tool_router(router = forms_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(
        description = "Chat forms: `form` (fleet.form/1) opens a form in YOUR \
        session's chat and waits ≤600 s for the person's answers (status \
        answered | pending | declined | cancelled | expired; on pending call \
        `wait`). `draft` shows it while written. \
        `cancel` withdraws. A person's side: list, get, answer, decline. \
        Spec: docs/forms.md."
    )]
    pub(super) async fn ask(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<AskParams>,
    ) -> Result<CallToolResult, McpError> {
        let actions = [
            p.form.is_some(),
            p.draft.is_some(),
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
                "say exactly one of form, draft, wait, cancel, list, get, answer or decline",
                None,
            ));
        }
        if let Some(spec) = &p.form {
            audit("ask", "action=form");
            let session_id = self.asking_session(&caller)?;
            let view =
                forms::open(&self.store, session_id, spec, p.why.as_deref()).map_err(to_mcp_err)?;
            // J5: the likely option of its first choice, off the form's path.
            let org = lock(&self.store)
                .ok()
                .and_then(|s| s.session_org(session_id).ok().flatten());
            crate::service::decide::quick_answer::spawn_for_form(
                crate::service::decide::DecideCtx::jev(std::sync::Arc::clone(&self.store)),
                &view.form_id,
                org,
                &view.title,
                view.why.as_deref(),
                &view.spec,
            );
            return self
                .wait_on(&caller, session_id, &view.form_id, p.timeout_s)
                .await;
        }
        if let Some(text) = &p.draft {
            audit("ask", "action=draft");
            let session_id = self.asking_session(&caller)?;
            return ok_json_compact(
                &forms::draft(&self.store, session_id, text, p.why.as_deref())
                    .map_err(to_mcp_err)?,
            );
        }
        if let Some(id) = &p.wait {
            audit("ask", &format!("action=wait form_id={}", id.escape_debug()));
            let row = forms::row(&self.store, id).map_err(to_mcp_err)?;
            self.resolve_target_row(
                &caller,
                Some(row.session_id),
                None,
                None,
                Reach::Read,
                "the form's session",
            )?;
            return self.wait_on(&caller, row.session_id, id, p.timeout_s).await;
        }
        if let Some(id) = &p.cancel {
            audit(
                "ask",
                &format!("action=cancel form_id={}", id.escape_debug()),
            );
            let row = forms::row(&self.store, id).map_err(to_mcp_err)?;
            let own = self.view_scope(&caller)?.proven_session == Some(row.session_id);
            if !(own || caller.is_master()) {
                return Err(mcp_err(
                    codes::E_FORBIDDEN,
                    "only the asking session withdraws its form",
                    None,
                ));
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
            self.resolve_target_row(
                &caller,
                Some(row.session_id),
                None,
                None,
                Reach::Read,
                "the form's session",
            )?;
            let view = match lock(&self.store) {
                Ok(s) => forms::view_with_proposal(&s, &row),
                Err(_) => forms::view(&row),
            };
            return ok_json_compact(&view);
        }
        // answer / decline: a person's, through `drive` on the session.
        let id = p
            .answer
            .as_ref()
            .or(p.decline.as_ref())
            .expect("one action");
        audit(
            "ask",
            &format!(
                "action={} form_id={}",
                if p.answer.is_some() {
                    "answer"
                } else {
                    "decline"
                },
                id.escape_debug()
            ),
        );
        if caller.host_alias.is_some() {
            return Err(mcp_err(
                codes::E_FORBIDDEN,
                "an agent never answers a form; a person does",
                None,
            ));
        }
        let row = forms::row(&self.store, id).map_err(to_mcp_err)?;
        self.resolve_target_row(
            &caller,
            Some(row.session_id),
            None,
            None,
            Reach::Drive,
            "the form's session",
        )?;
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
    /// The session a form (or its draft) opens in: the caller's own, proven
    /// by its per-host token and `X-Fleet-Pane`.
    fn asking_session(&self, caller: &Caller) -> Result<i64, McpError> {
        let scope = self.view_scope(caller)?;
        scope
            .proven_session
            .filter(|_| caller.host_alias.is_some())
            .ok_or_else(|| {
                mcp_err(
                    codes::E_NOT_A_SESSION,
                    "a form opens in the asking session's chat: call ask from inside a fleet session (its per-host token and X-Fleet-Pane)",
                    None,
                )
            })
    }

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
        let r = forms::wait(
            &self.store,
            form_id,
            forms::wait_timeout(timeout_s),
            &recheck,
        )
        .await
        .map_err(to_mcp_err)?;
        self.recheck_now(&recheck)?;
        ok_json(&r)
    }

    fn visible_forms(
        &self,
        caller: &Caller,
        f: &AskListFilter,
    ) -> Result<Vec<forms::FormView>, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let rows = s
            .forms(f.session_id, f.state.as_deref())
            .map_err(|e| to_mcp_err(e.into()))?;
        Ok(rows
            .iter()
            .filter(|r| {
                resolve_row_and_gate(
                    &s,
                    caller,
                    Some(r.session_id),
                    None,
                    None,
                    Reach::Read,
                    "the form's session",
                )
                .is_ok()
            })
            .map(forms::view)
            .collect())
    }
}
