# Organisation administration: one place to run a company

Status: design. Scope was chosen by the owner in conversation on 2026-10-06:
all four parts below, built into **Settings → Organisations**, not as a
separate screen. Phases A and B are built (2026-10-06). C and D are
specified here and each waits for its own plan.

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
