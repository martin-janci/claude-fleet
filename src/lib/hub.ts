// Which fleet this window is onto, and what that costs.
//
// Standalone the desktop *is* the fleet: its own database, its own SSH
// connections, its own reconcile tick, its own control API. Paired with a
// `fleet-hub` it is a client of someone else's fleet, and three families of
// thing stop working from here:
//
//   1. **Fleet administration.** The desktop pairs as an ordinary client, and
//      a client is never the fleet's administrator: adding, removing and
//      hiding hosts, provisioning, asset sync and secrets are the hub's.
//   2. **Things about THIS machine.** The terminal's PTY, the local
//      prerequisites check, SSH tunnels, the asset-catalog git checkout.
//   3. **PARITY OR REFUSAL.** A mutation routes to the hub only where the
//      desktop's arguments map one-to-one onto the tool's parameters. Where
//      they do not — `repair_session` with `explicit: false`, the automatic
//      pre-attach check — the command refuses rather than routing, because
//      routing would have quietly turned it into the tool's own (always
//      explicit) repair. A refusal is visible; a dropped field is not.
//      Nobody should later "fix" a refusal like this by wiring a lossy
//      mapping.
//
// The backend returns `E_LOCAL_ONLY` for these, with a message naming where
// the operation does work. This module is the *other* half: the reason in
// front of the control, before the click, so that a paired desktop looks like
// a paired desktop rather than like a broken one.
import { get, writable } from 'svelte/store';
import { invokeCmd, type IpcError, type Result } from './result';
import { hubConnection, type HubConnection } from './hub_connection';
import { hubUnavailableWords } from './error_copy';

/** Mirrors the backend `HubStatus` (`src-tauri/src/commands/hub.rs`). */
export interface HubStatus {
  /** What this process is doing right now. */
  remote: boolean;
  /** The hub it is a window onto; null when standalone. */
  url: string | null;
  client_name: string | null;
  /** `full` | `readonly`, known only in the session that paired. */
  client_mode: string | null;
  /** `hub.remote_url` as stored — differs from `url` until a restart. */
  configured_url: string | null;
  configured_client_name: string | null;
  allow_plaintext: boolean;
  /** Why a configured hub is not in use, or why the one in use is risky. */
  warning: string | null;
  /** The stored configuration no longer matches the running mode. */
  restart_required: boolean;
  /**
   * Set when a hub is configured but THIS launch could not use it (no stored
   * token, a keychain that would not open, plain http without the opt-in, a
   * URL that does not parse), with the reason. The backend then owns nothing
   * — no reconcile tick, no usage poll, no control API — and refuses every
   * fleet command, rather than quietly becoming a second brain for the hub's
   * fleet. `remote` is false in this state: nothing is talking to a hub.
   */
  unavailable: string | null;
}

export const STANDALONE: HubStatus = {
  remote: false,
  url: null,
  client_name: null,
  client_mode: null,
  configured_url: null,
  configured_client_name: null,
  allow_plaintext: false,
  warning: null,
  restart_required: false,
  unavailable: null,
};

/**
 * The last status the backend gave us. Standalone until told otherwise, which
 * is the safe default in one direction only — it enables things — so
 * `loadHubStatus` runs before the first render that can act on it, and a
 * FAILED reload deliberately keeps the previous value rather than falling
 * back to standalone.
 */
export const hubStatus = writable<HubStatus>({ ...STANDALONE });

export async function loadHubStatus(): Promise<Result<HubStatus>> {
  const r = await invokeCmd<HubStatus>('hub_status');
  if (r.ok && r.value) hubStatus.set(r.value);
  return r;
}

/** Redeem a pairing code. Takes effect at the next launch — the backend mode
 *  is resolved once, at startup, on purpose. */
export async function hubPair(
  url: string,
  code: string,
  allowPlaintext: boolean,
): Promise<Result<HubStatus>> {
  const r = await invokeCmd<HubStatus>('hub_pair', {
    args: { url: url.trim(), code: code.trim(), allow_plaintext: allowPlaintext },
  });
  if (r.ok && r.value) hubStatus.set(r.value);
  return r;
}

/**
 * Forget the pairing on this machine.
 *
 * This **revokes nothing**. The client row stays on the hub and the token
 * stays valid there until an operator revokes it (`fleet-hub client revoke`),
 * and a paired client is refused `revoke_client` by design — so the app could
 * not revoke even if it wanted to. Every string this module and the Settings
 * dialog show says so; anything softer would leave someone believing a stolen
 * laptop had been locked out when it had not.
 */
export async function hubDisconnect(): Promise<Result<HubStatus>> {
  const r = await invokeCmd<HubStatus>('hub_disconnect');
  if (r.ok && r.value) hubStatus.set(r.value);
  return r;
}

/**
 * Whether a client token is sitting on this machine with **no hub
 * configured** — a fleet-wide credential no launch reads.
 *
 * Pairing used to store the token before the URL, so a crash between the two
 * writes stranded one. That order is fixed (the token goes last now), but an
 * install that took the crash carries the token across the upgrade, and
 * `hub_status` never finds it: a blank URL is standalone, and the backend
 * deliberately does not query the OS keychain on every launch just in case.
 *
 * So Settings asks once, on open — the one moment a keychain prompt is
 * explicable — and offers {@link hubDisconnect}, which clears it. Deliberately
 * **not** a field of {@link HubStatus}: it is a one-off question about a legacy
 * state, not part of which fleet this window is onto, and nothing branches on
 * it but the Hub section.
 */
export async function hubStrandedToken(): Promise<Result<boolean>> {
  return invokeCmd<boolean>('hub_stranded_token');
}

// ---- what a hub client cannot do from here ---------------------------------

/**
 * Why each action is unavailable on a paired desktop, as a whole sentence
 * (review w16-6). `operator`, when set, is the one thing to tell a person who
 * cannot do it at all: an administration step the hub's operator runs, in
 * the words of its CLI. A reason without one says itself where the action
 * does work, or what to use here instead — `hubBlock` adds no "do it on the
 * hub", which was false for half of them ("edit the asset here instead",
 * "use Kill instead").
 *
 * The keys are the command names wherever there is one, so a reader can find
 * the matching `backend.local_only(...)` in `src-tauri/src/commands/`.
 */
interface Reason {
  text: string;
  /** Administration a client never does: `true` sends the person to the
   *  hub's operator, a string also names the operator's command. */
  operator?: string | true;
}

const NOT_ADMIN = 'This desktop is a paired client, and a client never administers the fleet.';
const SETUP_WIZARD =
  'The add-host wizard works from this machine’s SSH config, and this desktop is a paired client. Add host asks the hub to add one instead.';

const REASONS = {
  // --- fleet administration: refused to a client by design -----------------
  // (No `add_host` / `discover_hosts`: since contract 13 the hub adds a host
  // for its owner's trusted full device, so both route — Add host reads the
  // hub's ~/.ssh/config and the hub probes what it adds, as the phone's does.)
  remove_host: { text: NOT_ADMIN, operator: true },
  hide_host: { text: NOT_ADMIN, operator: true },
  list_host_setups: { text: SETUP_WIZARD },
  save_host_setup: { text: SETUP_WIZARD },
  discard_host_setup: { text: SETUP_WIZARD },
  run_host_setup_check: { text: SETUP_WIZARD },
  // Not a command: `provision_hosts` for every host (Settings → Control
  // API). One host's content refresh routes — HostDetail's "Re-provision this
  // host" is `ROUTED_ACTIONS`' `provision_hosts`.
  provision_all: {
    text: 'Provisioning every host mints tokens and rewrites each host’s hooks, which is the hub operator’s job. The hub refreshes a stale host by itself, and a host’s own page can ask for it now.',
  },
  install_fleet_hook: { text: 'The hook would point at a control API, and this app runs none while it is paired with a hub.' },
  host_tokens: { text: 'Per-host tokens belong to the fleet’s owner; a paired client has none.' },
  mcp_configure: { text: 'The hub already runs this fleet’s control API, and a second one against the same fleet is what pairing exists to prevent.' },
  // Trackers (work graph M3): the hub's work_admin is master-only.
  add_tracker: { text: `Trackers and their credentials are fleet administration. ${NOT_ADMIN}`, operator: 'fleet-hub tracker add' },
  update_tracker: { text: `Trackers and their credentials are fleet administration. ${NOT_ADMIN}`, operator: 'fleet-hub tracker' },
  set_tracker_credential: { text: `Tracker credentials live on the hub. ${NOT_ADMIN}`, operator: 'fleet-hub tracker set-credential' },
  test_tracker: { text: `The hub tests its trackers with its own credentials. ${NOT_ADMIN}`, operator: 'fleet-hub tracker test' },
  remove_tracker: { text: `Trackers and their credentials are fleet administration. ${NOT_ADMIN}`, operator: 'fleet-hub tracker remove' },
  tracker_sync_metrics: { text: `Sync metrics live in the hub’s memory. ${NOT_ADMIN}`, operator: 'fleet-hub tracker status' },
  // Ticket keys: a project's prefix is `work_admin`, master-only.
  ticket_prefixes: { text: `Which prefix a project’s ticket keys carry is fleet administration. ${NOT_ADMIN}`, operator: 'fleet-hub tracker prefixes' },
  set_ticket_prefix: { text: `Which prefix a project’s ticket keys carry is fleet administration. ${NOT_ADMIN}`, operator: 'fleet-hub tracker prefix <PREFIX> --project <id>' },
  // Jev's Asana section proposals: applying one is `work_admin update`.
  status_map_proposals: { text: `Section proposals are tracker administration. ${NOT_ADMIN}`, operator: 'fleet-hub decide proposals' },
  decide_status_map_proposal: { text: `Applying a section proposal changes the tracker’s section map, which is fleet administration. ${NOT_ADMIN}`, operator: 'fleet-hub decide proposals apply|reject' },
  // Asset catalogs (Assets M6, R11): which catalogs exist is the master's
  // alone (`add_catalog` / `remove_catalog` are `MasterOnly`). Both commands
  // ROUTE, so this is not a command-named key: Settings → Catalogs shows a
  // paired desktop this reason (`resourceBlock`), and the commands keep their
  // routed verdicts.
  catalog_registry: {
    text: `Which catalogs exist, and who may change them, is fleet administration. ${NOT_ADMIN}`,
    operator: 'fleet-hub catalog add or remove, and fleet-hub client grant',
  },

  // --- things about THIS machine ------------------------------------------
  // (No `terminal` key: the terminal is not blocked by being a hub client.
  // `pty_open` spawns this machine's own `ssh`/`tmux`, so a paired desktop
  // attaches exactly as a standalone one does. The one session it cannot
  // attach is one on an agent host, and that is not the hub's doing — nothing
  // anywhere has an SSH route to it — so TerminalView says that itself.)
  check_local_prereqs: { text: 'The setup checklist is about running a fleet from this machine, and the hub runs it instead.' },
  tunnel_status: { text: 'The tunnels belong to the hub, which runs the fleet.' },
  mcp_status: { text: 'The hub runs this fleet’s control API; this app runs none while it is paired.' },
  catalog_spawn_author_session: {
    text: 'An author session would run in the catalog’s checkout on the hub’s machine, and the hub can’t start one. Edit the asset here instead.',
  },
  // Not a command: the fleet's settings route to the hub (declarative pages
  // P6); this is what a paired desktop says when the hub serves them none.
  fleet_settings: {
    text: 'The hub didn’t send its settings to this device: the hub is older than this app, or this device belongs to one organisation.',
    operator: 'fleet-hub settings, or set_setting',
  },
  refresh_account_usage: { text: 'This reads usage over this machine’s own SSH, which a paired desktop doesn’t use for the fleet.' },
  account_usage_history: { text: 'This app keeps no usage history while a hub runs the fleet.' },
  account_spend: { text: 'This app collects no usage while a hub runs the fleet, so it has no spend per account.' },
  record_host_placement: { text: 'The hub records placement decisions; there is nothing to record here.' },

  // --- acted out over this machine's SSH, and the hub has no tool for it ---
  repo_write: {
    text: 'The hub has no way to stage or commit for a paired desktop, so nothing writes under a running agent. Ask Claude to do it in the session.',
  },
  purge_project: {
    text: 'This deletes Claude Code state on every host over this machine’s SSH, and the hub can’t do it for a paired desktop.',
  },
  inspect_safe_kill: { text: 'This inspects the worktree over this machine’s SSH, and the hub can’t do it for a paired desktop. Ask Claude to check it in the session.' },
  discard_kill_session: { text: 'The hub can’t discard a worktree and kill in one step. Use Safe remove’s “Ask Claude” instead.' },
  dismiss_agent_session: { text: 'Use Kill instead: it removes an inactive agent from the list the same way.' },
} as const satisfies Record<string, Reason>;

export type HubAction = keyof typeof REASONS;

/** Every action this module has a reason for — the list the tests sweep. */
export const HUB_ACTIONS = Object.keys(REASONS) as HubAction[];

/**
 * Whether this process owns its fleet — standalone, and not pointed at a hub,
 * working or not. Everything that fetches or shows a fleet-owner-only panel
 * asks this rather than `!status.remote`, because a configured hub this launch
 * cannot use is not a hub client and is not standalone either: it owns
 * nothing.
 */
export function ownsTheFleet(status: HubStatus = get(hubStatus)): boolean {
  return !status.remote && !status.unavailable;
}

/**
 * F1's sentence, shared by `hubBlock` and `hubActionBlocked`: a hub is
 * configured but THIS launch could not use it, so the backend owns no fleet
 * at all and refuses every command — routed or refused — the same way.
 *
 * **Exported because it is the only copy.** `access.ts::noAttachReason` and
 * `share.ts::sessionActionBlocked` both have to say this — a terminal pane and
 * a disabled button owe the same explanation, and the one thing neither may
 * say is "this session is not yours" when the hub is the problem. They used to
 * hand-copy the sentence, with a comment claiming it mirrored this one;
 * nothing held the copies together, so the next edit here would have left the
 * other surfaces saying the old thing. One function, three callers.
 */
export function unavailableReason(status: HubStatus): string | null {
  if (!status.unavailable) return null;
  // Not "do it on the hub": the hub is the problem, and every action is
  // refused until it is fixed. The raw reason names settings keys; it stays
  // under the banner's and Settings' Details (review w16-5).
  return `Not available: ${hubUnavailableWords(status.unavailable)}. This app is set to use that hub, so it manages no fleet of its own until that is fixed — pair again, or Disconnect, in Settings → Hub & sync.`;
}

/**
 * Why `action` is unavailable from this window, or `null` when it is
 * available. `null` in standalone mode, always: nothing here may change what
 * a standalone app does.
 */
export function hubBlock(action: HubAction, status: HubStatus = get(hubStatus)): string | null {
  const unavailable = unavailableReason(status);
  if (unavailable) return unavailable;
  if (!status.remote) return null;
  const r: Reason = REASONS[action];
  if (r.operator === true) return `${r.text} Ask the hub’s operator to do it.`;
  return r.operator ? `${r.text} Ask the hub’s operator to do it (${r.operator}).` : r.text;
}

/**
 * The reason a resource's routed commands share when the hub refuses a client
 * the whole page: master-only commands that ROUTE (so `REASONS` has no key of
 * their name).
 */
const MASTER_ONLY_RESOURCE_COMMANDS: Readonly<Record<string, HubAction>> = {
  catalog_add_catalog: 'catalog_registry',
  catalog_remove_catalog: 'catalog_registry',
};

/**
 * Why a resource's page is read-only here, or `null` when it is not: the
 * reason of its update command, else its create, else its delete — a resource
 * with no update (Catalogs) is explained by the command a client would reach
 * for first. `null` in standalone mode, like `hubBlock`.
 */
export function resourceBlock(
  resource: { update?: { command: string }; create?: { command: string }; delete?: { command: string } },
  status: HubStatus = get(hubStatus),
): string | null {
  for (const a of [resource.update, resource.create, resource.delete]) {
    if (!a) continue;
    const key = Object.hasOwn(REASONS, a.command) ? (a.command as HubAction) : MASTER_ONLY_RESOURCE_COMMANDS[a.command];
    if (key) return hubBlock(key, status);
  }
  return null;
}

// ---- offline gating: routed mutations while the live link is down ---------

/**
 * Action keys for the mutations that ROUTE to the hub — the ones a paired
 * client can still send, as long as the live connection to it is up. This is
 * a SUBSET of the backend's routed commands (`src-tauri/src/backend/
 * verdicts.rs`): the mutations the UI actually gates on the connection
 * state, not every routed command — a routed read like `list_sessions` has
 * no control to disable and so is not here.
 *
 * `hub_verdicts.test.ts` holds this list (and `REASONS` below) accountable
 * to `hub_verdicts.generated.json`, the JSON generated straight from
 * `VERDICTS`, rather than this file importing that JSON and deriving from
 * it: keeping the literal here lets `RoutedAction` stay exactly the union
 * type it is today. The names usually match the `#[tauri::command]` fn name;
 * the one place they don't is `set_friendly_name`, whose command is
 * `set_session_friendly_name` (the tool it routes to is `set_friendly_name`,
 * which is what `tests_routing.rs` names the case after, and what the
 * cross-check test maps through too).
 */
export const ROUTED_ACTIONS = [
  // Task 6: import works from any host over SSH now, so it routes to the
  // hub's catalog_admin like every other asset-catalog mutation.
  'catalog_import_host',
  // Multi-harness F3a: a host's Codex choice is a catalog_admin action too.
  'catalog_set_host_harnesses',
  'send_prompt',
  'answer_form',
  'decline_form',
  'kill_session',
  // A session's shell terminals (step 5.3).
  'shell_terminals',
  'safe_kill_session',
  'rename_session',
  'set_friendly_name',
  // The Label field (M15 G2.7): a session's tags.
  'set_session_tags',
  // Link / Not related on a related-session proposal (M15 G4.3).
  'decide_related_session',
  // Pin / unpin a session (the Pinned section).
  'pin_session',
  'restart_session',
  'rewind_conversation',
  'spawn_review',
  // Send prompt's "when idle" (step 5.10): queue, list, take back.
  'queue_prompt',
  'queued_prompts',
  'cancel_queued_prompt',
  // The composer's context help: one claude -p the hub runs and books.
  'session_context_help',
  // The composer's prompt tuner: one claude -p the hub runs and books.
  'session_prompt_tune',
  'recreate_session',
  // `restore_host_sessions` batches `recreate_session` over a host's lost
  // rows and is `routed` in `verdicts.rs` like it — it was missing here, so
  // HostDetail's "Restore n lost sessions…" could not ask the hub half at all
  // (multi-user M1, F2b).
  'restore_host_sessions',
  'dismiss_ghost_session',
  'adopt_session',
  'place_transcript',
  'new_bg_session',
  'cancel_task',
  'probe_host',
  // Add host on a paired desktop: the hub's add_host (contract 13).
  'add_host',
  // A host's health checklist, run by the hub over its own SSH.
  'check_host',
  // The fleet-agent install job, run by the hub over its own SSH (4.9).
  'install_agent',
  // An account's nickname: the hub's set_account_nickname.
  'set_account_nickname',
  'move_session',
  'new_session',
  'repair_session',
  // One host's content refresh, from its page (the hub owner's device may ask
  // for it; every host at once is `REASONS.provision_all`).
  'provision_hosts',
  'link_session_work',
  'reject_session_work',
  'unlink_session_work',
  'confirm_session_work',
  'set_work_project_trust',
  'start_work',
  'start_work_multi',
  'abandon_start',
  'request_work_handover',
  'summarize_past_work',
  'draft_task_title',
  'tidy_apply',
  'unarchive_session_work',
  'dismiss_reopened',
  'name_session_work',
  'rename_work_item',
  // The board: a native item's status, set by dragging its card.
  'set_work_status',
  'keep_work_open',
  // Archive / Unarchive a ticket or task (the hub's work_link archive_item).
  'set_work_archived',
  // Task editing: a native item's title, notes and assignees.
  'edit_work_item',
  // Task comments: kept in fleet; deleting is the author's own.
  'comment_on_work',
  'delete_work_comment',
  // Promoting a task of fleet's own: the hub owns the trackers that create it.
  'promote_work_item',
  // Task attachments: kept in fleet; deleting is the author's own.
  'attach_to_work',
  'delete_work_attachment',
  // Sprints and releases: the hub decides who plans (a person's own; an
  // org's as its admin, or member when the org allows it).
  'work_bucket_admin',
  // The Work view (work graph M14): every write is `work_link { … }` on the
  // hub, so a paired desktop sends them while the link is up.
  'set_primary_work',
  'switch_session_work',
  'reconsider_work_link',
  'ack_work_link',
  'decide_work_batch',
  'place_work',
  'assign_work_org',
  'save_work_rule',
  'delete_work_rule',
  'save_work_view',
  'delete_work_view',
  'add_project',
  // Assets M6: the card verbs.
  'catalog_apply_changeset',
  'catalog_undo_changeset',
  'catalog_dismiss_changeset',
  'catalog_reject_changeset_items',
  'catalog_propose_changesets',
  'catalog_propose_layer_change',
  // Assets M6: the catalog set (the reads — layers_in, host_provenance,
  // drift_diff — are not listed).
  'catalog_add_catalog',
  'catalog_remove_catalog',
  'catalog_admit_catalog',
  'catalog_unadmit_catalog',
  // Multi-user M1 (F3): the Share sheet's three writes. They route to the
  // hub's sharing tools (T13), so a paired desktop can take them while the
  // live link is up and must not offer them while it is down.
  //
  // The milestone's three routed READS are deliberately absent, by this
  // list's own rule above: `session_access` (the sheet's grant list),
  // `capture_session` (the watcher's pane snapshot) and `my_grants` have no
  // control to disable. `my_grants` failing is a different surface again —
  // `access.ts`'s fail-closed arm, which disables what it cannot vouch for
  // and says the hub has not told this device who it is, rather than "try
  // again once connected".
  'session_share',
  'session_unshare',
  'session_narrow',
  // Gap plan G4.2: a recipient's ask for a wider level, and the owner's
  // grant or decline of one (both buttons, so both are writes here).
  'session_ask_access',
  'access_requests',
  // Gap plan G7.11: the Share sheet's "Trust now" (a device's mode, through
  // the hub's org_admin).
  'update_device',
  // Orchestration O1: the Missions tab's writes.
  'save_mission',
  'set_mission_state',
  // The Missions tab's Import plan.
  'import_mission_plan',
] as const;

export type RoutedAction = (typeof ROUTED_ACTIONS)[number];

const ROUTED_ACTION_SET: ReadonlySet<string> = new Set(ROUTED_ACTIONS);

/** The offline banner's sentence, reworded as a per-control reason: what a
 *  click would need instead of what the whole window is missing. `null` for
 *  `standalone` / `connected`, which never block a routed action. */
function offlineReason(conn: HubConnection): string | null {
  // Review w16-13: no URL and no attempt counter in a tooltip; the banner
  // names the hub and carries the contract revisions under its Details.
  switch (conn.state) {
    case 'connecting':
      return "Still connecting to the hub. Try again once it's connected.";
    case 'reconnecting':
    case 'offline':
      return "The hub is unreachable; reconnecting. Try again once it's back.";
    case 'hub_too_old':
      return 'The hub is older than this app supports, so the two are incompatible. Update the hub.';
    case 'hub_too_new':
      return 'The hub is newer than this app understands, so the two are incompatible. Update this app.';
    default:
      return null;
  }
}

/**
 * The one place a control checks BOTH halves of "can I send this": refusal
 * (`hubBlock`, `REASONS`) and the live connection (`$hubConnection`), so a
 * call site is a one-liner and the two checks cannot drift apart.
 *
 * Refusal wins over offline: an action the hub never accepts from a client
 * says so, not "try again once connected" (which it never will be, and would
 * send someone chasing a connection that was never the problem). `standalone`
 * — no hub configured — never blocks anything, in either half; a key that is
 * neither refused nor routed (reads, navigation, and the handful of commands
 * that run the same in both modes) is never blocked here either.
 */
export function hubActionBlocked(
  action: HubAction | RoutedAction,
  status: HubStatus = get(hubStatus),
  conn: HubConnection = get(hubConnection),
): string | null {
  if (Object.hasOwn(REASONS, action)) return hubBlock(action as HubAction, status);
  if (!ROUTED_ACTION_SET.has(action)) return null;
  const unavailable = unavailableReason(status);
  if (unavailable) return unavailable;
  if (!status.remote) return null;
  if (conn.state === 'connected') return null;
  return offlineReason(conn);
}

/**
 * The next step a hub client needs for an error that does not carry one.
 *
 * `E_CONFIRM_REQUIRED`: with `mcp.confirm_destructive` on, the hub holds
 * `kill_session`, `delete_worktree`, `move_session`, `cancel_task` and
 * `repair_session` until someone approves them. The request does show in this
 * window's confirmation queue and on the phone (`mcp_pending_confirms` routes,
 * review w16-3), but a yes only marks the hub's nonce approved: the hub
 * replays an approved call for its operator alone (`confirm_replay.rs`), and
 * this window's routed commands never retry with the `confirm_nonce`. So the
 * sentence must not promise that approving runs it, or that this window
 * follows — clicking again only mints a fresh request. It names who can do
 * it instead.
 *
 * (`E_UNAUTHORIZED` has its own sentence in `error_copy.ts` now; a second one
 * here said the same thing twice, with the URL in it — review w16-4.)
 */
export function hubNextStep(
  error: IpcError,
  status: HubStatus = get(hubStatus),
): string | null {
  if (!status.remote) return null;
  if (error.code !== 'E_CONFIRM_REQUIRED') return null;
  const held = 'The hub asks for a yes before this, and this window can’t carry a yes back to it yet, so approving the request won’t run it from here.';
  if (status.client_mode === 'readonly') return `${held} Ask the fleet’s owner to do it.`;
  return `${held} Ask the hub’s operator to do it, or to turn off confirming destructive actions on the hub.`;
}
