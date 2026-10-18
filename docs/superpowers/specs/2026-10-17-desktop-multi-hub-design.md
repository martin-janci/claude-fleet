# Desktop multi-hub — design

One desktop app connected to several `fleet-hub`s at once, with each fleet's
work kept clearly separate. Written without a human in the loop (2026-10-17):
every open question below is decided and recorded rather than asked.

## Why

The owner runs two fleets:

- a **personal hub**, `https://fleet.rlt.sk` — personal, 32bit and SalesTwins
  work;
- a **company hub**, `https://fleet.papayapos.dev` — all PapayaPOS work, owned
  by the PapayaPOS organisation.

The two are federation-linked (messages only, cycle 3,
`2026-09-24-hub-federation-design.md`). The desktop resolves **once, at
startup**, to a window onto **one** hub (`src-tauri/src/backend/`, Settings →
Hub & sync), so the owner cannot see both fleets in one app. They switch by
re-pairing and restarting, or run a phone beside the desktop.

## Goal and non-goals

Goal: one desktop window shows the sessions, hosts and projects of every hub
it is paired with, live, and every action on a row goes to the hub that owns
the row. Which fleet a row belongs to is always visible.

Non-goals:

- **Merging fleets.** Rows are never deduplicated across hubs, ids are never
  translated, and nothing moves between fleets. A session belongs to exactly
  one hub forever (moving a session between fleets is a federation-level
  feature for a later cycle, if ever).
- **Managing a hub's link to another hub.** Federation links stay what they
  are: per-hub, made with `link_peer` from the primary's Federation page.
- **A second brain.** Being a window onto more hubs must never make this
  process reconcile, SSH-manage or otherwise *own* any fleet but the one it
  owned before (its own, when standalone; none, when paired).

## Where we start (survey)

Backend (`src-tauri/src/backend/`, ~24k lines with tests):

- `Backend::resolve` (`mod.rs`) decides Local / Remote / Unavailable once at
  startup from `hub.remote_url`, `hub.client_name`,
  `hub.client_plaintext_token` and one keychain item
  (`token_store.rs`, service `claude-fleet`, account `hub-client-token`).
- `FleetBackend` (`routing.rs`) holds `Option<HubBackend>`; every command is
  `match backend.hub() { Some(h) => <route>, None => <local service> }`.
- `HubBackend` (`remote.rs`) is a self-contained client of one hub: config,
  transport, contract gate (`watching(ConnectionView)`), call breaker.
- `EventBridge` + `HubSse` + `HubResync` (`events.rs`) follow one hub's
  `GET /events`, with backoff, `Last-Event-ID` replay, contract skew check
  per `ready` frame, and a re-list after a gap. It writes into a
  `RemoteEventSink`, which in the app is the very channel local row events go
  down (`app_events.rs`), so the stores "cannot tell the difference".
- `HubConnectionStatus` (`connection.rs`) is both the bridge's reporter and the
  calls' contract gate — **one value per hub**, which matters below.
- `verdicts.rs`: one row per Tauri command (Routed / RoutedUnless / LocalOnly
  / SameInBoth), held to the command bodies by `tests_routing.rs`, generated
  into `src/lib/hub_verdicts.generated.json` and `docs/hub.md`.

Everything per-hub is already an object. What is singular is only *how many
of them the app builds* (one, in `lib.rs` and `bootstrap/tasks.rs`) and the
frontend's assumption that there is one id space.

Frontend (`src/lib`): 326 distinct commands invoked from 87 modules; six
`createRowStore` stores (sessions, hosts, accounts, tasks, access, row_store
itself) plus projects, trackers/work items, inbox, PRs, missions, downloads,
assets; every one keyed by the hub's integer id or host alias. Selection,
drafts, viewed-at, pins, folds, prefs and the terminal map are keyed by
session id too. **Ids collide across hubs** (both hubs have a session 1 and a
host `mac`), so a row from a second hub cannot be merged into any existing
store without re-keying all of it — roughly every module that touches a row.

That sizes the delivery: the backend half is "build N of what we build one
of"; the frontend half is a re-key of the whole app, which cannot land in one
PR without freezing everything else. The phasing below follows from that.

## Decisions

| # | Decision | Why |
|---|----------|-----|
| D1 | **The existing pairing is fleet #1, the "primary", read where it already lives.** Slot id `primary`. No setting is rewritten, no keychain item moved. | Zero user action, zero migration risk, and a downgrade to an older build still finds its hub exactly where it left it. |
| D2 | **Every further hub is an "extra fleet"**: a row in the settings value `hub.fleets` (JSON list of `{id, label, url, client_name, allow_plaintext, color}`), its token in the OS store under account `hub-client-token.<id>`. Never registered in `service::settings::SPECS`, never in a log line; the token never in `state.db`. | Same rules the primary's token already follows (`token_store.rs`), one item per hub so tokens can never be confused. |
| D3 | **The primary stays decided once at launch; extra fleets are hot** — added, removed and retried at runtime, no restart. | The launch-time rule exists because flipping *this process's own* mode mid-run creates a second brain. An extra fleet is never this process's to own, so connecting or dropping one changes no ownership. |
| D4 | **One full client stack per extra fleet**: `HubBackend` + `HubConnectionStatus` + `EventBridge` + `HubResync`, each with its own backoff, `Last-Event-ID`, call breaker and **contract gate**. | "A hub on an older/newer contract must not blind the others" is then structural: a skew verdict lives in that hub's own `HubConnectionStatus` and gates only that hub's calls and events. |
| D5 | **Extra fleets' events travel on one new frontend event, `fleet:event` = `{fleet, name, payload}`.** The primary's channel is unchanged. | No existing store can receive a foreign row, so no id can collide in one. A `TaggedSink` wraps the shared drain thread; event names are still the `&'static str` set from `known_event_name`, so nothing off the wire invents an event. |
| D6 | **Routing by row owner: fleet-scoped commands take an optional `fleet` argument** and pick their backend with `Fleets::select(primary, fleet)`. Absent or `"primary"` → today's `FleetBackend`; an extra fleet's id → that fleet's `FleetBackend` (always a hub, never a local arm); unknown → `E_NOTFOUND`. The command then calls **the same `routed::` function** it always did. | Parity or refusal holds by construction: the argument mapping is the one already reviewed, field by field. A `LocalOnly` command addressed to an extra fleet refuses with that fleet's URL, because its backend is a hub. A secondary `FleetBackend` has no local arm, so nothing can SSH into another fleet's hosts with this machine's keys. |
| D7 | **The verdict table keeps its meaning.** A verdict says what a command does *when addressed to a hub*; an extra fleet is always a hub. New commands that manage the set of fleets (`fleets_list`, `fleet_add`, `fleet_remove`, `fleet_retry_now`) are `SameInBoth`: they are about this process. `FLEET_SCOPED` (in `backend/fleets.rs`) lists the commands that accept `fleet`, and a test holds every listed command's body to `select(` and every other command to not taking `fleet`. | One table stays the truth. The fleet-scoped list grows command by command, each one a reviewed decision, rather than all 326 at once. |
| D8 | **No duplicate hubs.** Adding a URL equal (normalised) to the primary's or another extra fleet's is refused `E_EXISTS` before the code is spent. | Two windows onto one fleet would show every row twice and route half the clicks to a second client token of the same fleet. Phase 2 adds a fleet identity to the `ready` frame to catch the same hub behind two URLs. |
| D9 | **Trust is per hub.** Each extra fleet is its own pairing (`fleet-hub pair --name <this machine>` on *that* hub), its own client row, its own token, its own plaintext opt-in. A device trusted on one hub is not trusted on another; nothing is copied. | A hub's trust is its operator's decision about its own fleet. The untrusted-prompt marker is applied by each hub to its own non-master callers, unchanged. |
| D10 | **Remove forgets, it does not revoke** — same wording and order as Disconnect: stop the bridge, clear the token, then drop the row. | Identical semantics to the primary's Disconnect, which `docs/hub.md` already explains. A row left without a token resolves "no token", never a token without a row. |
| D11 | **No feature flag.** With no extra fleet configured, nothing new runs: no bridge, no keychain read, no new event, and the sidebar shows no fleet switcher. | The feature is inert until someone adds a hub; a flag would be a second switch for the same decision. |
| D12 | **Standalone + extra fleets is allowed.** The local fleet is then the primary ("This machine"). | The owner's laptop can keep its own fleet and watch the company's. D8's URL check and D6's "secondary is always a hub" keep the two apart. |
| D13 | **Fleet colour and label are local**, chosen at add time (label defaults from the host name), stored in `hub.fleets`. | They are this window's way to tell fleets apart; no hub needs to know them. |

## Architecture

### Backend

```
lib.rs setup
  ├─ Backend::resolve (unchanged)          → FleetBackend (primary)      [managed]
  └─ Fleets::start(store, data_dir, sink)  → Fleets                       [managed]
        for each row in hub.fleets:
          OsTokenStore::for_fleet(id) ─ token ─┐
          RemoteConfig ────────────────────────┼─ HubBackend.watching(status) ─ FleetBackend::over(hub)
          HubConnectionStatus::remote(TaggedSink{fleet}) ─┘
          EventBridge(HubSse, TaggedSink, HubResync(hub, TaggedSink)).reporting_to(status)
          (cancelled by its own CancellationToken on remove / quit)
```

`backend/fleets.rs` owns: the `ExtraFleet` record and its (de)serialisation,
slot-id rules (`[a-z0-9-]{1,24}`, not `primary`), `Fleets` (an `RwLock` map
slot → `FleetHandle {record, backend, status, cancel}`), `select`, and the
`FLEET_SCOPED` list. `commands/fleets.rs` holds the four management commands.
`token_store.rs` gains an `account` field so one `OsTokenStore` type serves the
primary (`hub-client-token`, unchanged) and each extra fleet
(`hub-client-token.<id>`): keychain item, Credential Manager target and Linux
file alike.

A fleet whose stored row cannot be used (no token, plaintext without opt-in, a
token store that would not open) is **listed with the reason and not
connected** — the extra-fleet analogue of `Backend::Unavailable`. Unlike the
primary it does not stop anything else.

Shutdown: every extra bridge's token is a child of the app's shutdown token.

### Wire contract per hub

Each extra fleet's `HubConnectionStatus` judges that hub's `ready` frame
against `MIN_HUB_CONTRACT..=MAX_HUB_CONTRACT`. A skewed extra fleet shows
`hub_too_old` / `hub_too_new` on its own badge, its calls answer
`E_HUB_CONTRACT`, and the primary and every other fleet carry on.

### Frontend

- `src/lib/fleets.ts` — the fleet registry store (`fleets`: primary + extras,
  each with label, colour, URL, connection state, reason), the per-fleet row
  stores for extra fleets (`fleetSessions`, `fleetHosts`, `fleetProjects`,
  each `Map<fleet, Map<key, row>>`, key = id or alias **within** that fleet),
  `fleetKey(fleet, id)` for any flat collection, the `fleet:event` listener
  (row events, `hub:connection`, `hub:resynced`), the `fleetFilter` pref
  (`all` or a slot id, kept per viewer in localStorage) and the routed action
  wrappers (`fleetSendPrompt`, `fleetKillSession`, `fleetRenameSession`,
  `fleetPinSession`, `fleetNewSession`), each passing `fleet`.
- Sidebar: `FleetSwitcher.svelte` at the top — *All fleets* plus one chip per
  fleet with its colour and a health dot — shown only when an extra fleet
  exists. Selecting an extra fleet hides the primary's list; selecting the
  primary hides the extra fleets' groups. `FleetSessionGroups.svelte` renders
  each extra fleet's sessions as a group under a fleet badge, with the row's
  status, and per-row actions: send a prompt, rename, pin, close (kill) and
  copy the attach command; a fleet header shows its connection state and a
  *Retry* while it is down.
- Settings → Hub & sync: `FleetsSettings.svelte` lists every fleet (the
  primary read-only, extras removable), and *Add a hub* takes URL, pairing
  code, label, colour and the plaintext opt-in.

### Federation interplay

A federation address is `<fleet-uuid>/session/<host>/<name>`. The desktop does
not know a hub's fleet UUID today: `whoami` reports it only from inside a
session and the `ready` frame does not carry it. Phase 2 adds `fleet_id` to the
`ready` frame (additive, no contract bump needed for old readers) and the
desktop records it per fleet, so an inbox message from
`<company-uuid>/session/mac/dev-x` renders as **company › mac/dev-x** with the
company fleet's badge, and a click opens that session in its own fleet when
that fleet is connected. Messages that crossed a link keep their untrusted
marker; nothing here changes how they are rendered as text.

## Security

- **Per-hub credentials.** One keychain item per hub; a token is only ever
  presented to the base URL it was minted by (the row's URL is written in the
  same step as the token, under the same ordering argument as `hub_pair`:
  forget → write row → store token, so a crash leaves a row with no token,
  which is listed as unusable, never a token beside the wrong URL).
- **No token reuse.** Adding a hub always redeems a fresh pairing code on that
  hub; there is no "copy the primary's token" path.
- **Plaintext opt-in per fleet**, using the same `plaintext_risk` check as the
  primary and the pairing dialog.
- **Redaction.** `RemoteConfig`'s `Debug` already hides the token; reasons go
  through `HubConnectionStatus::scrub` with *that fleet's* token.
- **Untrusted content** is the hub's marking and is unchanged; an extra fleet's
  prompt from this desktop is marked untrusted unless that hub's operator
  trusted this client there.
- **No local arm** for an extra fleet (D6): nothing in the extra-fleet path
  reaches this machine's SSH, store or reconcile.

## Delivery

### Phase 1 — MVP (this PR)

Backend:
- per-fleet token items (`OsTokenStore::for_fleet`);
- `hub.fleets` registry, `Fleets` with hot add / remove / retry and one client
  stack per extra fleet, `fleet:event` tagged events;
- commands `fleets_list`, `fleet_add` (pairs with a code), `fleet_remove`,
  `fleet_retry_now` (all `SameInBoth`);
- `fleet` argument on `list_sessions`, `new_session`, `kill_session`,
  `send_prompt`, `set_session_friendly_name`, `pin_session`, `list_hosts`,
  `list_projects` (`FLEET_SCOPED`);
- tests: registry parsing and slot rules, duplicate-URL refusal, select,
  per-fleet token accounts, tagged sink, and the `FLEET_SCOPED` source test.

Frontend:
- `fleets.ts` (registry, per-fleet row stores, event handling, actions, filter);
- `FleetSwitcher` + extra-fleet session groups in the sidebar, fleet badge and
  colour, per-fleet health;
- `FleetsSettings` in Settings → Hub & sync (add / remove / retry).

Migration: none needed (D1).

**Visible in app:** yes — Settings → Hub & sync → *Fleets*, and the sidebar's
switcher once a second hub is added.

### Phase 2 — a second fleet as a first-class window (landed)

As built: the session view is `FleetSessionView` (conversation, composer,
keys, a terminal under its own pty id, a pane snapshot) rather than
ConversationPanel with a `fleet` prop — that panel's ~80 modules key drafts,
viewed-at, the terminal map, rewind and fork by the primary's ids, which is
Phase 3's re-key. `FLEET_SCOPED` grew by `session_conversation`,
`session_tool_detail`, `capture_session`, `touch_session_viewed` and
`list_pull_requests`, and later by Settings › Environment's `env_vars`,
`env_vars_list`, `env_var_set`, `env_var_unset` plus the two reads that say
what the device may change there, `my_grants` and `setting_proposals`
(a Fleet picker on that page). The New session default is: the fleet last used for
the repository, else the extra fleet whose label or host name is the
repository's owner, else the primary (the org rule, read from what the
window knows; the hubs' org rows are not compared). Repo reads stay on the
primary.

- The full session view (conversation, transcript, terminal attach, repo
  reads) for an extra fleet's session: add `fleet` to those commands
  (`FLEET_SCOPED` grows) and route the selection with its fleet.
- New session: a fleet picker in the new-session dialog, defaulting by rule —
  the project's owner org → the fleet whose org owns it (`PapayaPOS` →
  company), else the fleet whose project list contains the repo, else the
  primary.
- Hosts and Projects pages honour the fleet filter.
- Inbox, PRs, attention and notifications per fleet, with badges.
- `fleet_id` in the `ready` frame; federation addresses rendered as
  `<fleet label> › host/name`; duplicate detection by fleet id.
- Per-fleet error reports (`/report`) and update checks stay on the primary.

### Phase 3 — one id space

- Re-key every store to `(fleet, id)`; the primary becomes an ordinary slot,
  so a person can promote another hub to primary without restarting.
- Every command fleet-aware; the per-command `FLEET_SCOPED` list becomes the
  default and the verdict table gains nothing (a fleet is always a hub).
- Moving work between fleets, if ever, as a federation feature.

## Testing

- Rust, `src-tauri`: `backend::fleets` unit tests (record round-trip, slot
  rules, URL normalisation and duplicate refusal against primary and extras,
  `select` for primary / extra / unknown, unusable rows listed with a reason),
  `token_store` per-account isolation with `InMemoryTokenStore`, the tagged
  sink's envelope, and a source test over `FLEET_SCOPED`.
- `tests_routing`: the four new commands carry `SameInBoth` rows; `SOURCES`
  includes `commands/fleets.rs`.
- Vitest: `fleets.test.ts` — event application per fleet with colliding ids,
  filter, connection-state mapping, and that actions pass `fleet`.

## Risks and open points

- **Two hubs, one host.** Both fleets may manage the same machine (the owner's
  Mac). Each hub reconciles it independently; that is the hubs' business and
  unchanged by this design, but the sidebar will show the Mac under both
  fleets. Correct, since they are different sessions.
- **Attach from an extra fleet** uses this machine's `ssh` with that fleet's
  host alias; it works where aliases agree (the owner's machines) and fails
  with ssh's own message where they do not, exactly as the primary's terminal
  does today. Phase 1 offers *Copy attach command* only.
- **Keychain prompts.** Each extra fleet is one more keychain read at launch;
  they run off the main thread through `BoundedTokenStore`, so a locked
  keychain marks the fleet unusable rather than stalling startup.
