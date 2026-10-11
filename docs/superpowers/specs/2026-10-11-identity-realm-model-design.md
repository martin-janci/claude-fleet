# Identity: the realm model and user-management hardening

Status: design, approved in brainstorming 2026-10-11. Sub-project 1 of the
identity roadmap below.

## Why

The hub is to become an organisation's identity provider: its own login
(password + TOTP, passkeys, magic link, upstream IdPs), OIDC tokens a
Keycloak-configured application accepts unchanged, roles and groups in the
Keycloak model, and hub deployments per environment (dev / staging / prod)
that exchange their realm by export / import.

Today (origin/main 2a6825c49):

- SSO (`crates/fleet-core/src/mcp/oidc.rs`) is landed, off by default. It
  reads Keycloak's `realm_access` / `resource_access` / `groups` claims, but
  only as an admission allow-list, and does **not** verify the ID token's
  signature.
- `people` (098) and `person_identities` (167) know who a human is;
  `org_members` (107) gives a fixed `admin | member | viewer` role per org.
  There are no groups, no custom or composite roles, and fleet-wide
  administration is exactly one person (`is_personal_owner`).
- Client tokens never expire; the master and per-host tokens are stored in
  plaintext; trust and assets-admin are per device, not per person (gap
  analysis §2.7); person administration has no audit trail and no re-enable.

## Roadmap (one spec → plan → PR stack each)

| # | Sub-project | Content |
|---|---|---|
| 1 | **Realm model + hardening** (this spec) | users, hierarchical groups, realm/client/composite roles, role mappings; orgs onto groups; audit; token expiry; JWKS verification |
| 2 | IdP core | signing keys + rotation, JWKS, discovery at `/realms/<realm>/…`, login pages (password + TOTP, lockout), token endpoint (code + PKCE, rotating refresh, client credentials, device grant), userinfo, introspection, RP-initiated + back-channel logout, client registry, Keycloak-shaped claims |
| 3 | Fleet clients on OIDC | desktop / mobile via system browser + PKCE, CLI via device grant, MCP agents as service accounts; pairing stays as fallback |
| 4 | Passkeys + e-mail | WebAuthn, SMTP, magic link, invitations, password reset |
| 5 | Broker | today's `oidc.rs` becomes an upstream IdP (several at once) with group / role mappers from Keycloak |
| 6 | Administration + environments | desktop admin pages (edit), realm export / import between hub deployments |

Decisions taken for the whole roadmap: the IdP is our own crate inside the
hub (no sidecar, no bundled Keycloak); it is a drop-in Keycloak replacement
and can broker to an upstream Keycloak; one realm per hub, environments are
separate hub deployments; administration is desktop + MCP + CLI (the hub
serves only login / account pages; no Keycloak Admin REST API).

## 1. Data model

Shaped so that one row maps to one entry of a Keycloak `realm.json`.

**Users are `people`.** New columns: `uuid` (Keycloak's `id`; the integer
`id` stays internal), `email`, `email_verified`, `first_name`, `last_name`,
`attributes` (JSON object of string arrays, as Keycloak's). `enabled` is
derived from `disabled_at`. `person_identities` gains a foreign key to
`people` and an `auth_epoch` trigger; it is Keycloak's *federated identity*
once sub-project 5 lands.

**Roles** — `roles(id, uuid, name, description, client_id NULL, composite)`:
`client_id IS NULL` is a realm role, otherwise a client role; unique on
`(client_id, name)`. `role_composites(parent_id, child_id)`; a write that
would create a cycle is refused (`E_INVALID`).

**Groups** — `groups(id, uuid, name, parent_id NULL, attributes, managed)`,
unique on `(parent_id, name)`; the path is `/a/b`. `group_members(group_id,
person_id, added_at, added_by)`.

**Role mappings** — `user_role_mappings(person_id, role_id)`,
`group_role_mappings(group_id, role_id)`.

**Clients** — `clients(id, uuid, client_id UNIQUE, name)`, a register only;
sub-project 2 adds the OIDC columns. One seeded row: `fleet`.

**Effective roles** of a person = direct mappings ∪ mappings of every group
they are in and every ancestor of those groups, closed transitively over
composites. Resolved when a `Caller` is resolved, cached with it, invalidated
by `auth_epoch` (every new table has the insert / update / delete triggers).

**Seeded roles:**

| Role | Kind | Meaning |
|---|---|---|
| `fleet-admin` | realm | administers the whole fleet — today's hub owner (`Authority::Fleet`, `Access::Person` / `PersonDevice`) |
| `fleet-trusted` | realm | today's per-device `trusted_at`, now per person |
| `assets-admin` | realm | today's per-device `assets_admin_at`, now per person |
| `org-admin` ⊃ `org-member` ⊃ `org-viewer` | client `fleet`, composite | today's `org_members.role` |

**Orgs onto groups.** An org (`orgs`, unchanged: rules, hosts, isolation)
owns a managed group `/orgs/<org>` with subgroups `admins`, `members`,
`viewers`, mapped to `fleet:org-admin` / `org-member` / `org-viewer`. A
managed group's structure follows its org (rename, remove); only its
membership is edited. (Keycloak 26 calls this an Organization; the export in
sub-project 6 writes both shapes.)

Session grants (`watch | drive`) stay what they are: grants on a resource,
not roles. A device's `readonly` mode stays a property of the device.

## 2. How authorization reads roles

**`org_members` becomes a materialized projection.** Groups and role
mappings are the source of truth. Every write to `groups`, `group_members`
or a role-mapping table calls `reproject_org_memberships(person)` in the same
transaction, which computes the person's effective `fleet:org-*` roles per
org and writes the difference to `org_members`:

- several roles in one org: the highest wins (`admin > member > viewer`);
- `shares_since` is set only on a transition from viewer or no role to
  member or admin — a membership change still never widens an existing org
  grant;
- losing every role sets `removed_at`; the row stays (fail closed).

So `ViewScope`, `service::org_admin::Authority` and the org-grant checks are
unchanged. Nothing else writes `org_members` (a test pins it): `org_admin`'s
`set_member` / `remove_member` and `fleet-hub org member …` keep their
signatures but write membership of the org's managed subgroup instead.

**Fleet-wide administration** — `Authority::Fleet` and `Access::Person` /
`PersonDevice` change from "person is `personal_owner_id()`" to "person has
the effective realm role `fleet-admin`", resolved into `Caller` as
`is_fleet_admin`. `guard::access_allows` keeps taking no store. The personal
owner keeps `fleet-admin` irrevocably (the store refuses to remove it, as it
refuses to disable the owner).

**Device privileges per person** — a device is trusted / assets-admin when
its person has `fleet-trusted` / `assets-admin` **and** the device is `full`.
The device columns are kept for one release, read nowhere, then dropped.

**Migration** (one SQL migration plus a Rust backfill in the same
transaction):

1. create the tables; seed `fleet`, the realm roles, the `fleet:org-*` roles
   and their composites;
2. per org, create `/orgs/<org>` with `admins` / `members` / `viewers` and
   their mappings;
3. every live `org_members` row becomes membership of the matching subgroup,
   keeping `added_at` / `added_by`;
4. the personal owner gets `fleet-admin`; a person with a live trusted
   (assets-admin) device gets `fleet-trusted` (`assets-admin`);
5. run the projection and require an empty diff against `org_members` —
   otherwise the migration fails and nothing changes.

The hub wire contract does not change (`CONTRACT_REVISION` stays); the new
tables reach the desktop only through the `realm_admin` tool (§4).

## 3. Hardening

**Person lifecycle.** New `enable_person`: the person may sign in and pair
again; devices and grants revoked by the disable are **not** restored.
`disable_person` also ends every group membership (through the projection),
recorded in the audit log. There is still no person delete.

**Audit log** — `admin_events(id, ts, actor_person_id, actor_device_id,
action, resource_type, resource_id, representation)`, Keycloak's admin
events. Every write to people, groups, roles, mappings, SSO links, devices
and trust records one row; `representation` is a JSON diff passed through
`guard::redact_args` (no secrets). Triggers refuse `UPDATE` and `DELETE`
except the GC's retention delete (`auth.audit_retention_days`, default 365).

**Token expiry.** `client_tokens` gains `expires_at` and `last_used_at`
(written at most once a minute). Settings `auth.device_token_ttl_days`
(sliding idle limit, default 90) and `auth.device_token_max_age_days`
(absolute, default off). Existing tokens start their TTL at the migration, so
nobody is signed out by it. An expired token is refused as `E_UNAUTHORIZED`
with a reason the clients show ("pair again").

**Master token hashed.** Stored as SHA-256 like client tokens.
`fleet-hub token show` is replaced by `fleet-hub token rotate`, which prints
the new token once. Breaking for the operator: CHANGELOG and `docs/hub.md`.
Per-host tokens are hashed too **if** nothing re-reads them in plaintext
after provisioning; the plan's first task checks that, and if something
does, host-token hashing moves to sub-project 3.

**SSO.** `FLEET_HUB_OIDC_AUTO_PROVISION` defaults to `false`: an unknown SSO
account signs in only if linked (`link-sso`) or invited. The ID token's
signature is verified against the issuer's JWKS (`jwks_uri` from discovery,
cached, one refetch on an unknown `kid`, RS256 / ES256 / PS256 only, `none`
and HMAC refused). The rest of `oidc.rs`'s future (group mappers, several
issuers) is sub-project 5.

## 4. Administration surface

**`service/realm_admin.rs`** — users (list, get, update profile, enable,
disable), groups (create, rename, move, delete unmanaged, members), roles
(create, update, delete unseeded, composites), role mappings, audit query.

Authority:

- `fleet-admin`: everything;
- `org-admin` of an org: membership of that org's managed subgroups only
  (today's `Authority::Org` reach);
- anybody else: refused.

No lock-out: nobody removes their own `fleet-admin`; the last `fleet-admin`
and the personal owner's cannot be removed; seeded roles and managed groups
cannot be deleted.

**Reach:**

- MCP tool `realm_admin` (`Access::Device`, one `action` field like
  `org_admin`), a slim description within the tool-surface budget; prose in
  the control skill; `REGEN_DOCS=1 cargo fleet-test -- reference_is_current`;
- `fleet-hub realm user|group|role|audit …` on the hub machine;
- desktop: a read-only *People & groups* page (declarative pages: a person's
  effective roles and where each comes from, the group tree, the audit
  log). Editing is sub-project 6.

## 5. Testing

- Effective roles: inheritance from groups and their ancestors, composites,
  cycle refusal, invalidation through `auth_epoch`.
- Projection: a property test of random writes asserting projection ==
  direct computation, and the `shares_since` invariant.
- Migration: `store::testgen` fixtures with orgs, members, trusted and
  assets-admin devices before the migration; empty diff, owner is
  `fleet-admin`, nobody loses trust.
- Authorization table: every `realm_admin` action × (fleet-admin, org-admin
  of the org, org-admin of another org, member, host token, master) →
  allowed or the exact `E_*`.
- Audit: append-only triggers, redaction.
- Tokens: sliding and absolute expiry, the master hash, `token rotate`.
- JWKS: a signed token, a bad signature, an unknown `kid` (one refetch),
  `alg: none` and HS256 refused.

## 6. PR stack

1. JWKS signature verification (independent, first).
2. Tables, migration, effective roles, projection.
3. `fleet-admin`, `fleet-trusted`, `assets-admin` wired into authorization.
4. Audit log, `enable_person`, disable ends memberships.
5. Token expiry, master-token hash, `token rotate`, auto-provision default.
6. `realm_admin` (MCP + CLI) and the read-only desktop page.

## Out of scope here

OIDC issuance, login pages, credentials, e-mail, passkeys, brokering several
IdPs, realm export / import, admin editing UI — sub-projects 2–6.
