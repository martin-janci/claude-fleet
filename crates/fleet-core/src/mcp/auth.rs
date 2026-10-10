//! Request authorization for the embedded MCP server.
//!
//! Two layers, checked in order:
//!
//! 1. **DNS-rebinding defense** — the server binds localhost, but a remote web
//!    page can still point its own domain at `127.0.0.1` and have the victim's
//!    browser issue requests. We reject any request whose `Origin` or `Host`
//!    header names a non-loopback address. The MCP HTTP-transport spec requires
//!    `Origin` validation for exactly this reason.
//! 2. **Bearer token** — the request must carry `Authorization: Bearer <token>`
//!    matching either the master token (desktop / local clients) or one of the
//!    per-host tokens minted at provisioning (migration 018). The token that
//!    matched becomes the request's [`Caller`]: a per-host token identifies —
//!    and scopes the caller to — that host, so a token lifted from one
//!    machine cannot impersonate another, and a `readonly` host token is
//!    refused every mutating tool. A third kind of token identifies a paired
//!    *client* (a phone — migration `032_client_tokens.sql`): the DB keeps
//!    only its SHA-256, and it resolves to a caller that is deliberately
//!    NEITHER the master NOR a host, so the fleet-admin tools stay out of
//!    its reach.

use crate::store::{ApiScope, ClientTokenRow, ControlTokenRow, HostTokenRow};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};

/// What a token is allowed to do. Unknown mode strings in the DB fall back
/// to `Readonly` — fail closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenMode {
    /// Every tool.
    Full,
    /// Only tools that observe the fleet; mutating tools get `E_FORBIDDEN`.
    Readonly,
    /// Another fleet's hub (federation): `peer_exchange` and nothing else.
    /// Only a paired client row can hold it (see `parse_client`).
    Peer,
    /// `fleet-updater` acting for this hub (update-channel design §6.1):
    /// `/update/check` and `/update/report` and nothing else — no tool, no
    /// `/events`, no `/report`. Only a paired client row can hold it.
    Updater,
}

/// The one tool a `Peer` token may call, and that only a `Peer` token may
/// call.
pub(crate) const PEER_TOOL: &str = "peer_exchange";

impl TokenMode {
    /// A host token row's mode. `peer` is NOT recognised here: a host's
    /// token can never become a hub link, whatever string its row holds.
    pub fn parse(s: &str) -> TokenMode {
        match s {
            "full" => TokenMode::Full,
            _ => TokenMode::Readonly,
        }
    }

    /// A paired client row's mode: `full`, `peer`, `updater`, else
    /// `readonly`.
    fn parse_client(s: &str) -> TokenMode {
        match s {
            "peer" => TokenMode::Peer,
            "updater" => TokenMode::Updater,
            other => TokenMode::parse(other),
        }
    }

    /// A token with one door only (`peer` → `peer_exchange`, `updater` →
    /// `/update/*`): refused by every tool and route but its own.
    pub fn is_single_purpose(self) -> bool {
        matches!(self, TokenMode::Peer | TokenMode::Updater)
    }
}

/// The paired client behind a request: the `client_tokens` row that matched.
/// Only the id, the name and the trust flag travel — never the token or its
/// hash. `trusted` is `trusted_at IS NOT NULL` on the row: the operator has
/// vouched for this device, so what it sends is delivered without the
/// untrusted-content marker (`mcp::tools::apply_marker`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRef {
    pub id: i64,
    pub name: String,
    pub trusted: bool,
    /// The org this client is bound to (work graph M14, `fleet-hub pair
    /// --org`): its work and sessions are fenced to that org and unassigned
    /// data ([`crate::service::orgs::OrgScope::Org`]). `None`: unbound, every
    /// org (the org is a view, as for the master).
    pub org_id: Option<i64>,
    /// WHOSE device this is (multi-user M1, `client_tokens.person_id`): the
    /// `people` row the operator paired it for (`fleet-hub pair --person`,
    /// `fleet-hub client bind-person`). It decides which sessions this
    /// connection may see at all, so it rides on the caller rather than
    /// being looked up per tool, and a re-binding bumps the auth epoch
    /// (migration 098) so the change holds from the device's next request.
    ///
    /// `None` is a device the migration's backfill did not reach and no
    /// pairing has bound — unmintable since T2 (`pair_client` defaults to
    /// the hub's personal owner) and treated as *nobody* everywhere, never
    /// as *everybody*.
    pub person_id: Option<i64>,
}

/// The named Control API token behind a request (migration 157, M15 step
/// G2.8): the `control_tokens` row that matched. The id and name travel for
/// the "last used" stamp and audit labels; never the token or its hash.
///
/// A named token speaks for the hub's owner, as the master token does, so
/// its caller has the master's shape (no host alias, no client). What sets it
/// apart is [`Caller::is_master`]: true only for an `admin` token, so a `read`
/// or `act` token is refused every fleet-admin and settings tool, and a
/// `read` one every mutating tool through its `Readonly` mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiTokenRef {
    pub id: i64,
    pub name: String,
    pub scope: ApiScope,
    /// The hosts it may reach; `None` every host. Enforced by
    /// `ViewScope::sees_session_facts` (sessions) and `require_host`
    /// (host-addressed calls). Never set on an admin token.
    pub hosts: Option<Vec<String>>,
}

/// The authenticated identity behind a request, derived from the bearer
/// token that matched. Inserted into the request extensions by the auth
/// middleware so tools and the `/hook` handler can read it.
///
/// Exactly one of the three shapes: master (`host_alias` and `client` both
/// `None`), a host (`host_alias` set), a paired client (`client` set).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    /// `None` for the master token (desktop / local agent use — unrestricted)
    /// AND for a paired client; `Some(alias)` for a per-host token, which
    /// scopes identity-bearing tools (`register_self`, `send_message`,
    /// `inbox`) to that host.
    pub host_alias: Option<String>,
    /// `Some(_)` only for a paired client (a phone). A client is never the
    /// master: [`Caller::is_master`] — the fleet-admin gate — checks this
    /// field too, so provisioning, `add_host`/`remove_host`, `apply_sync` and
    /// `set_secret` stay unreachable from a paired device.
    pub client: Option<ClientRef>,
    pub mode: TokenMode,
    /// The tmux pane the caller is speaking from (`X-Fleet-Pane`, `%17`), as
    /// [`crate::mcp::hooks::pane_header`] validated it — multi-user M1's pane
    /// proof (plan revision 6, R6-i). It reaches the hub on the CONNECTION,
    /// through the `claude-fleet` MCP entry's `"X-Fleet-Pane":
    /// "${TMUX_PANE:-}"` header (`service::provision::merge_mcp_entry`), so
    /// every tool can evaluate "the one row whose pane it can prove it is in"
    /// rather than only the three that could have taken an argument.
    ///
    /// `None` for a caller that sends no header — a phone, a host
    /// provisioned before M1, an agent outside tmux — which proves no pane
    /// and is refused wherever the proof is what would have let it through.
    ///
    /// **It is a CLAIM, not an identity.** Any caller can put any well-formed
    /// `%N` in that header — the provisioned MCP entry is where the honest
    /// ones get it, not a channel only they have — so this field is worth
    /// nothing until it is matched against a row fleet wrote itself
    /// (`sessions.tmux_pane_id`), and it must never widen a caller's reach on
    /// its own: it can only pick out, from what the caller may already see,
    /// the one row whose pane it claims to be in. A claim that matches no row
    /// is simply no proof. What a MATCHING claim is worth is in turn bounded
    /// by the deployment: any process that can run `tmux list-panes` on the
    /// host can enumerate every pane id there, so it proves host access, not
    /// pane occupancy (see `docs/hub.md`).
    pub pane: Option<String>,
    /// Whether this caller is the hub's personal owner: the master token, or
    /// a paired device whose [`ClientRef::person_id`] is
    /// `Store::personal_owner_id()`.
    ///
    /// Answered ONCE, where the token is resolved and the store rows are
    /// already in hand, because [`crate::mcp::guard::access_allows`] — the
    /// gate that reads it — is shared with
    /// `crate::mcp::tools::present::visible_to`, which takes a `&Caller` and
    /// nothing else and runs over the whole router on every served list. A
    /// store read there would be a lock per request (R6-l).
    ///
    /// `false` on a hub that cannot say who its owner is
    /// (`personal_owner_id()` answered `None`): T1's fail-closed rule held
    /// here, so the fleet's settings are refused rather than served to
    /// whoever asked.
    pub is_personal_owner: bool,
    /// `Some(_)` for a named Control API token (see [`ApiTokenRef`]). The
    /// caller then has the master's shape; its scope decides the rest.
    pub api: Option<ApiTokenRef>,
}

impl Caller {
    /// The master-token caller: no host binding, no client, full mode — and
    /// the hub's personal owner, which is what the master token IS on a hub
    /// that knows whose it is. [`resolve_token`] clears that flag on a hub
    /// whose `people` row is missing (T1's fail-closed rule), so this
    /// constructor is the healthy shape rather than an assumption.
    pub fn master() -> Self {
        Caller {
            host_alias: None,
            client: None,
            mode: TokenMode::Full,
            pane: None,
            is_personal_owner: true,
            api: None,
        }
    }

    /// True only for the master token. A paired client carries no host alias
    /// either, so the client field must be checked as well — this is the one
    /// gate that keeps the fleet-admin tools master-only.
    ///
    /// A named token with the `admin` scope counts: it is what the master
    /// token is (G2.8). A `read` or `act` one does not.
    pub fn is_master(&self) -> bool {
        self.host_alias.is_none()
            && self.client.is_none()
            && self.api.as_ref().is_none_or(|a| a.scope == ApiScope::Admin)
    }

    /// True for the master token and for every named token: a caller that
    /// speaks for the hub's owner without being a paired device. The person
    /// behind it is `Store::personal_owner_id()`.
    pub fn speaks_for_owner(&self) -> bool {
        self.host_alias.is_none() && self.client.is_none()
    }

    /// The host a named token is limited to reaching, `None` for every host
    /// (and for every other kind of caller).
    pub fn api_hosts(&self) -> Option<&[String]> {
        self.api.as_ref().and_then(|a| a.hosts.as_deref())
    }

    /// True for a paired client (a phone), whatever its mode.
    pub fn is_client(&self) -> bool {
        self.client.is_some()
    }

    /// True for a person's own paired device: a paired client bound to no
    /// org, and not a machine token. The fleet-wide settings tools
    /// ([`crate::mcp::guard::Access::Person`] and
    /// [`crate::mcp::guard::Access::PersonDevice`]) answer it; a per-host
    /// token or an org-bound client is never one.
    ///
    /// The mode test is [`TokenMode::is_single_purpose`], which is exactly
    /// this distinction and covers `Updater` as well as `Peer` — an updater
    /// token is `fleet-updater` acting for this hub, not a human's phone, and
    /// spelling out one of the two modes let it through every gate that keys
    /// on a person's device. This predicate answers WHAT the caller is, never
    /// WHOSE it is: a gate that means "the hub's owner's own device" reads
    /// [`Caller::is_personal_owner`] alongside it.
    pub fn is_person_device(&self) -> bool {
        self.host_alias.is_none()
            && !self.mode.is_single_purpose()
            && self.client.as_ref().is_some_and(|c| c.org_id.is_none())
    }

    /// WHO this caller is, as a `people` row id (multi-user M1): the person
    /// whose device it is.
    ///
    /// `None` for the master token — whose person is the hub's personal
    /// owner, a store read rather than something the connection carries; see
    /// `crate::mcp::tools::fleet::owner_for`, which is the one place that
    /// mapping is made — and `None` for a per-host token, which is an agent
    /// on a machine and not a person at all.
    pub fn person(&self) -> Option<i64> {
        self.client.as_ref().and_then(|c| c.person_id)
    }

    /// True for the UX agent's operator session: the paired client token
    /// `ensure_operator` mints under [`OPERATOR_CLIENT_NAME`]. Its session
    /// starts and kills always need a person's approval (work graph M9.7,
    /// decision D12).
    ///
    /// [`OPERATOR_CLIENT_NAME`]: crate::service::operator::OPERATOR_CLIENT_NAME
    pub fn is_operator(&self) -> bool {
        self.host_alias.is_none()
            && self
                .client
                .as_ref()
                .is_some_and(|c| c.name == crate::service::operator::OPERATOR_CLIENT_NAME)
    }

    /// Who a work-link decision by this caller is (D34 label hygiene): a
    /// per-host token is the host's own Claude and the operator is the UX
    /// agent, so both are an AGENT, and their links and confirmations are
    /// recorded as `agent` whatever `source` they pass. The master and a
    /// paired person's client (a phone, a paired desktop) are a PERSON.
    pub fn work_decider(&self) -> crate::store::Decider {
        if self.host_alias.is_some() || self.is_operator() || self.mode == TokenMode::Peer {
            crate::store::Decider::Agent
        } else {
            crate::store::Decider::Person
        }
    }

    /// True for a paired client the operator has vouched for
    /// (`client_tokens.trusted_at` set): its text is the operator's own, so
    /// the untrusted-content marker is left off. Never true for the master
    /// (which has `raw` for that) or a per-host token.
    pub fn is_trusted_client(&self) -> bool {
        self.client.as_ref().is_some_and(|c| c.trusted)
    }

    /// The work graph's org scope for this caller (M5) — the ONE place a
    /// caller becomes a scope. The master and an unbound paired client read
    /// every org (the org is a view there); a per-host token is bounded by
    /// its host's org, read from the store now, so a host moved by the
    /// master is fenced from its next call on; a client bound to an org
    /// (M14) is bounded by that org. The binding travels in the cached
    /// caller, and a re-bind bumps the auth epoch (migration 066), so a
    /// re-bound client is fenced from its next request on.
    pub fn org_scope(
        &self,
        store: &crate::store::Store,
    ) -> Result<crate::service::orgs::OrgScope, crate::ipc_error::IpcError> {
        match (&self.host_alias, &self.client) {
            (Some(h), _) => crate::service::orgs::OrgScope::for_host(store, h),
            (
                None,
                Some(ClientRef {
                    org_id: Some(org), ..
                }),
            ) => crate::service::orgs::OrgScope::for_client(store, *org),
            (None, _) => Ok(crate::service::orgs::OrgScope::All),
        }
    }

    /// The multi-user scope for this caller (M1, T6) — the ONE place a
    /// caller becomes a [`ViewScope`], beside (never replacing)
    /// [`Self::org_scope`], whose answer it wraps.
    ///
    /// | Caller | `person` | `host` | Sees |
    /// |---|---|---|---|
    /// | master | `Store::personal_owner_id()` | — | its own rows, its grants, its org rows — and a REFUSING scope when that answers `None` |
    /// | client with a `person_id` | that person | — | as above |
    /// | client with no `person_id` | `None` | — | a REFUSING scope |
    /// | per-host token | `None` | `Some(alias)` | `unclaimed` rows on its own host, plus the one row this request's pane proves |
    ///
    /// Three things this function is careful about, each of them a hole if
    /// written the obvious way:
    ///
    /// * **`person: None` refuses, it does not widen.** The master's person
    ///   is a store read, not something the connection carries, and T1's
    ///   rule is that a hub which cannot say whose it is serves nobody
    ///   rather than everybody. A per-host token has no person at all — a
    ///   machine is not a person — and reaches its rows through
    ///   [`ViewScope::sees_session_row`]'s host clauses instead. The hub's
    ///   own readers never come through here: they build
    ///   [`ViewScope::internal`] directly, which is why "the hub does its
    ///   work" and "a caller sees everything" are no longer one value.
    /// * **The pane proof is resolved here, once per request**, and only
    ///   for a per-host token: for anyone else it could only *add* a row to
    ///   what they may already see, and `Caller::pane` is a claim any
    ///   caller can write, never an identity. `find_session_by_pane`
    ///   already filters on the host, excludes ghosts and answers `None` on
    ///   an ambiguous pane, so nothing here has to re-derive that.
    /// * **Everything is read off the handle the caller passes in.** A
    ///   scope built through the read pool while the rows come from the
    ///   writer races a grant created between the two — which is a revoked
    ///   share still being served, or a fresh one not yet honoured.
    ///
    /// [`ViewScope`]: crate::service::view_scope::ViewScope
    /// [`ViewScope::internal`]: crate::service::view_scope::ViewScope::internal
    /// [`ViewScope::sees_session_row`]: crate::service::view_scope::ViewScope::sees_session_row
    pub fn view_scope(
        &self,
        store: &crate::store::Store,
    ) -> Result<crate::service::view_scope::ViewScope, crate::ipc_error::IpcError> {
        use crate::service::view_scope::{GrantSet, ViewScope};
        let org = self.org_scope(store)?;
        let person = match (&self.host_alias, &self.client) {
            // A machine's token speaks for a machine.
            (Some(_), _) => None,
            // Whose device this is; `None` on a device no pairing bound,
            // which is a refusing scope and not a privileged one.
            (None, Some(c)) => c.person_id,
            // The master: the hub's personal owner, or nobody.
            (None, None) => store.personal_owner_id()?,
        };
        let grants = match person {
            Some(p) => GrantSet::from_map(store.grants_for_person(p)?),
            None => GrantSet::default(),
        };
        let proven_session = match (&self.host_alias, &self.pane) {
            (Some(alias), Some(pane)) => store.find_session_by_pane(alias, pane)?.map(|row| row.id),
            _ => None,
        };
        // R5-d, and the rule that keeps a single-person install whole: the
        // one live person on the hub still sees the `unclaimed` rows they
        // could see yesterday. False for a person-less caller, so it can
        // never widen one.
        let sole_person =
            matches!((person, store.sole_enabled_person()?), (Some(p), Some(o)) if p == o);
        // Org administration phase D: who administers a host (owner's answer
        // 1) and the per-org switch (answer 3). A person-less caller —
        // a per-host token — is served no count beyond its own rows.
        let unclaimed = match person {
            Some(p) => crate::service::org_admin::unclaimed_reach(store, p)?,
            None => crate::service::view_scope::UnclaimedReach::None,
        };
        Ok(ViewScope::for_caller(
            org,
            person,
            grants,
            self.host_alias.clone(),
            proven_session,
            sole_person,
            unclaimed,
        )
        .with_hosts(self.api_hosts().map(<[String]>::to_vec)))
    }

    /// True when this caller reads through an ORG boundary: a per-host
    /// token, or a paired client bound to an org (work graph M14).
    ///
    /// It is NOT "is this caller restricted at all", and must never be used
    /// as one: it is false for the master and for every paired client bound
    /// to no org, both of which multi-user M1 restricts by PERSON. The
    /// result gate used to hang off this predicate and therefore never ran
    /// for a person's own phone (`tools/support.rs::fence_result_via`), and
    /// `require_visible_session` / the old `resolve_reader` had the same
    /// hole; a gate that means "restricted" reads
    /// [`Caller::view_scope`] instead.
    pub fn is_scoped(&self) -> bool {
        self.host_alias.is_some() || self.client.as_ref().is_some_and(|c| c.org_id.is_some())
    }

    /// Short identity label for audit rows and rate-limit buckets.
    pub fn label(&self) -> String {
        match (&self.host_alias, &self.client) {
            (Some(h), _) => format!("host:{h}"),
            (None, Some(c)) => format!("client:{}", c.name),
            (None, None) => match &self.api {
                Some(a) => format!("token:{}", a.name),
                None => "master".to_string(),
            },
        }
    }
}

/// Lowercase-hex SHA-256 of a token. `client_tokens` stores only this, so a
/// stolen database hands out no usable bearer token.
pub fn sha256_hex(s: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(s.as_bytes()))
}

/// Constant-time byte comparison. Returns `false` immediately on a length
/// mismatch — the token length is fixed and not itself a secret — and runs
/// in time independent of *where* two equal-length inputs first differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Extract the token from an `Authorization` header value. Accepts only the
/// exact form `Bearer <token>`.
pub fn bearer_token(header: Option<&HeaderValue>) -> Option<&str> {
    let text = header?.to_str().ok()?;
    let token = text.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Map a presented token to its [`Caller`]: the master token → unrestricted;
/// a per-host token → that host with its stored mode; a paired client's token
/// (matched against the stored SHA-256) → that client; anything else → `None`.
/// Every candidate is compared in constant time and the scan never
/// short-circuits, so timing does not reveal which (if any) token matched.
///
/// `client_tokens` must be the *live* rows — `Store::active_client_tokens`,
/// which drops revoked ones — so a revoked pairing can never resolve.
///
/// `personal_owner` is `Store::personal_owner_id()` (multi-user M1), read
/// beside the token rows and passed in rather than looked up here: this
/// function has no store, and [`Caller::is_personal_owner`] must be answered
/// exactly once per request, where the rows already are. `None` — a hub that
/// cannot say whose it is — makes NOBODY the personal owner, the master
/// included: fail closed, loudly, rather than treating an unknown owner as
/// everybody.
pub fn resolve_token(
    presented: &str,
    master: &str,
    host_tokens: &[HostTokenRow],
    client_tokens: &[ClientTokenRow],
    personal_owner: Option<i64>,
) -> Option<Caller> {
    resolve_token_at(
        presented,
        master,
        host_tokens,
        client_tokens,
        &[],
        personal_owner,
        crate::store::now_unix(),
    )
}

/// [`resolve_token`] with the named Control API tokens (migration 157) and
/// the time to judge their expiry by. A named token matches by its SHA-256,
/// in the same no-short-circuit scan; one past its `expires_at` at `now`
/// resolves to nobody, so an expiry needs no write and no cache rebuild.
pub fn resolve_token_at(
    presented: &str,
    master: &str,
    host_tokens: &[HostTokenRow],
    client_tokens: &[ClientTokenRow],
    control_tokens: &[ControlTokenRow],
    personal_owner: Option<i64>,
    now: i64,
) -> Option<Caller> {
    let mut found: Option<Caller> = None;
    if !master.is_empty() && constant_time_eq(presented.as_bytes(), master.as_bytes()) {
        found = Some(Caller {
            is_personal_owner: personal_owner.is_some(),
            ..Caller::master()
        });
    }
    for row in host_tokens {
        if !row.token.is_empty() && constant_time_eq(presented.as_bytes(), row.token.as_bytes()) {
            found = Some(Caller {
                host_alias: Some(row.host_alias.clone()),
                client: None,
                mode: TokenMode::parse(&row.mode),
                pane: None,
                // A machine's token is never a person, so it is never the
                // person who owns this hub.
                is_personal_owner: false,
                api: None,
            });
        }
    }
    // Hash once, then compare every stored digest — same no-short-circuit
    // shape as above.
    let presented_sha = sha256_hex(presented);
    for row in client_tokens {
        if !row.token_sha256.is_empty()
            && constant_time_eq(presented_sha.as_bytes(), row.token_sha256.as_bytes())
        {
            found = Some(Caller {
                host_alias: None,
                client: Some(ClientRef {
                    id: row.id,
                    name: row.name.clone(),
                    trusted: row.trusted_at.is_some(),
                    org_id: row.org_id,
                    person_id: row.person_id,
                }),
                mode: TokenMode::parse_client(&row.mode),
                pane: None,
                is_personal_owner: is_the_personal_owner(row.person_id, personal_owner),
                api: None,
            });
        }
    }
    for row in control_tokens {
        if !row.token_sha256.is_empty()
            && constant_time_eq(presented_sha.as_bytes(), row.token_sha256.as_bytes())
        {
            found = if row.revoked_at.is_some() || row.expired_at(now) {
                // A match that may no longer be used is nobody, not a
                // fall-through to another row.
                None
            } else {
                Some(Caller {
                    host_alias: None,
                    client: None,
                    mode: match row.scope {
                        ApiScope::Read => TokenMode::Readonly,
                        ApiScope::Act | ApiScope::Admin => TokenMode::Full,
                    },
                    pane: None,
                    // The owner's token, as the master is: true exactly when
                    // the hub can say who its owner is (T1).
                    is_personal_owner: personal_owner.is_some(),
                    api: Some(ApiTokenRef {
                        id: row.id,
                        name: row.name.clone(),
                        scope: row.scope,
                        // An admin row never has a limit (the table's CHECK);
                        // dropped here too so `is_master` and a limit can
                        // never meet.
                        hosts: match row.scope {
                            ApiScope::Admin => None,
                            _ => row.hosts.clone(),
                        },
                    }),
                })
            };
        }
    }
    found
}

/// Whether a paired device's `person_id` is the hub's personal owner.
///
/// Spelled out rather than written `person_id == personal_owner`, because two
/// `None`s must NOT compare equal here: a device bound to nobody on a hub
/// that knows nobody would otherwise resolve as the owner's own and reach the
/// whole fleet's settings.
fn is_the_personal_owner(person_id: Option<i64>, personal_owner: Option<i64>) -> bool {
    matches!((person_id, personal_owner), (Some(p), Some(o)) if p == o)
}

/// True if `value` (a `Host`-header authority — `host` or `host:port`, IPv6
/// in brackets) names the local machine.
///
/// Splitting the authority is this function's job; deciding what counts as
/// the local machine is [`fleet_proto::net::is_loopback`]'s, shared with the
/// agent, the hub's bind check and the desktop.
pub fn is_loopback_host(value: &str) -> bool {
    fleet_proto::net::is_loopback(&authority_host(value))
}

/// True if an `Origin` header value is a loopback `http(s)` origin. Anything
/// else — a remote origin, the opaque `null` origin, a non-http scheme — is
/// treated as cross-origin and rejected.
// Kept as a public, independently-tested special case of `origin_allowed`
// (empty allowlist) even though production code now calls `check_origin`
// directly; not currently called outside its own test.
#[allow(dead_code)]
pub fn origin_is_loopback(origin: &str) -> bool {
    origin_allowed(origin, &[])
}

/// Lower-cased, trimmed allowlist entries (`host` or `host:port`), empties
/// dropped. Built once at server start from the hub's configuration.
pub fn normalize_allowed_hosts(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The host part of a `Host`-header authority: brackets and port stripped,
/// lower-cased.
fn authority_host(value: &str) -> String {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_ascii_lowercase();
    }
    v.split(':').next().unwrap_or(v).to_ascii_lowercase()
}

/// True when `value` is loopback or names an allowlisted host, matched as
/// the full authority (`host:port`) or as the bare host.
fn host_allowed(value: &str, allowed: &[String]) -> bool {
    if is_loopback_host(value) {
        return true;
    }
    let full = value.trim().to_ascii_lowercase();
    let host = authority_host(value);
    allowed.iter().any(|a| *a == full || *a == host)
}

/// True when an `Origin` is a loopback `http(s)` origin or one whose
/// authority is allowlisted.
fn origin_allowed(origin: &str, allowed: &[String]) -> bool {
    let after_scheme = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"));
    match after_scheme {
        Some(rest) => host_allowed(rest.split('/').next().unwrap_or(rest), allowed),
        None => false,
    }
}

/// Layer 1 — DNS-rebinding defense. An `Origin`/`Host` is validated only when
/// present; a non-browser MCP client legitimately omits `Origin`. Loopback is
/// always accepted; a hub exposed at a public URL adds that URL's host to
/// `allowed`. `Err(403)` on anything else.
///
/// `allowed` must ALREADY be normalized through [`normalize_allowed_hosts`],
/// as `mcp::start` does once before handing it to the `AuthState`: the
/// comparison here is exact, so an un-normalized entry (a scheme, a trailing
/// slash, mixed case) simply never matches.
pub fn check_origin(headers: &HeaderMap, allowed: &[String]) -> Result<(), StatusCode> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin
            .to_str()
            .map(|o| origin_allowed(o, allowed))
            .unwrap_or(false)
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    if let Some(host) = headers.get(header::HOST) {
        if !host
            .to_str()
            .map(|h| host_allowed(h, allowed))
            .unwrap_or(false)
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

/// Authorize an incoming request and identify its caller. `Err` carries the
/// status to return: `403` for a cross-origin / DNS-rebinding attempt, `401`
/// for a missing or unknown bearer token. `control_tokens` are the named
/// Control API tokens (migration 157). `allowed` must already be normalized
/// ([`normalize_allowed_hosts`]) — see [`check_origin`]. `personal_owner` is
/// the hub's `people` row id, read beside the token rows — see
/// [`resolve_token`] for why it is an argument and not a lookup.
pub fn check_request(
    headers: &HeaderMap,
    master_token: &str,
    host_tokens: &[HostTokenRow],
    client_tokens: &[ClientTokenRow],
    control_tokens: &[ControlTokenRow],
    allowed: &[String],
    personal_owner: Option<i64>,
) -> Result<Caller, StatusCode> {
    check_origin(headers, allowed)?;
    let presented =
        bearer_token(headers.get(header::AUTHORIZATION)).ok_or(StatusCode::UNAUTHORIZED)?;
    resolve_token_at(
        presented,
        master_token,
        host_tokens,
        client_tokens,
        control_tokens,
        personal_owner,
        crate::store::now_unix(),
    )
    .ok_or(StatusCode::UNAUTHORIZED)
}

/// The `Peer` gate shared by `/events` and `/report`: neither route is
/// `peer_exchange`, so a hub link's token must be refused before either does
/// any work. `/mcp`'s own gate is `enforce_mode` in `tools/support.rs`; this
/// is the same rule for the two routes that sit outside the tool router but
/// still take a `Caller` from the request extensions. A shared fn rather than
/// the check inlined twice, since the two routes disagreeing about who is a
/// peer is exactly the kind of drift this exists to prevent.
///
/// Returns the `403` to send when the caller must be refused, or `None` when
/// it may proceed.
pub(crate) fn refuses_peer(caller: &Caller) -> Option<axum::response::Response> {
    use axum::response::IntoResponse;
    let body = match caller.mode {
        TokenMode::Peer => "a hub link may call peer_exchange only\n",
        // Same rule for the updater's token: its only door is `/update/*`.
        TokenMode::Updater => "an updater token may call /update only\n",
        TokenMode::Full | TokenMode::Readonly => return None,
    };
    Some((StatusCode::FORBIDDEN, body).into_response())
}

/// One PERSON's own device, as a [`ViewScope`] — a paired client bound to no
/// org, which is the caller multi-user M1 exists to fence.
///
/// Test-only, and it lives here rather than in each test module because this
/// file holds the ONE constructor from a request
/// (`view_scope_tests::only_caller_view_scope_constructs_a_view_scope` keeps
/// it that way): it builds a `Caller` and asks `Caller::view_scope`, so a
/// service-layer test gets the real thing — grants read from the store,
/// `sole_person` answered by the store — instead of a hand-built struct that
/// could disagree with the resolver.
#[cfg(test)]
pub(crate) fn device_view(
    store: &crate::store::Store,
    person: i64,
) -> crate::service::view_scope::ViewScope {
    Caller {
        api: None,
        host_alias: None,
        client: Some(ClientRef {
            id: 11,
            name: "phone".into(),
            trusted: false,
            org_id: None,
            person_id: Some(person),
        }),
        mode: TokenMode::Full,
        pane: None,
        is_personal_owner: false,
    }
    .view_scope(store)
    .expect("the scope reads")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hub's personal owner as these tests see it (multi-user M1). Any
    /// id would do: what every assertion turns on is whether a caller's
    /// person IS this one, so one constant named once beats a literal
    /// repeated at forty call sites.
    const OWNER: Option<i64> = Some(1);

    /// A hub that cannot say whose it is — `Store::personal_owner_id()`
    /// answered `None`. Nobody is the personal owner then, the master
    /// included (T1's fail-closed rule).
    const NO_OWNER: Option<i64> = None;

    fn host_row(alias: &str, token: &str, mode: &str) -> HostTokenRow {
        HostTokenRow {
            last_used_at: None,
            rotated_at: None,
            host_alias: alias.into(),
            token: token.into(),
            created_at: 0,
            mode: mode.into(),
        }
    }

    #[test]
    fn a_trusted_row_resolves_to_a_trusted_client_ref() {
        let mut row = client_row(3, "mac-desktop", "tok", "full");
        row.trusted_at = Some(1_700_000_000);
        let c = resolve_token("tok", "master", &[], &[row], OWNER).expect("resolves");
        assert!(c.is_client() && !c.is_master());
        assert!(c.is_trusted_client());
        assert!(c.client.as_ref().unwrap().trusted);
        let plain = resolve_token(
            "tok",
            "master",
            &[],
            &[client_row(3, "phone", "tok", "full")],
            OWNER,
        )
        .unwrap();
        assert!(!plain.is_trusted_client());
        assert!(!Caller::master().is_trusted_client());
    }

    fn client_row(id: i64, name: &str, token: &str, mode: &str) -> ClientTokenRow {
        ClientTokenRow {
            id,
            name: name.into(),
            token_sha256: sha256_hex(token),
            mode: mode.into(),
            created_at: 0,
            last_seen_at: None,
            revoked_at: None,
            trusted_at: None,
            org_id: None,
            assets_admin_at: None,
            person_id: None,
        }
    }

    #[test]
    fn a_client_token_resolves_to_a_client_caller_that_is_not_master() {
        let rows = vec![client_row(7, "phone", "tok-phone", "full")];
        let c = resolve_token("tok-phone", "s3cret", &[], &rows, OWNER).unwrap();
        assert!(!c.is_master(), "a client must never count as the master");
        assert!(c.is_client());
        assert_eq!(c.label(), "client:phone");
        assert_eq!(c.mode, TokenMode::Full);
        assert_eq!(c.client.as_ref().unwrap().id, 7);
        assert!(c.host_alias.is_none());
    }

    /// D34: a per-host token (the host's Claude) and the operator (the UX
    /// agent) decide work links as an agent; the master and a paired
    /// person's client as a person.
    #[test]
    fn host_tokens_and_the_operator_decide_work_as_agents() {
        use crate::store::Decider;
        let clients = vec![
            client_row(1, "phone", "tok-phone", "full"),
            client_row(
                2,
                crate::service::operator::OPERATOR_CLIENT_NAME,
                "tok-op",
                "full",
            ),
        ];
        let hosts = vec![host_row("mefistos", "tok-mef", "full")];
        let who = |tok: &str| {
            resolve_token(tok, "s3cret", &hosts, &clients, OWNER)
                .unwrap()
                .work_decider()
        };
        assert_eq!(who("s3cret"), Decider::Person);
        assert_eq!(who("tok-phone"), Decider::Person);
        assert_eq!(who("tok-mef"), Decider::Agent);
        assert_eq!(who("tok-op"), Decider::Agent);
    }

    #[test]
    fn a_readonly_client_keeps_its_mode_and_an_unknown_token_resolves_to_nothing() {
        let rows = vec![client_row(1, "tablet", "tok-t", "readonly")];
        assert_eq!(
            resolve_token("tok-t", "s3cret", &[], &rows, OWNER)
                .unwrap()
                .mode,
            TokenMode::Readonly
        );
        assert!(resolve_token("nope", "s3cret", &[], &rows, OWNER).is_none());
    }

    #[test]
    fn the_master_and_host_tokens_still_resolve_with_clients_present() {
        let clients = vec![client_row(1, "phone", "tok-phone", "full")];
        let hosts = vec![host_row("mefistos", "tok-mef", "full")];
        assert_eq!(
            resolve_token("s3cret", "s3cret", &hosts, &clients, OWNER).unwrap(),
            Caller::master()
        );
        let h = resolve_token("tok-mef", "s3cret", &hosts, &clients, OWNER).unwrap();
        assert_eq!(h.host_alias.as_deref(), Some("mefistos"));
        assert!(!h.is_client());
    }

    #[test]
    fn sha256_hex_is_lowercase_hex_of_the_token() {
        // Known vector: SHA-256 of "abc".
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_client_token_never_authorizes_as_the_master_through_check_request() {
        let clients = vec![client_row(3, "phone", "tok-phone", "full")];
        let h = headers(&[
            ("host", "127.0.0.1:4180"),
            ("authorization", "Bearer tok-phone"),
        ]);
        let c = check_request(&h, "s3cret", &[], &clients, &[], &[], OWNER).unwrap();
        assert!(!c.is_master());
        assert!(c.is_client());
        assert_eq!(c.label(), "client:phone");
    }

    #[test]
    fn constant_time_eq_matches_identical_and_rejects_others() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn bearer_token_extracts_exact_bearer_form_only() {
        let h = HeaderValue::from_static("Bearer s3cret");
        assert_eq!(bearer_token(Some(&h)), Some("s3cret"));
        let padded = HeaderValue::from_static("Bearer  s3cret ");
        assert_eq!(bearer_token(Some(&padded)), Some("s3cret"));
        assert!(bearer_token(None).is_none());
        let no_scheme = HeaderValue::from_static("s3cret");
        assert!(bearer_token(Some(&no_scheme)).is_none());
        let basic = HeaderValue::from_static("Basic s3cret");
        assert!(bearer_token(Some(&basic)).is_none());
        let empty = HeaderValue::from_static("Bearer ");
        assert!(bearer_token(Some(&empty)).is_none());
    }

    #[test]
    fn resolve_token_maps_master_and_host_tokens_to_callers() {
        let hosts = [
            host_row("mefistos", "tok-mef", "full"),
            host_row("turanga", "tok-tur", "readonly"),
            host_row("weird", "tok-weird", "not-a-mode"),
        ];
        assert_eq!(
            resolve_token("master-tok", "master-tok", &hosts, &[], OWNER),
            Some(Caller::master())
        );
        assert_eq!(
            resolve_token("tok-mef", "master-tok", &hosts, &[], OWNER),
            Some(Caller {
                api: None,
                host_alias: Some("mefistos".into()),
                client: None,
                mode: TokenMode::Full,
                pane: None,
                is_personal_owner: false,
            })
        );
        assert_eq!(
            resolve_token("tok-tur", "master-tok", &hosts, &[], OWNER),
            Some(Caller {
                api: None,
                host_alias: Some("turanga".into()),
                client: None,
                mode: TokenMode::Readonly,
                pane: None,
                is_personal_owner: false,
            })
        );
        // Unknown mode strings fail closed to readonly.
        assert_eq!(
            resolve_token("tok-weird", "master-tok", &hosts, &[], OWNER)
                .unwrap()
                .mode,
            TokenMode::Readonly
        );
        assert_eq!(
            resolve_token("nope", "master-tok", &hosts, &[], OWNER),
            None
        );
        // An empty configured token never matches an empty presented one.
        assert_eq!(
            resolve_token("", "", &[host_row("h", "", "full")], &[], OWNER),
            None
        );
        // …nor an empty hash on a client row.
        assert_eq!(
            resolve_token(
                "",
                "",
                &[],
                &[ClientTokenRow {
                    id: 1,
                    name: "c".into(),
                    token_sha256: String::new(),
                    mode: "full".into(),
                    created_at: 0,
                    last_seen_at: None,
                    revoked_at: None,
                    trusted_at: None,
                    org_id: None,
                    assets_admin_at: None,
                    person_id: None,
                }],
                OWNER,
            ),
            None
        );
    }

    #[test]
    fn only_a_client_row_can_be_a_peer() {
        assert_eq!(TokenMode::parse_client("peer"), TokenMode::Peer);
        assert_eq!(TokenMode::parse_client("full"), TokenMode::Full);
        assert_eq!(TokenMode::parse_client("readonly"), TokenMode::Readonly);
        assert_eq!(
            TokenMode::parse_client("anything-else"),
            TokenMode::Readonly
        );
        // A host token row is parsed with `parse`: `peer` there is unknown and
        // fails closed, so an agent's token can never reach `peer_exchange`.
        assert_eq!(TokenMode::parse("peer"), TokenMode::Readonly);
    }

    #[test]
    fn a_peer_mode_row_resolves_only_through_a_client_row() {
        // A host token row whose `mode` column somehow holds "peer" still
        // resolves as `Readonly` — `resolve_token`'s host loop uses `parse`,
        // not `parse_client`.
        let hosts = [host_row("hub-a", "tok-hub", "peer")];
        assert_eq!(
            resolve_token("tok-hub", "master-tok", &hosts, &[], OWNER)
                .unwrap()
                .mode,
            TokenMode::Readonly
        );
        // A client row with mode "peer" resolves to `TokenMode::Peer`.
        let clients = vec![client_row(9, "hub-b", "tok-client", "peer")];
        let c = resolve_token("tok-client", "master-tok", &[], &clients, OWNER).unwrap();
        assert_eq!(c.mode, TokenMode::Peer);
        assert!(c.is_client());
        assert!(!c.is_master());
    }

    #[test]
    fn caller_labels_and_master_flag() {
        assert!(Caller::master().is_master());
        assert_eq!(Caller::master().label(), "master");
        assert!(!Caller::master().is_client());
        let c = Caller {
            api: None,
            host_alias: Some("mefistos".into()),
            client: None,
            mode: TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        };
        assert!(!c.is_master());
        assert!(!c.is_client());
        assert_eq!(c.label(), "host:mefistos");
        assert_eq!(TokenMode::parse("full"), TokenMode::Full);
        assert_eq!(TokenMode::parse("readonly"), TokenMode::Readonly);
        assert_eq!(TokenMode::parse("anything-else"), TokenMode::Readonly);
    }

    #[test]
    fn is_loopback_host_accepts_local_forms() {
        for h in [
            "127.0.0.1",
            "127.0.0.1:4180",
            // The whole of 127.0.0.0/8, since the rule moved to
            // `fleet_proto::net`. A `Host` header that is an IP literal
            // cannot be an attacker's DNS-rebinding name, so widening from
            // the single address costs nothing: these really are this
            // machine.
            "127.0.0.53",
            "127.0.0.53:4180",
            "127.255.255.255",
            "localhost",
            "localhost:4180",
            "LocalHost:4180",
            "[::1]",
            "[::1]:4180",
        ] {
            assert!(is_loopback_host(h), "should accept {h}");
        }
    }

    #[test]
    fn is_loopback_host_rejects_remote_forms() {
        for h in [
            "evil.com",
            "evil.com:4180",
            "127.0.0.1.evil.com",
            "10.0.0.5",
            "0.0.0.0",
            // A name under `localhost` is a DNS lookup on a resolver that
            // does not honour RFC 6761, so it is not this machine.
            "evil.localhost",
            "evil.localhost:4180",
            // An IPv4-mapped v6 address is routable, not `::1`.
            "[::ffff:127.0.0.1]",
            "[::ffff:127.0.0.1]:4180",
        ] {
            assert!(!is_loopback_host(h), "should reject {h}");
        }
    }

    #[test]
    fn origin_is_loopback_accepts_local_and_rejects_remote() {
        assert!(origin_is_loopback("http://127.0.0.1:4180"));
        assert!(origin_is_loopback("http://localhost:4180"));
        assert!(origin_is_loopback("https://[::1]"));
        assert!(!origin_is_loopback("http://evil.com"));
        assert!(!origin_is_loopback("https://evil.com:4180"));
        assert!(!origin_is_loopback("null"));
        assert!(!origin_is_loopback("file://"));
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn check_request_allows_local_request_with_token() {
        let h = headers(&[
            ("host", "127.0.0.1:4180"),
            ("authorization", "Bearer s3cret"),
        ]);
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[], &[], OWNER),
            Ok(Caller::master())
        );
    }

    #[test]
    fn check_request_identifies_host_token_callers() {
        let h = headers(&[("authorization", "Bearer tok-mef")]);
        let caller = check_request(
            &h,
            "s3cret",
            &[host_row("mefistos", "tok-mef", "readonly")],
            &[],
            &[],
            &[],
            OWNER,
        )
        .unwrap();
        assert_eq!(caller.host_alias.as_deref(), Some("mefistos"));
        assert_eq!(caller.mode, TokenMode::Readonly);
    }

    #[test]
    fn check_request_allows_non_browser_client_without_origin() {
        // A CLI MCP client sends no Origin — only the token gates it.
        let h = headers(&[("authorization", "Bearer s3cret")]);
        assert!(check_request(&h, "s3cret", &[], &[], &[], &[], OWNER).is_ok());
    }

    #[test]
    fn check_request_rejects_wrong_token_with_401() {
        let h = headers(&[("host", "127.0.0.1:4180"), ("authorization", "Bearer nope")]);
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[], &[], OWNER),
            Err(StatusCode::UNAUTHORIZED)
        );
        let none = headers(&[("host", "127.0.0.1:4180")]);
        assert_eq!(
            check_request(&none, "s3cret", &[], &[], &[], &[], OWNER),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn check_request_rejects_remote_origin_with_403() {
        // DNS-rebinding attempt: a remote page's Origin, even with a token.
        let h = headers(&[
            ("host", "127.0.0.1:4180"),
            ("origin", "http://evil.com"),
            ("authorization", "Bearer s3cret"),
        ]);
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[], &[], OWNER),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(check_origin(&h, &[]), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn check_request_rejects_rebound_host_with_403() {
        // Host header carrying the attacker's domain (rebound to 127.0.0.1).
        let h = headers(&[("host", "evil.com"), ("authorization", "Bearer s3cret")]);
        assert_eq!(
            check_request(&h, "s3cret", &[], &[], &[], &[], OWNER),
            Err(StatusCode::FORBIDDEN)
        );
    }

    fn allow_headers(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            h.insert(header::ORIGIN, o.parse().unwrap());
        }
        h.insert(header::AUTHORIZATION, "Bearer s3cret".parse().unwrap());
        h
    }

    #[test]
    fn allowlisted_host_and_origin_pass_others_still_403() {
        let allowed = normalize_allowed_hosts(&["Fleet.Example.com".into()]);
        // Bare host and port-qualified authority both match, case-insensitively.
        assert!(check_request(
            &allow_headers("fleet.example.com", None),
            "s3cret",
            &[],
            &[],
            &[],
            &allowed,
            OWNER,
        )
        .is_ok());
        assert!(check_request(
            &allow_headers("FLEET.example.com:443", None),
            "s3cret",
            &[],
            &[],
            &[],
            &allowed,
            OWNER,
        )
        .is_ok());
        assert!(check_request(
            &allow_headers("fleet.example.com", Some("https://fleet.example.com")),
            "s3cret",
            &[],
            &[],
            &[],
            &allowed,
            OWNER,
        )
        .is_ok());
        // Loopback keeps working with a non-empty list.
        assert!(check_request(
            &allow_headers("127.0.0.1:4180", None),
            "s3cret",
            &[],
            &[],
            &[],
            &allowed,
            OWNER,
        )
        .is_ok());
        // Not listed → 403 before the token is looked at.
        assert_eq!(
            check_request(
                &allow_headers("evil.example.com", None),
                "s3cret",
                &[],
                &[],
                &[],
                &allowed,
                OWNER,
            ),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            check_request(
                &allow_headers("fleet.example.com", Some("https://evil.example.com")),
                "s3cret",
                &[],
                &[],
                &[],
                &allowed,
                OWNER,
            ),
            Err(StatusCode::FORBIDDEN)
        );
        // An empty list is today's behaviour: loopback only.
        assert_eq!(
            check_request(
                &allow_headers("fleet.example.com", None),
                "s3cret",
                &[],
                &[],
                &[],
                &[],
                OWNER,
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn normalize_allowed_hosts_trims_lowercases_and_drops_empties() {
        let out = normalize_allowed_hosts(&[" A.Example.com ".into(), "".into(), "b:8443".into()]);
        assert_eq!(out, vec!["a.example.com".to_string(), "b:8443".to_string()]);
    }

    /// Multi-user M1: the device a request came in on says WHOSE it is, and
    /// two devices paired for the same person resolve to the same id — that
    /// is the whole of "two phones are one colleague".
    #[test]
    fn a_clients_person_travels_on_the_caller_and_two_devices_share_one() {
        let mut phone = client_row(1, "phone", "tok-phone", "full");
        let mut laptop = client_row(2, "laptop", "tok-laptop", "full");
        phone.person_id = Some(9);
        laptop.person_id = Some(9);
        let rows = vec![phone, laptop];
        let who = |tok: &str| {
            resolve_token(tok, "master-tok", &[], &rows, OWNER)
                .expect("resolves")
                .person()
        };
        assert_eq!(who("tok-phone"), Some(9));
        assert_eq!(who("tok-laptop"), Some(9));
        // A revoked token never reaches `resolve_token` at all — the caller
        // passes `active_client_tokens` — so nothing resolves for it.
        assert!(resolve_token("tok-gone", "master-tok", &[], &rows, OWNER).is_none());
        // Neither the master nor a per-host token carries a person: the
        // master's is the hub's personal owner, a store read
        // (`mcp::tools::fleet::owner_for`), and a host's token is an agent.
        assert_eq!(
            resolve_token("master-tok", "master-tok", &[], &rows, OWNER)
                .unwrap()
                .person(),
            None
        );
        let hosts = [host_row("mefistos", "tok-mef", "full")];
        assert_eq!(
            resolve_token("tok-mef", "master-tok", &hosts, &rows, OWNER)
                .unwrap()
                .person(),
            None
        );
    }

    /// T2a's boolean, answered where the token is resolved: the master and
    /// the owner's own device are the personal owner; a SECOND person's
    /// device, a person-less device and a per-host token are not. The gate
    /// (`guard::access_allows`) reads only this, so it takes no store lock.
    #[test]
    fn only_the_master_and_the_owners_own_device_are_the_personal_owner() {
        let mut mine = client_row(1, "laptop", "tok-mine", "full");
        mine.person_id = Some(1);
        let mut theirs = client_row(2, "their-phone", "tok-theirs", "full");
        theirs.person_id = Some(2);
        let orphan = client_row(3, "orphan", "tok-orphan", "full");
        let rows = vec![mine, theirs, orphan];
        let hosts = [host_row("mefistos", "tok-mef", "full")];
        let owns = |tok: &str| {
            resolve_token(tok, "master-tok", &hosts, &rows, OWNER)
                .expect("resolves")
                .is_personal_owner
        };
        assert!(owns("master-tok"), "the master is the fleet's owner");
        assert!(owns("tok-mine"), "the owner's own paired device");
        assert!(!owns("tok-theirs"), "a second person's device is not");
        assert!(
            !owns("tok-orphan"),
            "a device bound to nobody is nobody, never everybody"
        );
        assert!(!owns("tok-mef"), "a machine's token is not a person");

        // A hub that cannot say whose it is: nobody is the owner, the master
        // included. Two `None`s must not compare equal — the orphan device
        // is the case that would otherwise walk straight in.
        let nobody = |tok: &str| {
            resolve_token(tok, "master-tok", &hosts, &rows, NO_OWNER)
                .expect("resolves")
                .is_personal_owner
        };
        assert!(!nobody("master-tok"));
        assert!(!nobody("tok-mine"));
        assert!(!nobody("tok-orphan"));
    }

    /// The pane proof rides on the connection (R6-i), so a freshly resolved
    /// caller carries none: `mcp::authorize` stamps it from `X-Fleet-Pane`
    /// after the token has resolved. Nothing in T2 reads it.
    #[test]
    fn a_resolved_caller_carries_no_pane_until_authorize_stamps_one() {
        let rows = vec![client_row(1, "phone", "tok-phone", "full")];
        let hosts = [host_row("mefistos", "tok-mef", "full")];
        for tok in ["master-tok", "tok-phone", "tok-mef"] {
            assert_eq!(
                resolve_token(tok, "master-tok", &hosts, &rows, OWNER)
                    .unwrap()
                    .pane,
                None,
                "{tok}"
            );
        }
    }
}
