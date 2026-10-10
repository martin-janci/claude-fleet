//! MCP tools: sharing a session, reading who holds what, and claiming an
//! unclaimed row (multi-user M1, T12).
//!
//! Six tools, two audiences, and the split is the design:
//!
//! * the five sharing surfaces answer a PERSON's device (and the master).
//!   They are `Access::Client` so a paired device passes the central gate,
//!   and every one of them is in `guard::NOT_FOR_HOST_TOKENS` — a per-host
//!   token proves no person, so it can never be an owner or a grantee and a
//!   definition it could never use would cost every host's Claude request
//!   bytes;
//! * `session_claim` answers a per-host token and nothing else
//!   (`Access::HostToken`, the one row of that variant). The operator's own
//!   claim is `fleet-hub session claim`, which writes through `state.db` on
//!   the hub machine.
//!
//! **Every tool here addresses its session by ROW ID only.** The
//! `host_alias` + `tmux_name` fallback every other session tool offers is
//! deliberately absent: that pair is reusable — the next session started on
//! a host can take a dead one's tmux name — so a grant or a claim resolved by
//! name could land on a different row than the one the caller read. See the
//! comment above the param structs in `params.rs`.
//!
//! **Where the owner-only rule is enforced, twice.** `Reach::Own` in each
//! handler, so a caller who is not the owner is refused by the one gate every
//! session-addressed tool passes through (and a caller who cannot see the row
//! at all gets `E_NOTFOUND`, not a refusal that tells them it exists); and
//! again inside the store's own statements, whose `WHERE` carries
//! `sessions.owner_person_id = ?granter`. The two are the same rule at the
//! two layers that each need it (spec §4.3), not a duplication to collapse.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = sharing_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Share a session you OWN with a person, or an org \
        you are in (its members from now), at watch (read), answer (also a \
        dialog's keys) or drive (also prompt). Owner only; never 'own' or a terminal. Returns the row. \
        Errors: E_NOTFOUND, E_FORBIDDEN, E_VALIDATE, E_EXISTS.")]
    pub(super) async fn session_share(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionShareParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_share",
            &format!(
                "session_id={} person={:?} org={:?} level={:?}",
                p.session_id, p.person, p.org, p.level
            ),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        // `Reach::Own`: re-sharing is the `own` tier (spec §4.3 invariant 5),
        // which is where "a grantee cannot grant on" is enforced — a `drive`
        // grantee reaches `may_drive` and never `may_own`.
        resolve_row_and_gate(
            &s,
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Own,
            "the session to share",
        )?;
        let granter = super::fleet::owner_for(&caller, &s);
        let to = sessions::ShareTo::from_fields(&p.person, p.org.as_deref()).map_err(to_mcp_err)?;
        let row = sessions::share_session_to(&s, p.session_id, to, &p.level, granter)
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Revoke a person's or org's grant on your session \
        (owner only; kept, revoked, for audit). Returns the session row. \
        Errors: E_NOTFOUND, E_FORBIDDEN.")]
    pub(super) async fn session_unshare(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionGrantParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_unshare",
            &format!(
                "session_id={} person={:?} org={:?}",
                p.session_id, p.person, p.org
            ),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        resolve_row_and_gate(
            &s,
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Own,
            "the session to unshare",
        )?;
        let granter = super::fleet::owner_for(&caller, &s);
        let to = sessions::ShareTo::from_fields(&p.person, p.org.as_deref()).map_err(to_mcp_err)?;
        let row =
            sessions::unshare_session_to(&s, p.session_id, to, granter).map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Lower a person's or org's grant on your session to \
        watch (owner only). Nothing raises one: revoke and share \
        again. Returns the session row. Errors: E_NOTFOUND, E_FORBIDDEN.")]
    pub(super) async fn session_narrow(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionGrantParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_narrow",
            &format!(
                "session_id={} person={:?} org={:?}",
                p.session_id, p.person, p.org
            ),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        resolve_row_and_gate(
            &s,
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Own,
            "the session to narrow a grant on",
        )?;
        let granter = super::fleet::owner_for(&caller, &s);
        let to = sessions::ShareTo::from_fields(&p.person, p.org.as_deref()).map_err(to_mcp_err)?;
        let row =
            sessions::narrow_session_share_to(&s, p.session_id, to, granter).map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Who holds a live grant on your session (a person \
        or an org), the level, who granted it and when. Owner only: it names \
        others. Errors: E_NOTFOUND, E_FORBIDDEN.")]
    pub(super) async fn session_access(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionAccessParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_access", &format!("session_id={}", p.session_id));
        let s = lock(&self.store).map_err(to_mcp_err)?;
        // `Own` and not `Read`, though this is a read: the answer names OTHER
        // PEOPLE who hold a grant, which is not part of what a `watch` grant
        // promised and not the grantee's to learn. `readonly: true` in
        // `TOOL_POLICIES` answers the different question of whether a
        // readonly TOKEN may call it at all.
        resolve_row_and_gate(
            &s,
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Own,
            "the session whose grants to read",
        )?;
        let grants = sessions::session_access(&s, p.session_id).map_err(to_mcp_err)?;
        ok_json_compact(&grants)
    }

    #[tool(description = "Who you are on this fleet and every live grant TO \
        you: { person_id, grants: [{ session_id, level }] }. With each row's \
        owner_person_id and visibility, this is what a client derives its \
        access from. Re-read after a reconnect the hub could not replay.")]
    pub(super) async fn my_grants(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("my_grants", "");
        let s = lock(self.reader()).map_err(to_mcp_err)?;
        // The caller's own person and nothing else: a device carries it on
        // the connection, and the master's is the hub's personal owner (one
        // mapping, in `owner_for`). `None` — a device no pairing bound, a hub
        // that cannot say whose it is — answers an EMPTY list, never every
        // grant.
        let who = super::fleet::owner_for(&caller, &s);
        let answer = sessions::my_grants(&s, who).map_err(to_mcp_err)?;
        ok_json(&answer)
    }

    #[tool(description = "Ask the owner of a session shared with you for a \
        wider level (answer or drive). Confers nothing until they grant it; \
        one open ask per session. Errors: E_NOTFOUND, E_FORBIDDEN, \
        E_VALIDATE, E_EXISTS, E_INVALID_STATE (declined within the hour).")]
    pub(super) async fn session_ask_access(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionAskAccessParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_ask_access",
            &format!("session_id={} level={:?}", p.session_id, p.level),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        // `Reach::Read`: the asker is a grantee at any level, never the
        // owner. A caller who cannot see the row gets `E_NOTFOUND` here; the
        // store then checks the grant itself.
        resolve_row_and_gate(
            &s,
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session to ask about",
        )?;
        let asker = super::fleet::owner_for(&caller, &s);
        let view = sessions::ask_access(&s, p.session_id, &p.level, asker).map_err(to_mcp_err)?;
        ok_json(&view)
    }

    #[tool(description = "Asks for a wider level on sessions you OWN. \
        list (default; session_id narrows it), grant {id} (re-shares at the \
        asked level) or decline {id}. Errors: E_NOTFOUND, E_FORBIDDEN, \
        E_VALIDATE, E_INVALID_STATE (the asker's share is gone).")]
    pub(super) async fn access_requests(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<AccessRequestsParams>,
    ) -> Result<CallToolResult, McpError> {
        let action = p.action.as_deref().unwrap_or("list");
        audit(
            "access_requests",
            &format!(
                "action={action} session_id={:?} id={:?}",
                p.session_id, p.id
            ),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let owner = super::fleet::owner_for(&caller, &s);
        match action {
            "list" => {
                if let Some(id) = p.session_id {
                    resolve_row_and_gate(
                        &s,
                        &caller,
                        Some(id),
                        None,
                        None,
                        Reach::Own,
                        "the session whose asks to read",
                    )?;
                }
                let list =
                    sessions::access_requests(&s, owner, p.session_id).map_err(to_mcp_err)?;
                ok_json_compact(&list)
            }
            "grant" | "decline" => {
                let id = p.id.ok_or_else(|| {
                    to_mcp_err(IpcError::new(
                        codes::E_VALIDATE,
                        format!("{action} needs the ask's id"),
                    ))
                })?;
                // The ask's session passes the same owner gate as a share
                // (`Reach::Own`); the store compares the owner again. Every
                // refusal before the store's is the store's own "no open
                // ask" sentence, so an id says nothing about whether it
                // exists or which session it is on.
                let not_found = || {
                    to_mcp_err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("no open access request {id} on a session of yours"),
                    ))
                };
                let session_id = s
                    .access_request(id)
                    .map_err(to_mcp_err)?
                    .filter(|r| r.resolved_at.is_none())
                    .map(|r| r.session_id)
                    .ok_or_else(not_found)?;
                resolve_row_and_gate(
                    &s,
                    &caller,
                    Some(session_id),
                    None,
                    None,
                    Reach::Own,
                    "the session the ask is about",
                )
                .map_err(|_| not_found())?;
                let view = sessions::resolve_access_request(&s, id, action == "grant", owner)
                    .map_err(to_mcp_err)?;
                ok_json(&view)
            }
            other => Err(to_mcp_err(IpcError::new(
                codes::E_VALIDATE,
                format!("access_requests action must be list, grant or decline, not {other:?}"),
            ))),
        }
    }

    #[tool(description = "Claim the unclaimed session THIS pane is in for a \
        person: it becomes theirs and private. Only the session whose active \
        pane this request's X-Fleet-Pane header names — being on the same \
        host is not enough. Errors: E_NOTFOUND, E_INVALID_STATE (no pane of \
        it proven), E_FORBIDDEN (another session's pane), E_EXISTS (owned).")]
    pub(super) async fn session_claim(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionClaimParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_claim",
            &format!("session_id={} person={:?}", p.session_id, p.person),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        // `Reach::Read` is the only reach that can stand here, and the reason
        // is worth writing down: `may_own` is false for a per-host token
        // whatever it proves (the pane says "I am standing in this session",
        // never "this session is mine"), so `Reach::Own` would refuse the one
        // caller this tool exists for. What authorises the WRITE is not the
        // reach at all — it is `claim::claim_session`'s pane check plus the
        // row being unowned, both below.
        //
        // The gate is still load-bearing: it applies `require_host` and the
        // org boundary, and it answers `E_NOTFOUND` for a row this token may
        // not see — which is what keeps the refusals below from being an
        // existence oracle over another person's private sessions.
        resolve_row_and_gate(
            &s,
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session to claim",
        )?;
        // Built off the SAME handle the row came from: a scope read through
        // the other one races the reconcile pass that rewrites
        // `tmux_pane_id`, which is the pane this is about to trust.
        let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
        let row = sessions::claim_session(&s, p.session_id, &p.person, &scope, &caller.label())
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }
}
