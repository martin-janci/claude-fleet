# Organisation administration: one place to run a company

Status: design. Scope was chosen by the owner in conversation on 2026-10-06:
all four parts below, built into **Settings → Organisations**, not as a
separate screen. Phases A–D are built (2026-10-06).

## Problem

An organisation (work graph M5, `orgs`) is the unit a person runs a company
by. Its administration is scattered today:

| What | Where it is set today |
|---|---|
| name, colour, rules, hosts, trackers, isolate, auto-tidy, Jev, bound-sees-unassigned | Settings → Organisations (standalone desktop only); `fleet-hub org …` |
| the org's asset catalogs | Settings → Catalogs (the org is a text field on the catalog) |
| catalog admissions for org-less hosts | Settings → Catalogs |
| devices bound to the org (`client_tokens.org_id`) | `fleet-hub pair --org`, `fleet-hub client bind|unbind` only |
| which device may change which catalog | `fleet-hub client grant|ungrant … --catalog` only |
| people (`people`, `client_tokens.person_id`) | `fleet-hub person …`, `pair --person` only |
| a task's org | Work view → *Assign org…* |
| live sessions / sessions that need a person, per org | the scope selector only |
| per-org overrides of fleet settings | only `orgs.auto_tidy`; nothing general |
| spend per org, budgets | none |
| members and roles | none (multi-user M2) |

On a desktop paired to a hub, every org write is `LocalOnly`
(`ORGS_ARE_ADMIN`), so the company's owner has to use the hub's CLI.

## Decisions

- **One home: Settings → Organisations.** The org page grows sections; it
  stays a `master_detail` page on the `org` resource (declarative pages P4),
  so every write is still an existing command with its own hub verdict.
- **Phased:** A overview (read) → B devices and people → C per-org settings,
  spend and budgets → D members and roles. Each phase is usable alone.
- **Nothing in A widens a caller's reach.** The new read data goes through
  the same `work { action: orgs }` read, under the caller's whole
  `ViewScope`; device names reach only the fleet's administrator.

## Phase A — the org overview (built)

`OrgDetail` (`service/orgs.rs`), what `work { action: orgs }` and
`list_orgs` answer, gains:

- `session_count`, `needs_you` — the org's live sessions and the ones
  waiting on a person, counted exactly as `work { action: scopes }` counts
  them (one helper, `org_session_counts`, under the caller's `ViewScope`:
  org half and person half).
- `catalogs` — the names of the asset catalogs the org owns
  (`catalogs.org_id`). A host of the org already receives them.
- `devices` — the live paired clients bound to the org: name, mode,
  trusted, last seen. **Only for the fleet's administrator**: the
  desktop's own store, the master token, or the hub's personal owner on an
  unbound device (`Caller::is_person_device` and `is_personal_owner`).
  Everyone else gets no `devices` key at all (not an empty list), so a host
  token or an org-bound device never learns another device's name. The
  isolation matrix row for `work { orgs }` holds this.

The resource gains a read-only field kind, `count`, for the two numbers.
The page (`settings.orgs.json`) gains an **Overview** section (sessions,
need you), `catalogs` under *What belongs to it*, and a **Devices** section
with how to pair one (`fleet-hub pair --org <name>`) until phase B.

Old hubs: every new field is `#[serde(default)]`, so a paired desktop on an
older hub shows zeros and empty lists — no contract bump.

## Phase B — devices and people from the desktop (built)

The hub's CLI-only administration became one hub tool a desktop routes to.

- **One tool, `org_admin`** (`service/org_admin.rs`, the tool in
  `mcp/tools/fleet.rs`), rather than the `client_admin` / `person_admin`
  pair first sketched: one gate, one routing family, one place for the
  lock-out rule. Actions: the org actions of `work_admin` under the same
  names (an org named by `org_id` or `org`), `list_devices`,
  `pair_device`, `revoke_device`, `set_device_trust`, `bind_device`,
  `set_device_person`, `grant_catalog`, `list_people`, `rename_person`,
  `disable_person`.
- **Who:** `Access::PersonDevice` — the hub owner's own device bound to no
  org. Not the master (it has `fleet-hub org|client|person` and
  `work_admin`), not a host, an org-bound device or a colleague's. The tool
  is not readonly, so a readonly device is refused it whole. Lists for any
  such full device; a change needs it **trusted** (`org_admin_writer`, the
  same person `settings_writer` lets change the fleet's settings).
- **No lock-out:** the device a call comes through cannot revoke, untrust,
  bind, hand over or lose a catalog grant through it; the owner cannot be
  disabled (`Store::disable_person`). A peer link or updater token is not
  a device: never listed, refused by every device action, and never paired
  here (`pair_device` takes `full` / `readonly` only).
- **Pairing:** `pair_device` shares `pair_client`'s mint (`mint_pairing`)
  and answers the URL's QR as rows of `1` / `0` (the `qrcode` crate
  fleet-hub already used), drawn by the desktop as an SVG
  (`PairingResult.svelte`, result view `pairing`).
- **Org writes route.** The seven org commands changed from `LocalOnly`
  (`ORGS_ARE_ADMIN`, removed) to `Routed { tool: "org_admin" }`; standalone
  they run `service::org_admin` on the desktop's store. Ten new desktop
  commands (`src-tauri/src/commands/org_devices.rs`) route the device and
  people actions; standalone, `pair_device` refuses (a code is a hub's).
- **Pages:** Settings gains a **Company** section — Organisations, Devices
  (resource `device`: pair, trust, bind / unbind, hand to a person, grant /
  take back a catalog, revoke) and People (resource `person`: rename,
  display name, disable). The org page's *Devices* list binds and unbinds a
  device. Resources gained option sources `devices` / `catalogs`, a
  `choice` param, constant `true` / `false` arguments and record-action
  option selects.
- **Older hub:** a hub without `org_admin` answers an unknown tool, which
  the desktop shows as the hub's error; nothing else changes on it.

## Phase C — settings, spend and budgets per org (built)

- **Overrides as data.** Migration 104 adds `org_settings(org_id, key,
  value, set_at)`. A `SPECS` row opts in with `.per_org()` (shown as
  `per_org` in `describe`). `settings::get_string_for` / `get_bool_for`
  read the org's own valid value, else the fleet's; `org_values` gives a
  pass every org's value of one key. `settings::set_for_org` validates and
  normalises like a fleet write and audits under `<key>@org:<id>`, with the
  device that made it.
- **The keys, and who reads them per org:** `work.classify_nudge` (the
  nudge, by the session's org), `work.summary_model` (Summarise, by the past
  link's `snap_org_id`), `work.tidy_idle_unlinked_days` (Tidy's planner,
  `TidyConfig::unlinked_idle_for`), and the new budgets. Not migrated:
  `orgs.auto_tidy` stays its own column — it already was a per-org override
  with an Inherit control — and `orgs.jev_allowed` stays consent. Keys whose
  readers do not know the org (`work.session_start_context`, which also
  decides whether the hook is installed; `health.context_red_pct`, read once
  per pass) stay fleet-wide.
- **Spend:** `usage_daily_org(day, org_id, backfill, …)`, booked beside
  `usage_daily` in the same `apply_usage` pass, by the session's org at
  booking time (`session_org_sql!`). Additive rather than re-keying
  `usage_daily`, so no existing roll-up moves. It starts empty: spend before
  the upgrade has no org. Live rows only count (`org_live_cost_since`).
- **Budgets:** `budget.org_daily_usd` and `budget.org_monthly_usd` (whole
  USD, `0` none; Settings → Limits → *Company budgets*, and per org on its
  page). `service::org_spend` reads spend (today, 7 days, calendar month,
  UTC) and which budgets are reached. Fleet warns; it never stops a session.
- **Who sees spend:** an org's spend sums other people's private sessions,
  so it follows `usage_by_day`'s rule for a person's device — all or
  nothing, only for a caller that sees every session row
  (`org_spend::sees_all_spend`). `OrgDetail` carries the org's own settings
  for the administrator (`AdminView::Admin`) and its spend and budgets only
  when that administrator also sees every session.
  `fleet_health.org_budgets` lists the orgs at or over a budget for
  `HealthView::Fleet` and a person's device that sees every session; it is
  in the hub contract (additive), and the desktop raises one Attention item
  per org and period, "Acme over its daily budget", opening Settings →
  Organisations.
- **Page:** the org page's *Overview* shows spent today / last 7 days / this
  month and the budgets reached; *Its own settings* lists each per-org
  setting inheriting the fleet's value (with *Set for this org*) or with the
  org's own (the ordinary settings row, with *Inherit*). Resource field
  kinds `money` and `settings`; `org_admin { set_org_setting }` and the
  desktop command `set_org_setting` (routed).

## Phase D — members and roles (multi-user M2, built)

Plan: `docs/superpowers/plans/2026-10-06-org-administration-phase-d.md`.

**The owner's answers (2026-10-06)** to the gap analysis's open M2
questions: a host is administered by **the hub's owner, or — when a company
owns the hub — that company** (its admins); an org admin **may** revoke or
narrow the grants a departing member was given (downward only); and whether
an org admin sees the unclaimed count on the org's hosts is **an option**,
the hub owner's, off by default.

- Migration 105: `org_members(org_id, person_id, role, added_at, added_by,
  shares_since, removed_at)`, `role` ∈ `admin | member | viewer`; a removed
  member keeps the row (`removed_at`). `orgs.owns_hub` (at most one) and
  `orgs.admins_see_unclaimed`. Every membership write moves the auth epoch.
- **A device's org follows its person** (`store::effective_device`, applied
  to the auth rows and the `/events` re-check): the hub's owner and a person
  in no org keep their binding; a member's device is fenced to one of their
  orgs (`client_tokens.org_id` picks which; `bind_device` refuses another);
  a viewer's device is read-only; a former member's reads nothing of any org
  (`NO_ORG`). So `client_tokens.org_id` is no longer the authority for a
  member — their membership is — and `assign_client` remains only the hub
  owner's way to bind their own or a non-member's device.
- **Team sharing is a grant to an org**, not `visibility = 'org'`: the gap
  analysis calls the two the same capability, and the grant carries the rule
  the owner set — a membership change never widens an existing grant. It
  reaches members and admins whose `shares_since` is not after `granted_at`;
  a viewer never. Only the session's owner shares, with an org they are a
  member of (or as the hub's owner). `sessions.visibility` keeps its two
  values.
- **Org admin** (`org_admin::Authority::Org`): everything in phases A–C for
  that org only — its settings, colour, isolation, auto-tidy, Jev consent,
  own settings, spend, members, its members' devices and its catalogs'
  grants. Rules, tracker routing, `bound_sees_unassigned`, other orgs,
  people's names, disabling and the two switches stay the hub owner's (each
  decides which company something belongs to). Host routing is a host
  administrator's. No admin reads a member's private session; there is no
  break-glass (Q11).
- **Member** sees the org's work and what is shared with the org; **viewer**
  reads the org's work view and overview.
- Departure: `remove_member` revokes the grants TO that person on the org's
  sessions unless `keep_grants`; `member_grants` / `revoke_member_grants` /
  `narrow_member_grants` act on a member's grants there (counts only — a
  list would name sessions).
- The unclaimed count (`HostRow.unclaimed_sessions`): the one person of a
  one-person hub, the hub's owner and the owning company's admins on every
  host, an org's admins on its hosts when `admins_see_unclaimed` is on.
- The page's **Members** section (person, role; add / change / remove), and
  the overview's `members` and `my_role`; the Share sheet's *an org*;
  `fleet-hub org member|own-hub|unclaimed-count`.

## Not in scope

- A separate "admin console" window (the owner chose Settings).
- Billing, invoices, seats.
- Anything that lets an org admin read a member's private session.
