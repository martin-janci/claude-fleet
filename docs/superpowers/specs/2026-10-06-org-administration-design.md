# Organisation administration: one place to run a company

Status: design. Scope was chosen by the owner in conversation on 2026-10-06:
all four parts below, built into **Settings → Organisations**, not as a
separate screen. Phase A is built with this spec. B–D are specified here
and each waits for its own plan.

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

## Phase B — devices and people from the desktop

The hub's CLI-only administration becomes hub tools a desktop routes to.

- **Who:** the master, or the hub's personal owner on a trusted `full`
  device bound to no org — the same person `settings_writer` lets write the
  fleet's settings (declarative pages P6). One gate, `admin_writer`, next to
  it in `mcp/tools/fleet.rs`. A readonly or untrusted owner device reads.
- **Org writes route.** `add_org` … `assign_tracker_org` change from
  `LocalOnly` (`ORGS_ARE_ADMIN`) to routed calls of `work_admin`, whose
  access widens from `Master` to "master or `admin_writer`". The verdict
  rows, `REGEN_HUB_VERDICTS`, and the isolation matrix change with it.
- **Tool `client_admin`** `{ list | pair | revoke | trust | untrust | bind
  | unbind | grant | ungrant | bind_person }`: the `fleet-hub client` and
  `pair` verbs. `pair` answers the single-use code and its QR payload,
  never a token; `revoke` and `untrust` of the caller's own device are
  refused (no lock-out by accident).
- **Tool `person_admin`** `{ list | rename | disable }`.
- **Pages:** the org's `devices` field gets *Pair a device* (mode, trusted)
  and per-chip *Revoke* / *Unbind*; a new resource `client` backs
  **Settings → Devices** (every device, its org, person, mode, trust,
  catalog grants) and `person` backs **Settings → People**. The Catalogs
  page's read-only *Granted to* gets *Grant* / *Ungrant*.

## Phase C — settings, spend and budgets per org

- **Overrides as data.** Migration `org_settings(org_id, key, value,
  set_at, set_by)`. A `SPECS` row opts in with `org_override: true` (part of
  `every_spec_has_consistent_metadata`). `settings::get_for_org(store, key,
  org)` resolves org → fleet → default; every reader of an opted-in key that
  knows the session's or host's org calls it. Writes go through
  `settings::set_by` (audited, `Actor`), as fleet writes do.
- **First keys:** `work.auto_tidy` (the `orgs.auto_tidy` column is migrated
  into the table and kept readable for one release), `work.classify_nudge`,
  `work.session_start_context`, `work.summary_model`,
  `work.tidy_idle_unlinked_days`, `health.context_red_pct`,
  `downloads.keep_secs`. `orgs.jev_allowed` stays a column: it is consent,
  not a setting.
- **Page:** a section *Settings for this org* — one row per opted-in key:
  *Inherit (fleet: X)* or a value, validated by the spec's own kind.
- **Spend:** `usage_daily` gains `org_id` (migration re-keys it by `(day,
  host_alias, org_id, backfill)`, the session's org at booking time). The
  overview shows today / 7 days / 30 days; Usage gets an org filter.
- **Budgets:** `org_settings` keys `budget.daily_usd` and
  `budget.monthly_usd`. Crossing one raises an Attention item
  (`org_budget`) and a `fleet_health.orgs[]` entry. Fleet never stops a
  session over a budget: it warns.

## Phase D — members and roles (multi-user M2)

This is the M2 the multi-user gap analysis defers
(`2026-09-30-multi-user-gap-analysis.md`, roadmap row M2).

- Migration `org_members(org_id, person_id, role, added_at, added_by)`,
  `role` ∈ `admin | member | viewer`.
- **Org admin** (gap analysis §"Org admin"): may do everything in phases
  A–C **for that org only**. They never reach fleet administration
  (hosts' registration, peers, updates, other orgs) and never read members'
  private sessions; there is no break-glass (Q11).
- **Member:** sees the org's work and `visibility = 'org'` sessions;
  `session_share { org }` returns.
- **Viewer:** reads the org's work view and overview.
- A device's org is its person's membership; `client_tokens.org_id`
  becomes derived and stops being written by `assign_client`.
- The page gains a **Members** section (person, role; add / change /
  remove), and the overview shows who is in the company.

D needs the owner's answers on the gap analysis's open M2 questions (who
administers a host; the departed member's grants; the unclaimed count on a
multi-person hub) before its plan is written.

## Not in scope

- A separate "admin console" window (the owner chose Settings).
- Billing, invoices, seats.
- Anything that lets an org admin read a member's private session.
