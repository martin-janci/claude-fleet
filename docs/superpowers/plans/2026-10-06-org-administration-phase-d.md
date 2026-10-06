# Org administration, phase D — members and roles

Spec: `docs/superpowers/specs/2026-10-06-org-administration-design.md`
(*Phase D*), which is the multi-user M2 of
`docs/superpowers/specs/2026-09-30-multi-user-gap-analysis.md`.

## The owner's answers (2026-10-06)

| Question (gap analysis, M2) | Answer |
|---|---|
| Who administers a host? | **The hub's owner.** When the hub belongs to a company, that company does: the admins of the org that owns the hub. |
| Does an org admin get authority over the grants a departing member was given? | **Yes**: revoke or narrow, downward only (Q9). |
| On a hub with several people, does an org admin see the count of unclaimed sessions on the org's hosts? | **As an option**, off by default, switched per org by the hub's owner. |

## Decisions this plan takes

1. **Memberships are rows**, `org_members(org_id, person_id, role,
   added_at, added_by, shares_since, removed_at)`, `role` ∈ `admin |
   member | viewer`. A removed member keeps the row with `removed_at`, so
   "former member" is a fact the scope can read. No foreign keys (the 066
   rationale). Every write bumps `auth_epoch`, because a membership decides
   what a device resolves to.
2. **A device's org follows its person's memberships** (one function,
   `store::org_members::effective_device_org`, applied where the auth layer
   reads token rows and where `/events` re-checks a client):
   * the hub's owner, or a person with no membership row at all: the
     device's own binding, as before (an upgraded hub changes nothing);
   * a person with live memberships: the device's binding when it is one of
     them, else their first membership (by `added_at`). Never an org they are
     not in;
   * a person whose every membership was removed: `orgs::NO_ORG`, which
     reads nothing of any org — a departed member does not fall back to the
     whole fleet's work.
   A **viewer**'s device is `readonly` whatever its token mode; a former
   member's too.
   `client_tokens.org_id` stays as the device's chosen org among the
   person's memberships (the M3 switcher's seed); `org_admin
   { bind_device }` refuses an org the person is not in.
3. **Team sharing is a grant to an org** (`session_grants.org_id`, reserved
   since migration 100), not a third `visibility`: the gap analysis calls
   them the same capability, and the grant carries the rule the owner set —
   *changing a membership never widens an existing grant*. An org grant
   reaches a member or admin whose `shares_since` (when they last became
   able to receive shares) is not after the grant's `granted_at`. Only the
   session's owner shares, and only with an org they are a member (not a
   viewer) of, or as the hub's owner. A viewer never receives one.
4. **Authority** (`service::org_admin::Authority`):
   * `Fleet`: the desktop's own store, or the hub owner's unbound device —
     everything, as in phases A–C;
   * `Org { org, hub }`: a trusted person's device whose (effective) org is
     `org` and whose person is its admin (the hub owner counts as admin of
     every org). `hub` is set when `org` owns the hub (`orgs.owns_hub`).
   An org admin acts on its own org only: its settings, colour, isolation,
   auto-tidy and Jev consent, its own settings, its members, its members'
   devices (pair, revoke, trust), its catalogs' grants, its spend. Rules,
   tracker routing, `bound_sees_unassigned`, `owns_hub`,
   `admins_see_unclaimed`, other orgs, people's names and disabling stay
   the hub owner's: each one decides which company something belongs to, or
   reaches beyond one. Host routing (`assign_host` / `unassign_host`) is a
   host administrator's: the hub owner, or (`hub`) an admin of the org that
   owns the hub. Registering, removing and provisioning hosts stay the
   operator's (`Access::Master`), unchanged.
5. **Departure**: `remove_member` revokes every live grant TO that person on
   the org's sessions (unless `keep_grants`); `member_grants` counts them and
   `revoke_member_grants` / `narrow_member_grants` act on a current member.
   Counts only — a grant list would name sessions, and a session's metadata
   is content.
6. **Unclaimed counts**: `HostRow.unclaimed_sessions` is served to the one
   person of a one-person hub (M1), to the hub's owner, to the admins of the
   org that owns the hub, and — for its own hosts — to an org's admins when
   the hub owner turned `orgs.admins_see_unclaimed` on.
7. **No admin reads a member's private session.** Nothing here touches
   `ViewScope::sees_session_row`'s person half; the org grant is a grant.

## Tasks

- **D1 — store.** Migration 105 (`org_members`, `orgs.owns_hub` with its
  unique index, `orgs.admins_see_unclaimed`, auth-epoch triggers);
  `store/org_members.rs` (add/upsert, set role, remove, list, memberships of
  a person, effective device org); the derivation applied in
  `ReadPool::auth_snapshot`, the writer fallback in `mcp/mod.rs` and
  `client_token_binding`; `remove_org` tombstones its memberships.
- **D2 — scope.** Org grants in `grant_session` / `grants_for_person` (with
  `shares_since`), revoke and narrow for an org recipient, `session_access`
  names orgs; `session_share|unshare|narrow { org }`; membership writes bump
  the grant generation; `ViewScope`'s host-admin fact and `list_hosts`.
- **D3 — authority.** `Authority`, `Access::Device` for `org_admin`, the
  per-action checks, the member actions, `set_hub_org`,
  `AdminView::{Fleet, Org, Other}` with members on the org detail.
- **D4 — operator.** `fleet-hub org member list|add|remove`, `fleet-hub org
  own-hub`, `--admins-see-unclaimed`.
- **D5 — desktop.** Commands and verdict rows, the org page's Members
  section, sharing with an org in the Share sheet, tests.
- **D6 — docs and validation.** Spec, user guide, `docs/hub.md`, control
  API, regenerated references, the full suite.
